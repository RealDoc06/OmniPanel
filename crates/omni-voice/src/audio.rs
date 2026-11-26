//! The audio thread. It owns the cpal streams (which may be `!Send`), opens and
//! closes them on command, forwards preset swaps to the input callback and
//! recovers from unplugged devices. The optional monitor output comes and goes
//! on its own without interrupting the main output.
//!
//! Signal path: input callback (downmix -> [`Processor`]) -> one SPSC ring per
//! output -> output callbacks ([`Reader`]: resample, drift control, upmix).
//! Shared app sound ([`crate::share`]) joins after the processor, on the way to
//! the main output only.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
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
use crate::share::{self, AppCapture, Mixer, MixerPort, Shares};
use crate::{Effect, Result, VoiceError, VoiceLevels, VoiceStatus};

/// Preferred processing rate; outputs are opened at the input's rate when possible.
const PREFERRED_RATE: u32 = 48_000;
/// Requested callback size (~5 ms) where the device lets us choose.
const CALLBACK_SECS: f32 = 0.005;
/// Ring capacity; latency is bounded far below this by [`Reader`]. Roomy so even
/// huge device callbacks (~0.6 s each way) can prime and play without gaps.
pub(crate) const RING_CAPACITY_SECS: f64 = 2.0;
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// How long closing the streams may take before the audio thread is abandoned.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(1);
const RETRY_INTERVAL: Duration = Duration::from_secs(3);
/// cpal's macOS input stays on the device it opened, so a session on the default
/// microphone polls for default changes. WASAPI default-device streams and macOS
/// default outputs follow the default by themselves.
const FOLLOW_DEFAULT_INPUT: bool = cfg!(target_os = "macos");
/// A lost monitor is brought back by polling. Not on Linux, where every cpal
/// device query used to open a new PulseAudio connection (they share one now,
/// see `devices::host`; polling there is untried).
const POLL_MONITOR: bool = cfg!(not(target_os = "linux"));
const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Why apps are not shared into an output that is not a virtual device.
const NOT_A_VIRTUAL_MIC: &str =
    "the voice output is not a virtual microphone; choose one under Voice changer to share app sound";
/// Longer than the preset crossfade.
const DRAIN_AFTER_SWAP: Duration = Duration::from_millis(250);

/// Which devices to use. `monitor`: `None` = off, `Some(None)` = default output.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Selection {
    pub input: Option<String>,
    pub output: Option<String>,
    pub monitor: Option<Option<String>>,
}

/// Everything the audio thread is asked to run. Only a new `selection` reopens
/// the streams; effects and shared apps change while they run.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Wanted {
    pub selection: Selection,
    /// The voice changer is on. Without it the streams only run to share apps,
    /// which needs a virtual output: on speakers the microphone would just be
    /// played back at the user.
    pub voice: bool,
    pub effects: Vec<Effect>,
    /// Apps whose sound is mixed into the main output.
    pub shares: Shares,
}

/// State shared between the engine handle, the audio thread and the callbacks.
#[derive(Default)]
pub(crate) struct Shared {
    pub status: Mutex<VoiceStatus>,
    pub params: AtomicParams,
    pub levels: Levels,
    /// Linear gain of the shared app sound (`f32` bits), read by the input callback.
    pub share_gain: AtomicU32,
    /// The number of the latest change to the list of shared apps the host committed
    /// ([`Shares::revision`]). A session only mixes app sound while its capture is
    /// lined up with exactly that change ([`ShareGate`]), see
    /// [`crate::VoiceEngine::commit_shares`].
    pub shares_committed: AtomicU64,
    /// Whether a general-purpose virtual cable output exists, once known.
    pub has_cable: Mutex<Option<bool>>,
}

/// How far a session's capture is, for its own microphone callback. Per session:
/// one that was left behind (its audio thread stuck in a backend call) must not
/// open again because a newer session caught up.
#[derive(Default)]
pub(crate) struct ShareGate {
    /// The [`Shares::revision`] the capture was last lined up with.
    applied: AtomicU64,
    /// Counts the times an app stopped being shared: the input callback then
    /// throws away what its mixer holds, so nothing captured of it is still heard.
    removed: AtomicU32,
}

impl Shared {
    pub fn status(&self) -> std::sync::MutexGuard<'_, VoiceStatus> {
        self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set_has_cable(&self, value: bool) {
        *self.has_cable.lock().unwrap_or_else(PoisonError::into_inner) = Some(value);
    }
}

/// Cancellation and publication share a lock, so a dropped worker cannot publish late.
struct WorkerState {
    shared: Arc<Shared>,
    active: Mutex<bool>,
}

impl WorkerState {
    fn new(shared: Arc<Shared>) -> Self {
        Self { shared, active: Mutex::new(true) }
    }

    fn update(&self, f: impl FnOnce(&Shared)) {
        let active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        if *active {
            f(&self.shared);
        }
    }

    fn cancel(&self) {
        *self.active.lock().unwrap_or_else(PoisonError::into_inner) = false;
    }

