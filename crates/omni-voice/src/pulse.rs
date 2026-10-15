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
        let stream = block_on(client.create_record_stream(params, callback)).map_err(|e| pulse_error(e, source))?;
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

/// The handshake blocks without a timeout, so run it on a helper thread.
fn connect() -> Result<pulseaudio::Client> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(pulseaudio::Client::from_env(c"OmniPanel"));
    });
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result.map_err(|e| pulse_error(e, "server")),
        Err(_) => Err(VoiceError::Backend("PulseAudio did not respond".into())),
    }
}

fn pulse_error(err: pulseaudio::ClientError, what: &str) -> VoiceError {
    match err {
        pulseaudio::ClientError::ServerError(protocol::PulseError::NoEntity) => VoiceError::DeviceNotFound(what.into()),
        other => VoiceError::Backend(format!("PulseAudio: {other}")),
    }
}
