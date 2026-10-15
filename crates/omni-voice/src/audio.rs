//! The audio thread. It owns the cpal streams (which may be `!Send`), opens and
//! closes them on command, forwards preset swaps to the input callback and
//! recovers from unplugged devices.
//!
//! Signal path: input callback (downmix -> [`Processor`]) -> one SPSC ring per
//! output -> output callbacks ([`Reader`]: resample, drift control, upmix).

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{
    BufferSize, FromSample, Sample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize, SupportedStreamConfig,
};
use rtrb::RingBuffer;

use crate::devices::{self, Direction, backend};
use crate::dsp::{Chain, ChainPort, Params, Processor};
use crate::reader::Reader;
use crate::{Effect, Result, VoiceError, VoiceLevels, VoiceStatus};

/// Preferred processing rate; outputs are opened at the input's rate when possible.
const PREFERRED_RATE: u32 = 48_000;
/// Requested callback size (~5 ms) where the device lets us choose.
const CALLBACK_SECS: f32 = 0.005;
/// Ring capacity; latency is bounded far below this by [`Reader`].
const RING_CAPACITY_SECS: f32 = 0.5;
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(1);
const RETRY_INTERVAL: Duration = Duration::from_secs(3);
/// Longer than the preset crossfade.
const DRAIN_AFTER_SWAP: Duration = Duration::from_millis(250);

/// Which devices to use. `monitor`: `None` = off, `Some(None)` = default output.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Selection {
    pub input: Option<String>,
    pub output: Option<String>,
    pub monitor: Option<Option<String>>,
}

/// State shared between the engine handle, the audio thread and the callbacks.
#[derive(Default)]
pub(crate) struct Shared {
    pub status: Mutex<VoiceStatus>,
    pub params: AtomicParams,
    pub levels: Levels,
    /// Whether a general-purpose virtual cable output exists, once known.
    pub has_cable: Mutex<Option<bool>>,
}

impl Shared {
    pub fn status(&self) -> std::sync::MutexGuard<'_, VoiceStatus> {
        self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set_has_cable(&self, value: bool) {
        *self.has_cable.lock().unwrap_or_else(PoisonError::into_inner) = Some(value);
    }
}

/// [`Params`] as atomics, read by the input callback once per buffer.
pub(crate) struct AtomicParams {
    input_gain: AtomicU32,
    output_gain: AtomicU32,
    /// Linear threshold bits; 0 = gate off.
    gate: AtomicU32,
}

impl Default for AtomicParams {
    fn default() -> Self {
        let params = Self { input_gain: AtomicU32::new(0), output_gain: AtomicU32::new(0), gate: AtomicU32::new(0) };
        params.store(Params::default());
        params
    }
}

impl AtomicParams {
    pub fn store(&self, p: Params) {
        self.input_gain.store(p.input_gain.to_bits(), Ordering::Relaxed);
        self.output_gain.store(p.output_gain.to_bits(), Ordering::Relaxed);
        self.gate.store(p.gate_threshold.unwrap_or(0.0).to_bits(), Ordering::Relaxed);
    }

    fn load(&self) -> Params {
        let gate = f32::from_bits(self.gate.load(Ordering::Relaxed));
        Params {
            input_gain: f32::from_bits(self.input_gain.load(Ordering::Relaxed)),
            output_gain: f32::from_bits(self.output_gain.load(Ordering::Relaxed)),
            gate_threshold: (gate > 0.0).then_some(gate),
        }
    }
}

/// Peak meters. Non-negative floats order like their bit patterns, so `fetch_max` works.
#[derive(Default)]
pub(crate) struct Levels {
    input: AtomicU32,
    output: AtomicU32,
}

impl Levels {
    fn record(&self, input: f32, output: f32) {
        self.input.fetch_max(input.to_bits(), Ordering::Relaxed);
        self.output.fetch_max(output.to_bits(), Ordering::Relaxed);
    }

    pub fn take(&self) -> VoiceLevels {
        VoiceLevels {
            input: f32::from_bits(self.input.swap(0, Ordering::Relaxed)),
            output: f32::from_bits(self.output.swap(0, Ordering::Relaxed)),
        }
    }
}

