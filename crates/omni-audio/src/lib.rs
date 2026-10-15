//! Volume control: system output, default microphone and per-application volume.
//!
//! Backends:
//! - macOS: CoreAudio for output/mic, Core Audio process taps (macOS 14.2+) for per-app volume
//! - Windows: WASAPI (`IAudioEndpointVolume`, `ISimpleAudioVolume` per session)
//! - Linux: PulseAudio / PipeWire through `pactl`
//!
//! All methods are blocking but fast; call them from `spawn_blocking` in async code.

pub use omni_core::{AppAudio, AppLevel, AudioCapabilities, MixerState, VolumeState};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::Platform;

#[cfg(windows)]
mod wasapi;
#[cfg(windows)]
use wasapi::Platform;

// Compiled everywhere under `test` so the pactl parsers are tested on every host.
#[cfg(any(target_os = "linux", test))]
mod pulse;
#[cfg(target_os = "linux")]
use pulse::Platform;

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
mod unsupported;
#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
use unsupported::Platform;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("no audio app matches \"{0}\"")]
    AppNotFound(String),
    #[error("permission required: {0}")]
    Permission(String),
    #[error("{0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, AudioError>;

/// The two system endpoints: default output ("master") and default input ("mic").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Output,
    Input,
}

/// What every platform backend implements. `AudioController` adds clamping and app matching.
pub(crate) trait Backend: Send + Sync + Sized {
    fn new() -> Self;
    fn capabilities(&self) -> AudioCapabilities;
    fn endpoint(&self, endpoint: Endpoint) -> Result<VolumeState>;
    fn set_endpoint_volume(&self, endpoint: Endpoint, volume: f32) -> Result<()>;
    fn set_endpoint_muted(&self, endpoint: Endpoint, muted: bool) -> Result<()>;
    fn apps(&self) -> Result<Vec<AppAudio>>;
    /// The app `query` refers to (see [`find_app`]); backends with a costly listing
    /// may answer from a recent one.
    fn find(&self, query: &str) -> Result<AppAudio> {
        find_app(&self.apps()?, query).cloned().ok_or_else(|| AudioError::AppNotFound(query.into()))
    }
    /// `app` comes from `find`.
    fn set_app_volume(&self, app: &AppAudio, volume: f32) -> Result<()>;
    fn set_app_muted(&self, app: &AppAudio, muted: bool) -> Result<()>;
    /// Levels OmniPanel itself holds (macOS); systems that remember per-app volume have none.
    fn app_levels(&self) -> Vec<AppLevel> {
        Vec::new()
    }
    fn restore_app_levels(&self, _levels: &[AppLevel]) {}
}

/// Thread-safe (`Send + Sync`) handle to the platform audio system.
pub struct AudioController {
    platform: Platform,
}

impl AudioController {
    pub fn new() -> Self {
        Self { platform: Platform::new() }
    }

    pub fn capabilities(&self) -> AudioCapabilities {
        self.platform.capabilities()
    }

    pub fn master(&self) -> Result<VolumeState> {
        self.platform.endpoint(Endpoint::Output)
    }

    /// `volume` is clamped to 0.0 - 1.0.
    pub fn set_master_volume(&self, volume: f32) -> Result<()> {
        self.platform.set_endpoint_volume(Endpoint::Output, clamp_volume(volume)?)
    }

    pub fn set_master_muted(&self, muted: bool) -> Result<()> {
        self.platform.set_endpoint_muted(Endpoint::Output, muted)
    }

    pub fn mic(&self) -> Result<VolumeState> {
        self.platform.endpoint(Endpoint::Input)
    }

    /// `volume` is clamped to 0.0 - 1.0.
    pub fn set_mic_volume(&self, volume: f32) -> Result<()> {
        self.platform.set_endpoint_volume(Endpoint::Input, clamp_volume(volume)?)
    }

    pub fn set_mic_muted(&self, muted: bool) -> Result<()> {
        self.platform.set_endpoint_muted(Endpoint::Input, muted)
    }

    /// Applications known to the audio system (playing now or recently).
    pub fn apps(&self) -> Result<Vec<AppAudio>> {
        self.platform.apps()
    }