    fn is_active(&self) -> bool {
        *self.active.lock().unwrap_or_else(PoisonError::into_inner)
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
        wanted: Wanted,
        reply: SyncSender<Result<()>>,
    },
    Effects(Vec<Effect>),
    Shares(Shares),
    /// What the shared apps play changed (sent by the app capture).
    SharesChanged,
    /// Fatal error reported by cpal for the stream opened as `generation`
    /// (a session's microphone and output, or one monitor attach).
    StreamError {
        generation: u64,
        stream: &'static str,
        message: String,
    },
    Stop,
}

/// Handle to the audio thread. Dropping it closes the streams and joins the thread
/// (or abandons it after [`STOP_TIMEOUT`]).
pub(crate) struct Worker {
    tx: Sender<Command>,
    state: Arc<WorkerState>,
    thread: Option<JoinHandle<()>>,
    /// Disconnects when the thread is done.
    done: Receiver<()>,
}

impl Worker {
    pub fn spawn(shared: Arc<Shared>) -> Result<Self> {
        let state = Arc::new(WorkerState::new(shared));
        let worker_state = state.clone();
        let (tx, rx) = mpsc::channel();
        let events = tx.clone();
        let (done_tx, done) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("omni-voice".into())
            .spawn(move || {
                let _done = done_tx;
                run(rx, events, worker_state);
            })
            .map_err(|e| VoiceError::Backend(format!("cannot start audio thread: {e}")))?;
        Ok(Self { tx, state, thread: Some(thread), done })
    }

    /// (Re)open all streams; blocks until the devices are running or failed.
    pub fn open(&self, wanted: Wanted) -> Result<()> {
        let (reply, result) = mpsc::sync_channel(1);
        self.send(Command::Open { wanted, reply })?;
        result
            .recv_timeout(OPEN_TIMEOUT)
            .unwrap_or_else(|_| Err(VoiceError::Backend("audio devices did not respond".into())))
    }

    /// Hot-swap the effect chain without touching the streams.
    pub fn set_effects(&self, effects: Vec<Effect>) -> Result<()> {
        self.send(Command::Effects(effects))
    }

    /// Change which apps are shared without touching the streams.
    pub fn set_shares(&self, shares: Shares) -> Result<()> {
        self.send(Command::Shares(shares))
    }