enum Command {
    Open {
        selection: Selection,
        effects: Vec<Effect>,
        reply: SyncSender<Result<()>>,
    },
    Effects(Vec<Effect>),
    /// Fatal stream error reported by cpal for session `generation`.
    StreamError {
        generation: u64,
        message: String,
    },
    Stop,
}

/// Handle to the audio thread. Dropping it closes the streams and joins the thread.
pub(crate) struct Worker {
    tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(shared: Arc<Shared>) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let events = tx.clone();
        let thread = std::thread::Builder::new()
            .name("omni-voice".into())
            .spawn(move || run(rx, events, shared))
            .map_err(|e| VoiceError::Backend(format!("cannot start audio thread: {e}")))?;
        Ok(Self { tx, thread: Some(thread) })
    }

    /// (Re)open all streams; blocks until the devices are running or failed.
    pub fn open(&self, selection: Selection, effects: Vec<Effect>) -> Result<()> {
        let (reply, result) = mpsc::sync_channel(1);
        self.send(Command::Open { selection, effects, reply })?;
        result
            .recv_timeout(OPEN_TIMEOUT)
            .unwrap_or_else(|_| Err(VoiceError::Backend("audio devices did not respond".into())))
    }

    /// Hot-swap the effect chain without touching the streams.
    pub fn set_effects(&self, effects: Vec<Effect>) -> Result<()> {
        self.send(Command::Effects(effects))
    }

    fn send(&self, command: Command) -> Result<()> {
        self.tx.send(command).map_err(|_| VoiceError::Backend("audio thread stopped".into()))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(rx: Receiver<Command>, events: Sender<Command>, shared: Arc<Shared>) {
    let mut session: Option<Session> = None;
    let mut wanted: Option<(Selection, Vec<Effect>)> = None;
    let mut generation = 0u64;
    let mut retry_at: Option<Instant> = None;
    // After a swap the old chain comes back once its crossfade is done.
    let mut drain_at: Option<Instant> = None;

    loop {
        let wake = [retry_at, drain_at].into_iter().flatten().min();
        let command = match wake {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match command {
            Ok(Command::Open { selection, effects, reply }) => {
                session = None; // release the devices before opening them again
                generation += 1;
                let result = open(&selection, &effects, &shared, &events, generation);
                retry_at = result.as_ref().err().filter(|e| retryable(e)).map(|_| Instant::now() + RETRY_INTERVAL);
                let reply_result = publish(&shared, result.map(|s| session.insert(s).warning.clone()));
                wanted = Some((selection, effects));
                let _ = reply.send(reply_result);
            }
            Ok(Command::Effects(effects)) => {
                if let Some(s) = session.as_mut() {
                    s.set_effects(&effects);
                    drain_at = Some(Instant::now() + DRAIN_AFTER_SWAP);
                }
                if let Some((_, current)) = wanted.as_mut() {
                    *current = effects;
                }
            }
            Ok(Command::StreamError { generation: g, message }) if g == generation && session.is_some() => {
                tracing::warn!("voice stream failed: {message}");
                session = None;
                let _ = publish(&shared, Err(VoiceError::DeviceNotFound(message)));
                retry_at = Some(Instant::now() + RETRY_AFTER_ERROR);
            }
            Ok(Command::StreamError { .. }) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }

        if drain_at.is_some_and(|at| Instant::now() >= at) {
            drain_at = None;
            if let Some(s) = session.as_mut() {
                s.drop_retired();
            }
        }
        if session.is_none()
            && let (Some(at), Some((selection, effects))) = (retry_at, wanted.as_ref())
            && Instant::now() >= at
        {
            generation += 1;
            let result = open(selection, effects, &shared, &events, generation);
            retry_at = result.as_ref().err().filter(|e| retryable(e)).map(|_| Instant::now() + RETRY_INTERVAL);
            if result.is_ok() {
                tracing::info!("voice devices recovered");
            }
            let _ = publish(&shared, result.map(|s| session.insert(s).warning.clone()));
        }
    }
    drop(session); // close the streams on this thread, where they were created
    let mut status = shared.status();
    status.running = false;
    status.virtual_mic = None;
}

/// Missing / unplugged devices may come back; anything else needs a settings change.
fn retryable(err: &VoiceError) -> bool {
    matches!(err, VoiceError::DeviceNotFound(_))
}

/// Mirror an open result (with an optional non-fatal warning) into the status.
fn publish(shared: &Shared, result: Result<Option<String>>) -> Result<()> {
    let mut status = shared.status();
    status.running = result.is_ok();
    match result {
        Ok(warning) => {
            status.error = warning;
            Ok(())
        }
        Err(e) => {
            status.error = Some(e.to_string());
            status.virtual_mic = None;
            Err(e)
        }
    }
}

/// Open streams for `selection`. On success the status' `virtual_mic` is set.
fn open(
    selection: &Selection,
    effects: &[Effect],
    shared: &Arc<Shared>,
    events: &Sender<Command>,
    generation: u64,
) -> Result<Session> {
    let input = devices::find(selection.input.as_deref(), Direction::Input)?;
    let output = match selection.output.as_deref() {
        Some(id) => devices::find(Some(id), Direction::Output)?,
        None => {
            let found = devices::best_cable_output()?;
            shared.set_has_cable(found.is_some());
            found.ok_or_else(|| VoiceError::DeviceNotFound("virtual audio cable".into()))?
        }
    };
    // The monitor is a convenience: without it the voice must still reach the cable.
    let mut warning = None;
    let monitor = selection.monitor.as_ref().and_then(|id| match devices::find(id.as_deref(), Direction::Output) {
        Ok(m) => (!devices::same_device(&m, &output)).then_some(m),
        Err(e) => {
            warning = Some(format!("monitor unavailable: {e}"));
            None
        }
    });

    let output_name = devices::name(&output);
    let output_virtual = devices::is_virtual(&output);
    let input_name = devices::name(&input);
    if devices::same_device(&input, &output)
        || (output_virtual && input_name == devices::mic_name_for_output(&output_name))
    {
        return Err(VoiceError::Unsupported(format!(
            "\"{input_name}\" is both the microphone and the output; that would feed the voice back into itself"
        )));
    }
    if devices::cable_rank(&output_name).is_some() {
        shared.set_has_cable(true);
    }

    let in_cfg = pick_config(&input, Direction::Input, PREFERRED_RATE)?;
    let outputs = [Some(output), monitor]
        .into_iter()
        .flatten()
        .map(|d| pick_config(&d, Direction::Output, in_cfg.sample_rate()).map(|cfg| (d, cfg)))
        .collect::<Result<Vec<_>>>()?;

    let build = |fixed| Session::build(&input, in_cfg, &outputs, effects, fixed, shared, events, generation);
    // Small fixed buffers keep latency low; some drivers refuse them, so fall back.
    let mut session = build(true).or_else(|e| {
        tracing::debug!("voice: fixed buffer size refused ({e}), using device default");
        build(false)
    })?;
    session.warning = session.warning.take().or(warning);
    tracing::info!("voice running: {input_name} @ {} Hz -> {output_name}", in_cfg.sample_rate());
    if let Some(w) = &session.warning {
        tracing::warn!("voice: {w}");
    }
    shared.status().virtual_mic = output_virtual.then(|| devices::mic_name_for_output(&output_name));
    Ok(session)
}

/// Open streams of one enabled period. Field order matters: the input stops first.
struct Session {
    _input: Input,
    _outputs: Vec<cpal::Stream>,
    chains: rtrb::Producer<Box<Chain>>,
    retired: rtrb::Consumer<Box<Chain>>,
    sample_rate: f32,
    /// Non-fatal problem (monitor missing), shown as the status error while running.
    warning: Option<String>,
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    fn build(
        input: &cpal::Device,
        in_cfg: SupportedStreamConfig,
        outputs: &[(cpal::Device, SupportedStreamConfig)],
        effects: &[Effect],
        fixed: bool,
        shared: &Arc<Shared>,
        events: &Sender<Command>,
        generation: u64,
    ) -> Result<Self> {
        let rate = in_cfg.sample_rate();
        let in_block = Arc::new(AtomicUsize::new(0));
        let on_error = || stream_error_handler(events.clone(), generation);

        let mut streams = Vec::with_capacity(outputs.len());
        let mut rings = Vec::with_capacity(outputs.len());
        let mut warning = None;
        for (i, (device, cfg)) in outputs.iter().enumerate() {
            let (tx, rx) = RingBuffer::new((rate as f32 * RING_CAPACITY_SECS) as usize);
            let reader = Reader::new(rx, rate, cfg.sample_rate(), in_block.clone());
            match output_stream(device, *cfg, fixed, reader, on_error()) {
                Ok(stream) => {
                    streams.push(stream);
                    rings.push(tx);
                }
                // Only the first output (the cable) is required; the rest is the monitor.
                Err(e) if i > 0 => warning = Some(format!("monitor unavailable: {e}")),
                Err(e) => return Err(e),
            }
        }

        let (chains, incoming) = RingBuffer::new(4);
        let (retired_tx, retired) = RingBuffer::new(8);
        let sample_rate = rate as f32;
        let processor = Processor::new(sample_rate, Chain::new(effects, sample_rate))
            .with_port(ChainPort { incoming, retired: retired_tx });
        let side = InputSide {
            processor,
            rings,
            in_block,
            shared: shared.clone(),
            scratch: vec![0.0; 1024].into_boxed_slice(),
        };
        for stream in &streams {
            stream.play().map_err(backend)?;
        }
        let input = Input::open(input, in_cfg, fixed, side, on_error())?;
        Ok(Self { _input: input, _outputs: streams, chains, retired, sample_rate, warning })
    }

    fn set_effects(&mut self, effects: &[Effect]) {
        self.drop_retired();
        let chain = Box::new(Chain::new(effects, self.sample_rate));
        if self.chains.push(chain).is_err() {
            tracing::warn!("voice: preset swap queue full, change ignored");
        }
    }

    /// Chains the callback no longer uses are freed here, never on the audio thread.
    fn drop_retired(&mut self) {
        while self.retired.pop().is_ok() {}
    }
}

fn stream_error_handler(events: Sender<Command>, generation: u64) -> impl FnMut(cpal::Error) + Send + 'static {
    move |err| match err.kind() {
        // Recoverable or informational: the stream keeps running.
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied => {
            tracing::debug!("voice stream: {err}");
        }
        _ => {
            let _ = events.send(Command::StreamError { generation, message: err.to_string() });
        }
    }
}

/// The running microphone stream.
enum Input {
    Cpal(#[allow(dead_code)] cpal::Stream),
    #[cfg(target_os = "linux")]
    Pulse(#[allow(dead_code)] crate::pulse::Capture),
}

impl Input {
    fn open(
        device: &cpal::Device,
        cfg: SupportedStreamConfig,
        fixed: bool,
        side: InputSide,
        on_error: impl FnMut(cpal::Error) + Send + 'static,
    ) -> Result<Self> {
        #[cfg(target_os = "linux")]
        if let Some(source) = devices::pulse_name(device) {
            return crate::pulse::Capture::open(&source, cfg.sample_rate(), side).map(Self::Pulse);
        }
        let stream = input_stream(device, cfg, fixed, side, on_error)?;
        stream.play().map_err(backend)?;
        Ok(Self::Cpal(stream))
    }
}

/// Everything the input callback owns.
pub(crate) struct InputSide {
    processor: Processor,
    rings: Vec<rtrb::Producer<f32>>,
    in_block: Arc<AtomicUsize>,
    shared: Arc<Shared>,
    scratch: Box<[f32]>,
}

impl InputSide {
    /// Downmix interleaved `channels`-channel frames, then [`Self::process_mono`].
    fn process<T: Copy>(&mut self, data: &[T], channels: usize)
    where
        f32: FromSample<T>,
    {
        let scale = 1.0 / channels as f32;
        let frames = data.chunks_exact(channels).map(|f| f.iter().map(|&s| f32::from_sample(s)).sum::<f32>() * scale);
        self.process_mono(data.len() / channels, frames);
    }

    /// Process and publish one callback's worth of mono samples. Realtime-safe.
    pub(crate) fn process_mono(&mut self, frames: usize, mut samples: impl Iterator<Item = f32>) {
        self.processor.set_params(self.shared.params.load());
        self.in_block.store(frames, Ordering::Relaxed);
        let mut left = frames;
        while left > 0 {
            let n = left.min(self.scratch.len());
            let buf = &mut self.scratch[..n];
            for (out, s) in buf.iter_mut().zip(&mut samples) {
                *out = s;
            }
            self.processor.process(buf);
            for ring in &mut self.rings {
                // Full ring = the output stalled; dropping is the only realtime-safe option.
                let _ = ring.push_partial_slice(buf);
            }
            left -= n;
        }
        let (input, output) = self.processor.take_peaks();
        self.shared.levels.record(input, output);
    }
}

fn stream_config(cfg: &SupportedStreamConfig, fixed: bool) -> StreamConfig {
    let want = (cfg.sample_rate() as f32 * CALLBACK_SECS) as u32;
    let buffer_size = match *cfg.buffer_size() {
        SupportedBufferSize::Range { min, max } if fixed && min <= max => BufferSize::Fixed(want.clamp(min, max)),
        _ => BufferSize::Default,
    };
    StreamConfig { channels: cfg.channels(), sample_rate: cfg.sample_rate(), buffer_size }
}

const TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));

fn input_stream(
    device: &cpal::Device,
    cfg: SupportedStreamConfig,
    fixed: bool,
    side: InputSide,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream> {
    fn typed<T: SizedSample + Send + 'static>(
        device: &cpal::Device,
        config: StreamConfig,
        mut side: InputSide,
        on_error: impl FnMut(cpal::Error) + Send + 'static,
    ) -> Result<cpal::Stream>
    where
        f32: FromSample<T>,
    {
        let channels = usize::from(config.channels.max(1));
        device
            .build_input_stream::<T, _, _>(config, move |data, _| side.process(data, channels), on_error, TIMEOUT)
            .map_err(backend)
    }
    let config = stream_config(&cfg, fixed);
    match cfg.sample_format() {
        SampleFormat::F32 => typed::<f32>(device, config, side, on_error),
        SampleFormat::I32 => typed::<i32>(device, config, side, on_error),
        SampleFormat::I24 => typed::<cpal::I24>(device, config, side, on_error),
        SampleFormat::I16 => typed::<i16>(device, config, side, on_error),
        SampleFormat::F64 => typed::<f64>(device, config, side, on_error),
        SampleFormat::U16 => typed::<u16>(device, config, side, on_error),
        SampleFormat::I8 => typed::<i8>(device, config, side, on_error),
        SampleFormat::U8 => typed::<u8>(device, config, side, on_error),
        other => Err(VoiceError::Unsupported(format!("sample format {other}"))),
    }
}

fn output_stream(
    device: &cpal::Device,
    cfg: SupportedStreamConfig,
    fixed: bool,
    reader: Reader,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream> {
    fn typed<T: SizedSample + FromSample<f32> + Send + 'static>(
        device: &cpal::Device,
        config: StreamConfig,
        mut reader: Reader,
        on_error: impl FnMut(cpal::Error) + Send + 'static,
    ) -> Result<cpal::Stream> {
        let channels = usize::from(config.channels.max(1));
        device
            .build_output_stream::<T, _, _>(
                config,
                move |data: &mut [T], _| {
                    reader.render(data.len() / channels, |i, sample| {
                        data[i * channels..(i + 1) * channels].fill(T::from_sample(sample));
                    });
                },
                on_error,
                TIMEOUT,
            )
            .map_err(backend)
    }
    let config = stream_config(&cfg, fixed);
    match cfg.sample_format() {
        SampleFormat::F32 => typed::<f32>(device, config, reader, on_error),
        SampleFormat::I32 => typed::<i32>(device, config, reader, on_error),
        SampleFormat::I24 => typed::<cpal::I24>(device, config, reader, on_error),
        SampleFormat::I16 => typed::<i16>(device, config, reader, on_error),
        SampleFormat::F64 => typed::<f64>(device, config, reader, on_error),
        SampleFormat::U16 => typed::<u16>(device, config, reader, on_error),
        SampleFormat::I8 => typed::<i8>(device, config, reader, on_error),
        SampleFormat::U8 => typed::<u8>(device, config, reader, on_error),
        other => Err(VoiceError::Unsupported(format!("sample format {other}"))),
    }
}

/// Preference among the formats we can convert; `None` = unsupported.
fn format_rank(format: SampleFormat) -> Option<u8> {
    use SampleFormat::*;
    Some(match format {
        F32 => 0,
        I32 => 1,
        I24 => 2,
        I16 => 3,
        F64 => 4,
        U16 => 5,
        I8 => 6,
        U8 => 7,
        _ => return None,
    })
}

/// Pick a config at `want_rate` (else 48 kHz, else the device default rate),
/// preferring f32 and the device's default channel count.
fn pick_config(device: &cpal::Device, dir: Direction, want_rate: u32) -> Result<SupportedStreamConfig> {
    let (default, ranges) = match dir {
        Direction::Input => (device.default_input_config(), device.supported_input_configs().map(|r| r.collect())),
        Direction::Output => (device.default_output_config(), device.supported_output_configs().map(|r| r.collect())),
    };
    let default = default.map_err(backend)?;
    let ranges: Vec<cpal::SupportedStreamConfigRange> = ranges.unwrap_or_default();
    for rate in [want_rate, PREFERRED_RATE, default.sample_rate()] {
        let best = ranges
            .iter()
            .filter(|r| r.contains_rate(rate) && r.channels() > 0)
            .filter_map(|r| format_rank(r.sample_format()).map(|rank| (rank, r)))
            .min_by_key(|(rank, r)| (*rank, r.channels() != default.channels(), r.channels()));
        if let Some((_, range)) = best {
            return Ok(range.with_sample_rate(rate));
        }
    }
    match format_rank(default.sample_format()) {
        Some(_) => Ok(default),
        None => Err(VoiceError::Unsupported(format!("{} has no usable sample format", devices::name(device)))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-device smoke test of the output path (ring -> Reader -> cpal), no microphone needed.
    /// OMNI_VOICE_TEST_OUTPUT=<device id or name> picks the device, else the first virtual one.
    #[test]
    #[ignore = "plays a quiet tone on a real output device"]
    fn live_output_stream_consumes_in_real_time() {
        let device = match std::env::var("OMNI_VOICE_TEST_OUTPUT") {
            Ok(id) => devices::find(Some(&id), Direction::Output).unwrap(),
            Err(_) => Direction::Output
                .list(&cpal::default_host())
                .unwrap()
                .into_iter()
                .find(devices::is_virtual)
                .expect("no virtual output device; set OMNI_VOICE_TEST_OUTPUT"),
        };
        let cfg = pick_config(&device, Direction::Output, PREFERRED_RATE).unwrap();
        let rate = cfg.sample_rate();
        let block = rate as usize / 100;
        let (mut tx, rx) = RingBuffer::new(rate as usize);
        let reader = Reader::new(rx, rate, rate, Arc::new(AtomicUsize::new(block)));
        let (events, errors) = mpsc::channel();
        let stream = output_stream(&device, cfg, true, reader, stream_error_handler(events, 1)).unwrap();
        stream.play().unwrap();
        println!(
            "{} @ {rate} Hz, {} ch, {:?}, callback {:?} frames",
            devices::name(&device),
            cfg.channels(),
            cfg.sample_format(),
            stream.buffer_size()
        );

        // Produce a -30 dB tone paced by the wall clock, like a microphone would.
        let start = Instant::now();
        let (mut produced, mut phase, mut worst_backlog) = (0usize, 0.0f32, 0usize);
        let mut buf = vec![0.0f32; block];
        while start.elapsed() < Duration::from_secs(3) {
            let due = (start.elapsed().as_secs_f64() * f64::from(rate)) as usize;
            while produced + block <= due {
                for s in &mut buf {
                    phase = (phase + 440.0 / rate as f32).fract();
                    *s = 0.03 * (std::f32::consts::TAU * phase).sin();
                }
                assert!(tx.push_partial_slice(&buf).1.is_empty(), "ring overflow");
                produced += block;
            }
            if start.elapsed() > Duration::from_millis(500) {
                worst_backlog = worst_backlog.max(tx.buffer().capacity() - tx.slots());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        std::thread::sleep(Duration::from_millis(200));
        let left = tx.buffer().capacity() - tx.slots();
        drop(stream);

        let backlog_ms = worst_backlog as f64 * 1000.0 / f64::from(rate);
        println!("worst ring backlog {backlog_ms:.1} ms, left after stop {left}");
        assert!(errors.try_recv().is_err(), "stream reported an error");
        assert!(backlog_ms < 40.0, "latency {backlog_ms} ms");
        assert_eq!(left, 0, "output stopped consuming");
    }
}