    /// `app` matches [`AppAudio::id`] (case-insensitive) or, failing that, a
    /// case-insensitive substring of [`AppAudio::name`].
    pub fn app(&self, app: &str) -> Result<VolumeState> {
        let found = self.platform.find(app)?;
        Ok(VolumeState { volume: found.volume, muted: found.muted })
    }

    /// `volume` is clamped to 0.0 - 1.0. See [`Self::app`] for matching.
    pub fn set_app_volume(&self, app: &str, volume: f32) -> Result<()> {
        let volume = clamp_volume(volume)?;
        self.platform.set_app_volume(&self.platform.find(app)?, volume)
    }

    /// See [`Self::app`] for matching.
    pub fn set_app_muted(&self, app: &str, muted: bool) -> Result<()> {
        self.platform.set_app_muted(&self.platform.find(app)?, muted)
    }

    /// App levels that differ from 100% / unmuted, to persist. Only macOS keeps
    /// per-app levels itself; Windows and Linux remember them and return none.
    pub fn app_levels(&self) -> Vec<AppLevel> {
        self.platform.app_levels()
    }

    /// Registers persisted levels at startup. Apps that are not running get theirs
    /// when they appear. Returns at once and never shows a permission prompt: on
    /// macOS the levels stay pending until System Audio Recording is allowed.
    pub fn restore_app_levels(&self, levels: &[AppLevel]) {
        self.platform.restore_app_levels(levels);
    }

    /// Everything the mixer UI shows; failures become `None` / empty.
    pub fn mixer(&self) -> MixerState {
        MixerState {
            master: self.master().ok(),
            mic: self.mic().ok(),
            apps: self.apps().unwrap_or_default(),
            capabilities: self.capabilities(),
        }
    }
}

// The host shares one controller across threads.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AudioController>()
};

impl Default for AudioController {
    fn default() -> Self {
        Self::new()
    }
}

fn clamp_volume(volume: f32) -> Result<f32> {
    if volume.is_nan() {
        return Err(AudioError::Backend("volume is not a number".into()));
    }
    Ok(volume.clamp(0.0, 1.0))
}

/// Matching rule shared by all backends: id (case-insensitive), then exact name,
/// then name substring. Earlier entries win ties.
fn find_app<'a>(apps: &'a [AppAudio], query: &str) -> Option<&'a AppAudio> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let needle = query.to_lowercase();
    apps.iter()
        .find(|a| a.id.to_lowercase() == needle)
        .or_else(|| apps.iter().find(|a| a.name.to_lowercase() == needle))
        .or_else(|| apps.iter().find(|a| a.name.to_lowercase().contains(&needle)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> AppAudio {
        AppAudio { id: id.into(), name: name.into(), pid: None, volume: 1.0, muted: false, playing: false }
    }

    #[test]
    fn matching_prefers_id_then_exact_name_then_substring() {
        let apps = [
            app("com.hnc.Discord.PTB", "Discord PTB"),
            app("com.spotify.client", "Spotify"),
            app("com.hnc.Discord", "Discord"),
            app("firefox", "Firefox Nightly"),
        ];
        let id = |q: &str| find_app(&apps, q).map(|a| a.id.as_str());

        assert_eq!(id("COM.SPOTIFY.CLIENT"), Some("com.spotify.client"));
        assert_eq!(id("spotify"), Some("com.spotify.client"));
        // Exact name beats an earlier substring hit.
        assert_eq!(id("discord"), Some("com.hnc.Discord"));
        assert_eq!(id("ptb"), Some("com.hnc.Discord.PTB"));
        assert_eq!(id("  Firefox "), Some("firefox"));
        assert_eq!(id("nightly"), Some("firefox"));
        assert_eq!(id("chrome"), None);
        assert_eq!(id(""), None);
    }

    #[test]
    fn volumes_are_clamped() {
        assert_eq!(clamp_volume(1.5).unwrap(), 1.0);
        assert_eq!(clamp_volume(-0.2).unwrap(), 0.0);
        assert!(clamp_volume(f32::NAN).is_err());
    }
}