    fn send(&self, command: Command) -> Result<()> {
        self.tx.send(command).map_err(|_| VoiceError::Backend("audio thread stopped".into()))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.state.cancel();
        let _ = self.tx.send(Command::Stop);
        // A backend call that never returns must not hang disabling or quitting.
        if let Err(RecvTimeoutError::Timeout) = self.done.recv_timeout(STOP_TIMEOUT) {
            tracing::warn!("voice: audio thread did not stop, abandoning it");
        } else if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The audio thread's loop. Publications stop as soon as its handle is dropped;
/// status updates on exit are left to [`crate::VoiceEngine`].
fn run(rx: Receiver<Command>, events: Sender<Command>, worker: Arc<WorkerState>) {
    let mut session: Option<Session> = None;
    let mut wanted: Option<Wanted> = None;
    // Counts sessions and monitor attaches, so errors of closed streams are ignored.
    let mut generation = 0u64;
    let mut retry_at: Option<Instant> = None;
    // After a swap the old chain comes back once its crossfade is done.
    let mut drain_at: Option<Instant> = None;
    let mut poll_at: Option<Instant> = None;

    loop {
        let wake = [retry_at, drain_at, poll_at].into_iter().flatten().min();
        let command = match wake {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match command {
            Ok(Command::Open { wanted: new, reply }) => {
                session = None; // release the devices before opening them again
                // A poll due for the old session would find the new microphone "stalled".
                poll_at = None;
                let result = open(&new, &worker, &events, &mut generation);
                retry_at = result.as_ref().err().filter(|e| retryable(e)).map(|_| Instant::now() + RETRY_INTERVAL);
                let reply_result = publish(&worker, result.map(|s| session.insert(s).warning.clone()));
                wanted = Some(new);
                let _ = reply.send(reply_result);
            }
            Ok(Command::Effects(effects)) => {
                if let Some(s) = session.as_mut() {
                    s.set_effects(&effects);
                    drain_at = Some(Instant::now() + DRAIN_AFTER_SWAP);
                }
                if let Some(wanted) = wanted.as_mut() {
                    wanted.effects = effects;
                }
            }
            Ok(Command::Shares(shares)) => {
                if let Some(wanted) = wanted.as_mut() {
                    wanted.shares = shares;
                }
                if let (Some(s), Some(wanted)) = (session.as_mut(), wanted.as_ref()) {
                    s.sync_shares(&wanted.shares, &worker, &events);
                    // The feeds of apps no longer shared end a moment later.
                    drain_at = Some(Instant::now() + DRAIN_AFTER_SWAP);
                }
            }
            Ok(Command::SharesChanged) => {
                // A capture that failed is left to the poll: its last events must not reopen it at once.
                if let (Some(s), Some(wanted)) = (session.as_mut().filter(|s| s.capture.is_some()), wanted.as_ref()) {
                    s.sync_shares(&wanted.shares, &worker, &events);
                    // The feeds of streams that ended come back a moment later.
                    drain_at = Some(Instant::now() + DRAIN_AFTER_SWAP);
                }
            }
            Ok(Command::StreamError { generation: g, stream, message }) => match session.as_mut() {
                Some(s) if s.generation == g => {
                    tracing::warn!("voice {stream} failed: {message}");
                    session = None;
                    let _ = publish(&worker, Err(VoiceError::DeviceNotFound(format!("{stream}: {message}"))));
                    retry_at = Some(Instant::now() + RETRY_AFTER_ERROR);
                }
                // Only the monitor is lost: the voice keeps reaching the main output.
                Some(s) if s.monitor.as_ref().is_some_and(|m| m.generation == g) => {
                    tracing::warn!("voice {stream} failed: {message}");
                    s.monitor = None;
                    s.warning = Some(format!("monitor unavailable: {message}"));
                    worker.update(|shared| shared.status().error.clone_from(&s.warning));
                }
                _ => {}
            },
            Err(RecvTimeoutError::Timeout) => {}
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }

        if !worker.is_active() {
            break;
        }
        let now = Instant::now();
        if drain_at.is_some_and(|at| now >= at) {
            drain_at = None;
            if let Some(s) = session.as_mut() {
                s.drop_retired();
                // A swap the full queue refused goes out now (or on the next drain).
                if s.unsent.is_some() {
                    s.send_unsent();
                    drain_at = Some(now + DRAIN_AFTER_SWAP);
                }
            }
        }
        if poll_at.is_some_and(|at| now >= at) {
            poll_at = None;
            if let (Some(s), Some(wanted)) = (session.as_mut(), wanted.as_ref()) {
                if s.input_stalled() {
                    let message = "stopped delivering audio".into();
                    let _ =
                        events.send(Command::StreamError { generation: s.generation, stream: "microphone", message });
                } else if s.default_input_moved() {
                    // Open on the new default first: if it can't be used (say it is the
                    // cable itself), the running session stays.
                    match open(wanted, &worker, &events, &mut generation) {
                        Ok(new) => {
                            tracing::info!("voice: switched to the new default microphone");
                            let _ = publish(&worker, Ok(session.insert(new).warning.clone()));
                        }
                        Err(e) => {
                            tracing::warn!("voice: keeping the microphone, the new default is unusable: {e}");
                            if let Some(input) = s.default_input.as_mut() {
                                input.switch_failed(Instant::now());
                            }
                        }
                    }
                } else {
                    s.retry_monitor(&wanted.selection, &events, &mut generation);
                    worker.update(|shared| shared.status().error.clone_from(&s.warning));
                    // The safety net of the app capture: reopens a failed one and
                    // catches what its change events did not report.
                    if !wanted.shares.apps.is_empty() {
                        s.sync_shares(&wanted.shares, &worker, &events);
                    }
                }
            }
        }
        if session.is_none()
            && let (Some(at), Some(wanted)) = (retry_at, wanted.as_ref())
            && now >= at
        {
            let result = open(wanted, &worker, &events, &mut generation);
            retry_at = result.as_ref().err().filter(|e| retryable(e)).map(|_| Instant::now() + RETRY_INTERVAL);
            if result.is_ok() {
                tracing::info!("voice devices recovered");
            }
            let _ = publish(&worker, result.map(|s| session.insert(s).warning.clone()));
        }
        poll_at = match (&session, &wanted) {
            (Some(s), Some(wanted)) if s.needs_poll(wanted) => poll_at.or_else(|| Some(Instant::now() + POLL_INTERVAL)),
            _ => None,
        };
    }
    drop(session); // close the streams on this thread, where they were created
}

/// Missing / unplugged devices may come back; anything else needs a settings change.
fn retryable(err: &VoiceError) -> bool {
    matches!(err, VoiceError::DeviceNotFound(_))
}

/// Mirror an open result (with an optional non-fatal warning) into the status.
fn publish(worker: &WorkerState, result: Result<Option<String>>) -> Result<()> {
    worker.update(|shared| {
        let mut status = shared.status();
        status.running = result.is_ok();
        match &result {
            Ok(warning) => status.error.clone_from(warning),
            Err(e) => {
                status.error = Some(e.to_string());
                status.virtual_mic = None;
                status.share_error = None;
                status.share_waiting.clear();
            }
        }
    });
    result.map(|_| ())
}

/// Open streams for `wanted`. On success the status' `virtual_mic` is set.
fn open(wanted: &Wanted, worker: &WorkerState, events: &Sender<Command>, generation: &mut u64) -> Result<Session> {
    let Wanted { selection, voice, effects, shares } = wanted;
    let input = devices::find(selection.input.as_deref(), Direction::Input)?;
    let output = match selection.output.as_deref() {
        Some(id) => devices::find(Some(id), Direction::Output)?,
        None => {
            let found = devices::best_cable_output()?;
            worker.update(|shared| shared.set_has_cable(found.is_some()));
            found.ok_or_else(|| VoiceError::DeviceNotFound("virtual audio cable".into()))?
        }
    };

    let output_name = devices::name(&output);
    let output_virtual = devices::is_virtual(&output);
    if !voice && !output_virtual {
        return Err(VoiceError::Backend(NOT_A_VIRTUAL_MIC.into()));
    }
    let input_name = devices::name(&input);
    // A virtual cable plays its output back into its own input; the two directions
    // of a physical interface (mic in, headphones out) are separate.
    if output_virtual
        && (devices::same_device(&input, &output) || input_name == devices::mic_name_for_output(&output_name))
    {
        return Err(VoiceError::Unsupported(format!(
            "\"{input_name}\" is both the microphone and the output; that would feed the voice back into itself"
        )));
    }
    if devices::cable_rank(&output_name).is_some() {
        worker.update(|shared| shared.set_has_cable(true));
    }

    let in_cfg = pick_config(&input, Direction::Input, PREFERRED_RATE)?;
    let out_cfg = pick_config(&output, Direction::Output, in_cfg.sample_rate())?;
    let mut build = |fixed| {
        *generation += 1;
        Session::build(&input, in_cfg, &output, out_cfg, effects, fixed, &worker.shared, events, *generation)
    };
    // Small fixed buffers keep latency low; some drivers refuse them, so fall back.
    let mut session = build(true).or_else(|e| {
        tracing::debug!("voice: fixed buffer size refused ({e}), using device default");
        build(false)
    })?;
    if FOLLOW_DEFAULT_INPUT && selection.input.is_none() {
        session.default_input = Some(DefaultInput { id: devices::id(&input), retry_at: None });
    }
    // The monitor is a convenience: without it the voice must still reach the cable.
    if let Err(e) = session.attach_monitor(selection, events, generation) {
        session.warning = Some(format!("monitor unavailable: {e}"));
    }
    tracing::info!("voice running: {input_name} @ {} Hz -> {output_name}", in_cfg.sample_rate());
    if let Some(w) = &session.warning {
        tracing::warn!("voice: {w}");
    }
    worker.update(|shared| {
        shared.status().virtual_mic = output_virtual.then(|| devices::mic_name_for_output(&output_name));
    });
    session.output_virtual = output_virtual;
    session.sync_shares(shares, worker, events);
    Ok(session)
}

/// Open streams of one enabled period. Field order matters: the input stops first.
struct Session {
    _input: Input,
    _output: cpal::Stream,
    monitor: Option<Monitor>,
    generation: u64,
    rate: u32,
    in_block: Arc<AtomicUsize>,
    output_id: String,
    /// The system default microphone in use, when the selection follows the default.
    default_input: Option<DefaultInput>,
    /// Input callback count and the value at the previous poll, for a microphone
    /// stream that can't report errors (see [`Input::reports_errors`]).
    watchdog: Option<(Arc<AtomicU32>, u32)>,
    /// Monitor rings for the input callback, and the ones it replaced (freed here).
    monitor_rings: rtrb::Producer<rtrb::Producer<f32>>,
    retired_rings: rtrb::Consumer<rtrb::Producer<f32>>,
    chains: rtrb::Producer<Box<Chain>>,
    retired: rtrb::Consumer<Box<Chain>>,
    /// Newest chain the full swap queue did not take yet; it replaces older unsent ones.
    unsent: Option<Box<Chain>>,
    /// Non-fatal problem (monitor missing), shown as the status error while running.
    warning: Option<String>,
    /// Feeds of shared app sound for the input callback, and the capture making
    /// them (the first thing a session lets go of, see its `Drop`).
    mixer: MixerPort,
    capture: Option<AppCapture>,
    gate: Arc<ShareGate>,
    /// The main output is a virtual device. Anything else is speakers: the app and
    /// the microphone must not be played there.
    output_virtual: bool,
    /// A [`Command::SharesChanged`] is on its way: the capture reports bursts of
    /// changes (every volume step of any app is one), a single sync answers them all.
    shares_changed: Arc<AtomicBool>,
    /// The apps the capture was last lined up with.
    shares_synced: Vec<String>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // First, before the devices: closing those can wait on the sound server, and
        // a session given up on meanwhile must not keep recording apps.
        self.capture = None;
    }
}

/// The microphone actually open, with a delay before retrying a failed switch.
struct DefaultInput {
    id: String,
    retry_at: Option<Instant>,
}

impl DefaultInput {
    fn switch_failed(&mut self, now: Instant) {
        self.retry_at = Some(now + RETRY_INTERVAL);
    }

    fn should_switch(&self, desired: &str, now: Instant) -> bool {
        self.id != desired && self.retry_at.is_none_or(|at| now >= at)
    }
}

/// The optional second output, attached and detached while the session runs.
struct Monitor {
    _stream: cpal::Stream,
    generation: u64,
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    fn build(
        input: &cpal::Device,
        in_cfg: SupportedStreamConfig,
        output: &cpal::Device,
        out_cfg: SupportedStreamConfig,
        effects: &[Effect],
        fixed: bool,
        shared: &Arc<Shared>,
        events: &Sender<Command>,
        generation: u64,
    ) -> Result<Self> {
        let rate = in_cfg.sample_rate();
        let in_block = Arc::new(AtomicUsize::new(0));
        let callbacks = Arc::new(AtomicU32::new(0));
        let (ring, rx) = ring(rate);
        let reader = Reader::new(rx, rate, out_cfg.sample_rate(), in_block.clone());
        let on_error = stream_error_handler(events.clone(), generation, "output");
        let stream = output_stream(output, out_cfg, fixed, reader, on_error)?;

        let (chains, incoming) = RingBuffer::new(4);
        let (retired_tx, retired) = RingBuffer::new(8);
        let (monitor_rings, monitor_in) = RingBuffer::new(2);
        let (retired_rings_tx, retired_rings) = RingBuffer::new(2);
        let (mixer, mixer_port) = share::mixer(rate);
        let gate = Arc::<ShareGate>::default();
        let sample_rate = rate as f32;
        let processor = Processor::new(sample_rate, Chain::new(effects, sample_rate))
            .with_port(ChainPort { incoming, retired: retired_tx });
        let side = InputSide {
            processor,
            output: ring,
            monitor: None,
            monitor_rings: monitor_in,
            retired_rings: retired_rings_tx,
            mixer,
            gate: gate.clone(),
            removed: 0,
            in_block: in_block.clone(),
            callbacks: callbacks.clone(),
            shared: shared.clone(),
            scratch: vec![0.0; 1024].into_boxed_slice(),
        };
        stream.play().map_err(backend)?;
        let on_error = stream_error_handler(events.clone(), generation, "microphone");
        let input = Input::open(input, in_cfg, fixed, side, on_error)?;
        Ok(Self {
            watchdog: (!input.reports_errors()).then_some((callbacks, 0)),
            _input: input,
            _output: stream,
            monitor: None,
            generation,
            rate,
            in_block,
            output_id: devices::id(output),
            default_input: None,
            monitor_rings,
            retired_rings,
            chains,
            retired,
            unsent: None,
            warning: None,
            mixer: mixer_port,
            capture: None,
            gate,
            output_virtual: false,
            shares_changed: Arc::default(),
            shares_synced: Vec::new(),
        })
    }

    /// Capture exactly the apps in `shares` that play now (none: stop capturing)
    /// and put any problem in the status. Run again whenever the capture reports
    /// a change; a failed capture is reopened by the next poll.
    fn sync_shares(&mut self, shares: &Shares, worker: &WorkerState, events: &Sender<Command>) {
        let Shares { apps, revision } = shares;
        // Cleared first: a change during the sync asks for another one.
        self.shares_changed.store(false, Ordering::Relaxed);
        // The capture is about to change: shut until it is lined up with the new list.
        if *apps != self.shares_synced {
            self.gate.applied.store(0, Ordering::Release);
        }
        let result = if apps.is_empty() {
            self.capture = None;
            Ok(Vec::new())
        } else if !self.output_virtual {
            Err(VoiceError::Backend(NOT_A_VIRTUAL_MIC.into()))
        } else {
            self.capture_shares(apps, events)
        };
        self.mixer.drop_retired();
        if result.is_err() {
            self.capture = None;
        }
        let (waiting, error) = match result {
            Ok(waiting) => (waiting, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        worker.update(|shared| {
            let mut status = shared.status();
            // A lasting problem (permission not given yet) comes back at every poll.
            if let Some(e) = error.as_ref().filter(|e| status.share_error.as_ref() != Some(e)) {
                tracing::warn!("voice: cannot share app sound: {e}");
            }
            status.share_error = error;
            status.share_waiting = waiting;
            // The capture of an app that is no longer shared has ended above; what the
            // mixer still holds of it goes too, before the mix may open for the new list.
            if self.shares_synced.iter().any(|app| !apps.contains(app)) {
                self.gate.removed.fetch_add(1, Ordering::Relaxed);
            }
            // From here on the feeds in the mixer belong to exactly this list. (Not
            // for a worker that was let go: its session stays shut.)
            self.gate.applied.store(*revision, Ordering::Release);
        });
        self.shares_synced.clone_from(apps);
    }

    /// Returns the shared apps with nothing to capture right now.
    fn capture_shares(&mut self, shares: &[String], events: &Sender<Command>) -> Result<Vec<String>> {
        if self.capture.is_none() {
            let (events, pending) = (events.clone(), self.shares_changed.clone());
            let changed = move || {
                if !pending.swap(true, Ordering::Relaxed) {
                    let _ = events.send(Command::SharesChanged);
                }
            };
            self.capture = Some(AppCapture::open(self.rate, changed)?);
        }
        match self.capture.as_mut() {
            Some(capture) => capture.sync(shares, &mut self.mixer),
            None => Ok(Vec::new()),
        }
    }

    fn set_effects(&mut self, effects: &[Effect]) {
        self.drop_retired();
        self.unsent = Some(Box::new(Chain::new(effects, self.rate as f32)));
        self.send_unsent();
    }

    /// Queue the unsent chain for the input callback; it stays unsent while the queue is full.
    fn send_unsent(&mut self) {
        if let Some(chain) = self.unsent.take()
            && let Err(rtrb::PushError::Full(chain)) = self.chains.push(chain)
        {
            tracing::debug!("voice: preset swap queue full, retrying");
            self.unsent = Some(chain);
        }
    }

    /// Chains, rings and feeds the callback no longer uses are freed here, never on the audio thread.
    fn drop_retired(&mut self) {
        while self.retired.pop().is_ok() {}
        while self.retired_rings.pop().is_ok() {}
        self.mixer.drop_retired();
    }

    /// Whether the audio thread should poll: for a stalled microphone, a change of
    /// the default microphone, to bring back a wanted monitor that is not running,
    /// or to look after the app capture while apps are shared (reopen a failed
    /// one, notice a tap or record stream that died without an event).
    fn needs_poll(&self, wanted: &Wanted) -> bool {
        self.watchdog.is_some()
            || self.default_input.is_some()
            || (POLL_MONITOR && wanted.selection.monitor.is_some() && self.monitor.is_none())
            || !wanted.shares.apps.is_empty()
    }

    /// No input callback since the previous poll.
    fn input_stalled(&mut self) -> bool {
        let Some((callbacks, seen)) = self.watchdog.as_mut() else { return false };
        let now = callbacks.load(Ordering::Relaxed);
        std::mem::replace(seen, now) == now
    }

    /// Whether the default microphone changed and any failed switch can be retried.
    fn default_input_moved(&self) -> bool {
        let Some(current) = self.default_input.as_ref() else { return false };
        let Ok(desired) = devices::find(None, Direction::Input) else { return false };
        current.should_switch(&devices::id(&desired), Instant::now())
    }

    fn retry_monitor(&mut self, selection: &Selection, events: &Sender<Command>, generation: &mut u64) {
        if POLL_MONITOR && self.monitor.is_none() {
            let result = self.attach_monitor(selection, events, generation);
            self.warning = result.err().map(|e| format!("monitor unavailable: {e}"));
        }
    }

    /// Start the monitor `selection` asks for, unless it already runs. No monitor
    /// when it is off or would be the main output itself.
    fn attach_monitor(&mut self, selection: &Selection, events: &Sender<Command>, generation: &mut u64) -> Result<()> {
        let (None, Some(id)) = (&self.monitor, &selection.monitor) else {
            return Ok(());
        };
        let device = devices::find(id.as_deref(), Direction::Output)?;
        if devices::id(&device) == self.output_id {
            return Ok(());
        }

        let cfg = pick_config(&device, Direction::Output, self.rate)?;
        let mut start = |fixed| {
            *generation += 1;
            let (tx, rx) = ring(self.rate);
            let reader = Reader::new(rx, self.rate, cfg.sample_rate(), self.in_block.clone());
            let on_error = stream_error_handler(events.clone(), *generation, "monitor");
            let stream = output_stream(&device, cfg, fixed, reader, on_error)?;
            stream.play().map_err(backend)?;
            Ok::<_, VoiceError>((Monitor { _stream: stream, generation: *generation }, tx))
        };
        let (monitor, ring) = start(true).or_else(|_| start(false))?;
        self.drop_retired();
        if self.monitor_rings.push(ring).is_err() {
            return Err(VoiceError::Backend("the microphone stream stalled".into()));
        }
        self.monitor = Some(monitor);
        Ok(())
    }
}

/// Ring from the input callback to one output, see [`RING_CAPACITY_SECS`].
fn ring(rate: u32) -> (rtrb::Producer<f32>, rtrb::Consumer<f32>) {
    RingBuffer::new((f64::from(rate) * RING_CAPACITY_SECS) as usize)
}

/// `stream` names the endpoint in messages ("microphone", "output", "monitor").
fn stream_error_handler(
    events: Sender<Command>,
    generation: u64,
    stream: &'static str,
) -> impl FnMut(cpal::Error) + Send + 'static {
    move |err| match err.kind() {
        // Recoverable or informational: the stream keeps running.
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied => {
            tracing::debug!("voice {stream}: {err}");
        }
        _ => {
            let _ = events.send(Command::StreamError { generation, stream, message: err.to_string() });
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

    /// The PulseAudio capture has no error callback: a killed stream (say, the
    /// microphone was unplugged) or a lost connection just goes quiet.
    fn reports_errors(&self) -> bool {
        matches!(self, Self::Cpal(_))
    }
}

/// Everything the input callback owns.
pub(crate) struct InputSide {
    processor: Processor,
    output: rtrb::Producer<f32>,
    monitor: Option<rtrb::Producer<f32>>,
    /// Monitor rings from the audio thread, and the replaced ones going back to be freed.
    monitor_rings: rtrb::Consumer<rtrb::Producer<f32>>,
    retired_rings: rtrb::Producer<rtrb::Producer<f32>>,
    /// Shared app sound, added for the main output only; how far the session's
    /// capture is, and the count of [`ShareGate::removed`] the mixer is up to date with.
    mixer: Mixer,
    gate: Arc<ShareGate>,
    removed: u32,
    in_block: Arc<AtomicUsize>,
    callbacks: Arc<AtomicU32>,
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
        while let Ok(ring) = self.monitor_rings.pop() {
            if let Some(old) = self.monitor.replace(ring) {
                // Only fails if the audio thread stopped draining; then dropping here is the lesser evil.
                let _ = self.retired_rings.push(old);
            }
        }
        self.processor.set_params(self.shared.params.load());
        self.in_block.store(frames, Ordering::Relaxed);
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        // New feeds come in before the look at the gate: it then holds for all of them.
        self.mixer.admit();
        // Shut unless the capture is lined up with the list of apps shared right now.
        // The applied list is read first and written last: whoever sees it also
        // sees the removals that came with it.
        let applied = self.gate.applied.load(Ordering::Acquire);
        let removed = self.gate.removed.load(Ordering::Relaxed);
        if std::mem::replace(&mut self.removed, removed) != removed {
            self.mixer.flush();
        }
        let committed = self.shared.shares_committed.load(Ordering::Relaxed);
        let open = committed != 0 && committed == applied;
        let share_gain = if open { f32::from_bits(self.shared.share_gain.load(Ordering::Relaxed)) } else { 0.0 };
        let sharing = self.mixer.begin(Instant::now(), frames, share_gain);
        let mut left = frames;
        while left > 0 {
            let n = left.min(self.scratch.len());
            let buf = &mut self.scratch[..n];
            for (out, s) in buf.iter_mut().zip(&mut samples) {
                *out = s;
            }
            self.processor.process(buf);
            // Full ring = the output stalled (or a lost monitor); dropping is the only realtime-safe option.
            if let Some(monitor) = self.monitor.as_mut() {
                let _ = monitor.push_partial_slice(buf);
            }
            if sharing {
                self.mixer.add(buf);
            }
            let _ = self.output.push_partial_slice(buf);
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

/// Pick a config for `device`, see [`choose_config`].
fn pick_config(device: &cpal::Device, dir: Direction, want_rate: u32) -> Result<SupportedStreamConfig> {
    let (default, ranges) = match dir {
        Direction::Input => (device.default_input_config(), device.supported_input_configs().map(|r| r.collect())),
        Direction::Output => (device.default_output_config(), device.supported_output_configs().map(|r| r.collect())),
    };
    let ranges: Vec<cpal::SupportedStreamConfigRange> = ranges.unwrap_or_default();
    if let Some(cfg) = choose_config(default.as_ref().ok(), &ranges, want_rate) {
        return Ok(cfg);
    }
    match default {
        Ok(default) if format_rank(default.sample_format()).is_some() => Ok(default),
        Ok(_) => Err(VoiceError::Unsupported(format!("{} has no usable sample format", devices::name(device)))),
        Err(e) => Err(backend(e)),
    }
}

/// A supported config at `want_rate` (else 48 kHz, else the default rate, else any
/// supported rate), preferring f32 and the default channel count. The default
/// config is only a preference: cpal's macOS backend fails it for some formats
/// (e.g. 24-bit) that the supported configs still cover.
fn choose_config(
    default: Option<&SupportedStreamConfig>,
    ranges: &[cpal::SupportedStreamConfigRange],
    want_rate: u32,
) -> Option<SupportedStreamConfig> {
    let default_channels = default.map(SupportedStreamConfig::channels);
    [Some(want_rate), Some(PREFERRED_RATE), default.map(SupportedStreamConfig::sample_rate)]
        .into_iter()
        .flatten()
        .chain(ranges.iter().map(cpal::SupportedStreamConfigRange::max_sample_rate))
        .find_map(|rate| {
            ranges
                .iter()
                .filter(|r| r.contains_rate(rate) && r.channels() > 0)
                .filter_map(|r| format_rank(r.sample_format()).map(|rank| (rank, r)))
                .min_by_key(|(rank, r)| (*rank, Some(r.channels()) != default_channels, r.channels()))
                .map(|(_, range)| range.with_sample_rate(rate))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_default_input_switch_retries_after_backoff() {
        let now = Instant::now();
        let mut input = DefaultInput { id: "old-mic".into(), retry_at: None };
        assert!(input.should_switch("new-mic", now));

        input.switch_failed(now);
        assert_eq!(input.id, "old-mic");
        assert!(!input.should_switch("new-mic", now));
        assert!(!input.should_switch("new-mic", now + RETRY_INTERVAL - Duration::from_millis(1)));
        assert!(input.should_switch("new-mic", now + RETRY_INTERVAL));
        assert!(!input.should_switch("old-mic", now + RETRY_INTERVAL));
    }

    #[test]
    fn cancelled_worker_cannot_publish_after_disable_or_replacement() {
        let shared = Arc::new(Shared::default());
        // An idle worker opens no devices; keep its publication token after dropping it.
        let worker = Worker::spawn(shared.clone()).unwrap();
        let old = worker.state.clone();
        publish(&old, Ok(None)).unwrap();
        drop(worker);
        *shared.status() = VoiceStatus::default();

        // A stalled open finishes after disable cleared the status.
        old.update(|shared| {
            shared.status().virtual_mic = Some("old cable".into());
            shared.set_has_cable(false);
        });
        publish(&old, Ok(Some("old warning".into()))).unwrap();
        assert_eq!(*shared.status(), VoiceStatus::default());
        assert_eq!(*shared.has_cable.lock().unwrap(), None);

        let current = WorkerState::new(shared.clone());
        current.update(|shared| shared.status().virtual_mic = Some("new cable".into()));
        publish(&current, Ok(Some("new warning".into()))).unwrap();
        let expected = shared.status().clone();
        publish(&old, Err(VoiceError::Backend("late failure".into()))).unwrap_err();
        publish(&old, Ok(None)).unwrap();
        old.update(|shared| shared.status().error = None);
        assert_eq!(*shared.status(), expected);
    }

    #[test]
    fn config_choice_does_not_need_the_default() {
        // cpal's macOS backend fails the default config of e.g. 24-bit devices that still list f32.
        let range =
            |format, min, max| cpal::SupportedStreamConfigRange::new(2, min, max, SupportedBufferSize::Unknown, format);
        let ranges = [range(SampleFormat::I24, 44_100, 96_000), range(SampleFormat::F32, 44_100, 96_000)];
        let cfg = choose_config(None, &ranges, 48_000).unwrap();
        assert_eq!((cfg.sample_format(), cfg.sample_rate()), (SampleFormat::F32, 48_000));
        // Neither the wanted rate nor 48 kHz: any supported rate beats failing.
        let cfg = choose_config(None, &[range(SampleFormat::I16, 44_100, 44_100)], 48_000).unwrap();
        assert_eq!((cfg.sample_format(), cfg.sample_rate()), (SampleFormat::I16, 44_100));
    }

    /// The call hears the shared app next to the voice; the monitor (the user's own
    /// ears) gets the voice only, since the app already plays there.
    #[test]
    fn shared_app_sound_reaches_the_output_but_not_the_monitor() {
        let shared = Arc::new(Shared::default());
        shared.share_gain.store(1.0f32.to_bits(), Ordering::Relaxed);
        shared.shares_committed.store(1, Ordering::Relaxed);
        let gate = Arc::<ShareGate>::default();
        gate.applied.store(1, Ordering::Relaxed);
        let (output, mut output_rx) = ring(48_000);
        let (monitor, mut monitor_rx) = ring(48_000);
        let (_monitor_rings, monitor_in) = RingBuffer::new(2);
        let (retired_rings, _retired) = RingBuffer::new(2);
        let (mixer, mut port) = share::mixer(48_000);
        let mut side = InputSide {
            processor: Processor::new(48_000.0, Chain::new(&[], 48_000.0)),
            output,
            monitor: Some(monitor),
            monitor_rings: monitor_in,
            retired_rings,
            mixer,
            gate,
            removed: 0,
            in_block: Arc::default(),
            callbacks: Arc::default(),
            shared,
            scratch: vec![0.0; 1024].into_boxed_slice(),
        };
        let (mut app, feed) = share::feed(48_000, 48_000);
        assert!(port.add(feed));

        // A 0.1 "voice" and a 0.5 "app", in callbacks larger than the scratch buffer.
        for _ in 0..20 {
            app.write(2048, std::iter::repeat_n(0.5, 2048));
            side.process_mono(2048, std::iter::repeat_n(0.1, 2048));
            // Callbacks any closer together would count as one delivery in pieces.
            std::thread::sleep(Duration::from_millis(12));
        }
        let drain = |rx: &mut rtrb::Consumer<f32>| std::iter::from_fn(|| rx.pop().ok()).collect::<Vec<_>>();
        let (to_call, to_ears) = (drain(&mut output_rx), drain(&mut monitor_rx));
        assert_eq!(to_call.len(), 20 * 2048);
        assert!(to_call[10 * 2048..].iter().all(|s| (s - 0.6).abs() < 1e-3), "voice + app: {}", to_call[10 * 2048]);
        assert!(to_ears[10 * 2048..].iter().all(|s| (s - 0.1).abs() < 1e-3), "voice only: {}", to_ears[10 * 2048]);

        // The list changes: the app is shut out within one callback, whatever its
        // feed and the capture still do.
        side.shared.shares_committed.store(2, Ordering::Relaxed);
        for _ in 0..2 {
            app.write(2048, std::iter::repeat_n(0.5, 2048));
            side.process_mono(2048, std::iter::repeat_n(0.1, 2048));
            std::thread::sleep(Duration::from_millis(12));
        }
        let to_call = drain(&mut output_rx);
        assert!(to_call[2048..].iter().all(|s| (s - 0.1).abs() < 1e-3), "voice only once shut: {}", to_call[2048]);
    }

    /// Real-device smoke test of the output path (ring -> Reader -> cpal), no microphone needed.
    /// OMNI_VOICE_TEST_OUTPUT=<device id or name> picks the device, else the first virtual one.
    #[test]
    #[ignore = "plays a quiet tone on a real output device"]
    fn live_output_stream_consumes_in_real_time() {
        let device = match std::env::var("OMNI_VOICE_TEST_OUTPUT") {
            Ok(id) => devices::find(Some(&id), Direction::Output).unwrap(),
            Err(_) => Direction::Output
                .list(&devices::host())
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
        let stream = output_stream(&device, cfg, true, reader, stream_error_handler(events, 1, "output")).unwrap();
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
