//! Fallback for platforms without a backend: everything reports `Unsupported`.

use crate::{AppAudio, AudioCapabilities, AudioError, Backend, Endpoint, Result, VolumeState};

pub(crate) struct Platform;

fn unsupported<T>() -> Result<T> {
    Err(AudioError::Unsupported("audio control is not available on this platform".into()))
}

impl Backend for Platform {
    fn new() -> Self {
        Platform
    }

    fn capabilities(&self) -> AudioCapabilities {
        let note = "audio control is not available on this platform";
        AudioCapabilities {
            per_app_volume: false,
            note: Some(note.into()),
            per_app_output: false,
            output_note: Some(note.into()),
            per_app_input: false,
            input_note: Some(note.into()),
        }
    }

    fn endpoint(&self, _: Endpoint) -> Result<VolumeState> {
        unsupported()
    }

    fn set_endpoint_volume(&self, _: Endpoint, _: f32) -> Result<()> {
        unsupported()
    }

    fn set_endpoint_muted(&self, _: Endpoint, _: bool) -> Result<()> {
        unsupported()
    }

    fn apps(&self) -> Result<Vec<AppAudio>> {
        Ok(Vec::new())
    }

    fn set_app_volume(&self, _: &AppAudio, _: f32) -> Result<()> {
        unsupported()
    }

    fn set_app_muted(&self, _: &AppAudio, _: bool) -> Result<()> {
        unsupported()
    }
}
