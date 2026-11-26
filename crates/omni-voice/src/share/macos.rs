//! macOS app capture: one unmuted process tap for all shared apps, provided by
//! `omni_audio::capture` (macOS 14.2+, needs the System Audio Recording
//! permission). The apps keep playing; the tap dies with our process.

use omni_audio::capture;

use super::{MixerPort, feed};
use crate::{Result, VoiceError};

/// Why apps cannot be shared on this macOS, if so.
pub(crate) fn unsupported() -> Option<&'static str> {
    capture::unsupported()
}

pub(crate) struct AppCapture {
    capture: capture::AppCapture,
    rate: u32,
}

impl AppCapture {
    /// `changed` runs on another thread whenever audio processes start or stop;
    /// answer it with [`Self::sync`].
    pub fn open(mix_rate: u32, changed: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        Ok(Self { capture: capture::AppCapture::new(changed), rate: mix_rate })
    }

    /// Capture exactly the apps in `apps` that run now; a new tap's feed goes to
    /// `port`. Returns the apps that are not running. Once `superseded` holds, no
    /// permission is asked for and no tap is made.
    pub fn sync(
        &mut self,
        apps: &[String],
        port: &mut MixerPort,
        superseded: &dyn Fn() -> bool,
    ) -> Result<Vec<String>> {
        let rate = self.rate;
        // The mixer takes feeds every microphone callback; a full queue means it stalled.
        let mut stalled = false;
        let mut open = |capture_rate| -> omni_audio::capture::Sink {
            let (mut writer, feed) = feed(capture_rate, rate);
            stalled |= !port.add(feed);
            Box::new(move |mono| writer.write(mono.len(), mono.iter().copied()))
        };
        let waiting = self.capture.sync(apps, &mut open, superseded);
        port.drop_retired();
        match waiting {
            Ok(_) if stalled => Err(VoiceError::Backend("the microphone stream stalled".into())),
            Ok(waiting) => Ok(waiting),
            Err(omni_audio::AudioError::Unsupported(reason)) => Err(VoiceError::Unsupported(reason)),
            Err(e) => Err(VoiceError::Backend(e.to_string())),
        }
    }
}
