//! Linux microphone capture straight from PulseAudio / PipeWire (pipewire-pulse).
//!
//! cpal 0.18.2 caps PulseAudio record queues at one fragment when a buffer size is
//! requested, which drops most audio once the server pushes larger chunks
//! (RustAudio/cpal#1387), and its default is ~2 s fragments. Until the fix ships,
//! capture opens the record stream itself: 5 ms fragments, queue limit left to the
//! server, mono f32 at our rate (the server downmixes and resamples).
//! Playback still goes through cpal, which is unaffected.

use std::ffi::CString;
use std::sync::mpsc;
use std::time::Duration;

use futures::FutureExt;
use futures::executor::block_on;
use pulseaudio::protocol;

use crate::audio::InputSide;
use crate::{Result, VoiceError};

const FRAGMENT_SECS: f32 = 0.005;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const CREATE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct Capture {
    stream: pulseaudio::RecordStream,
    _client: pulseaudio::Client,
}

impl Capture {
    /// Start capturing `source` (a PulseAudio source name) into `side`.
    pub fn open(source: &str, rate: u32, mut side: InputSide) -> Result<Self> {
        let client = connect()?;
        let name = CString::new(source).map_err(|_| VoiceError::DeviceNotFound(source.into()))?;
        let fragment_frames = ((rate as f32 * FRAGMENT_SECS) as u32).max(1);
        let params = protocol::RecordStreamParams {
            sample_spec: protocol::SampleSpec {
                format: protocol::SampleFormat::Float32Le,
                channels: 1,
                sample_rate: rate,
            },
            channel_map: protocol::ChannelMap::mono(),
            source_name: Some(name),
            buffer_attr: protocol::stream::BufferAttr {
                fragment_size: fragment_frames * 4,
                ..Default::default() // max_length = u32::MAX: the server's maximum
            },
            flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
            ..Default::default()
        };
        let callback = move |bytes: &[u8]| {
            let samples = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            side.process_mono(bytes.len() / 4, samples);
        };
        let creator = client.clone();
        let stream = with_timeout(CREATE_TIMEOUT, move || block_on(creator.create_record_stream(params, callback)))?
            .map_err(|e| pulse_error(e, source))?;
        Ok(Self { stream, _client: client })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // Queue the delete without waiting on the server; the reactor drops the callback
        // on acknowledgement, and closing the connection ends the stream regardless.
        let _ = self.stream.clone().delete().now_or_never();
    }
}

fn connect() -> Result<pulseaudio::Client> {
    with_timeout(CONNECT_TIMEOUT, || pulseaudio::Client::from_env(c"OmniPanel"))?.map_err(|e| pulse_error(e, "server"))
}

/// Run `job` on a helper thread and wait at most `timeout`: the pulseaudio crate
/// waits for the server without one. A job that never returns leaks its thread;
/// one that returns late drops its result (a late stream is deleted on drop).
fn with_timeout<T: Send + 'static>(timeout: Duration, job: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("omni-voice-pulse".into())
        .spawn(move || {
            let _ = tx.send(job());
        })
        .map_err(|e| VoiceError::Backend(format!("cannot start PulseAudio helper thread: {e}")))?;
    rx.recv_timeout(timeout).map_err(|_| VoiceError::Backend("PulseAudio did not respond".into()))
}

fn pulse_error(err: pulseaudio::ClientError, what: &str) -> VoiceError {
    match err {
        pulseaudio::ClientError::ServerError(protocol::PulseError::NoEntity) => VoiceError::DeviceNotFound(what.into()),
        other => VoiceError::Backend(format!("PulseAudio: {other}")),
    }
}
