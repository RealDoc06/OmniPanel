//! No app capture on this OS yet: sharing reports why and does nothing.

use super::MixerPort;
use crate::{Result, VoiceError};

/// Windows needs per-process loopback capture (Windows 10 2004+), which is not
/// implemented yet.
const REASON: &str = if cfg!(windows) {
    "Sharing app sound into the microphone is not supported on Windows yet."
} else {
    "Sharing app sound into the microphone is not supported on this system yet."
};

/// Why apps cannot be shared here.
pub(crate) fn unsupported() -> Option<&'static str> {
    Some(REASON)
}

pub(crate) struct AppCapture;

impl AppCapture {
    pub fn open(_mix_rate: u32, _changed: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        Err(VoiceError::Unsupported(REASON.into()))
    }

    pub fn sync(&mut self, _apps: &[String], _port: &mut MixerPort) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}
