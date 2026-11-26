//! Volume control: system output, default microphone and per-application volume,
//! plus the device each application plays on and records from.
//!
//! Backends:
//! - macOS: CoreAudio for output/mic, Core Audio process taps (macOS 14.2+) for per-app
//!   volume and output device. Another app's microphone cannot be changed.
//! - Windows: WASAPI (`IAudioEndpointVolume`, `ISimpleAudioVolume` per session). No
//!   per-app devices yet.
//! - Linux: PulseAudio / PipeWire through `pactl`
//!
//! All methods are blocking but fast; call them from `spawn_blocking` in async code.

pub use omni_core::{
    AppAudio, AppLevel, AudioCapabilities, AudioDeviceInfo, AudioDevices, AudioDirection, MixerState, VolumeState,
};

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
    #[error("no audio device matches \"{0}\"")]
    DeviceNotFound(String),
    #[error("permission required: {0}")]
    Permission(String),
    #[error("{0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, AudioError>;

/// Output or input side: the default output ("master") and default input ("mic")
/// endpoints, or the device an app plays on / records from.
pub(crate) use omni_core::AudioDirection as Endpoint;

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
    /// Devices apps can be sent to; none where `set_app_device` is unsupported.
    fn devices(&self) -> Result<AudioDevices> {
        Ok(AudioDevices::default())
    }
    /// Sends what the app plays to an output device, or gives it a microphone.
    /// `device` matches like [`find_device`]; `None` is the system default. Takes the
    /// app query instead of a found app: a platform that cannot do this says so
    /// (with `capabilities().device_note`) whether or not the app is running.
    fn set_app_device(&self, _app: &str, _direction: Endpoint, _device: Option<&str>) -> Result<()> {
        Err(AudioError::Unsupported(self.capabilities().device_note.unwrap_or_default()))
    }
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

    /// Devices an app can be sent to, per direction. Empty where the platform cannot
    /// do that (see [`AudioCapabilities`]).
    pub fn devices(&self) -> Result<AudioDevices> {
        self.platform.devices()
    }

    /// Sends what `app` plays to an output device, or gives it another microphone.
    /// `device` is an [`AudioDeviceInfo::id`] or a device name; `None` is the system
    /// default. See [`Self::app`] for app matching.
    pub fn set_app_device(&self, app: &str, direction: AudioDirection, device: Option<&str>) -> Result<()> {
        self.platform.set_app_device(app, direction, device)
    }

    /// App levels and output devices that differ from 100% / unmuted / default, to
    /// persist. Only macOS keeps them itself; Windows and Linux remember them and
    /// return none.
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
            devices: self.devices().unwrap_or_default(),
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

/// Matching rule shared by all backends, for apps and devices: id (case-insensitive),
/// then exact name, then name substring. Earlier entries win ties.
fn best_match<'a, T>(items: &'a [T], query: &str, id: impl Fn(&T) -> &str, name: impl Fn(&T) -> &str) -> Option<&'a T> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let needle = query.to_lowercase();
    items
        .iter()
        .find(|i| id(i).to_lowercase() == needle)
        .or_else(|| items.iter().find(|i| name(i).to_lowercase() == needle))
        .or_else(|| items.iter().find(|i| name(i).to_lowercase().contains(&needle)))
}

fn find_app<'a>(apps: &'a [AppAudio], query: &str) -> Option<&'a AppAudio> {
    best_match(apps, query, |a| &a.id, |a| &a.name)
}

// Backends without per-app devices never look one up.
#[cfg_attr(windows, allow(dead_code))]
fn find_device<'a>(devices: &'a [AudioDeviceInfo], query: &str) -> Result<&'a AudioDeviceInfo> {
    best_match(devices, query, |d| &d.id, |d| &d.name).ok_or_else(|| AudioError::DeviceNotFound(query.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> AppAudio {
        AppAudio {
            id: id.into(),
            name: name.into(),
            pid: None,
            volume: 1.0,
            muted: false,
            playing: false,
            output: None,
            input: None,
        }
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
    fn devices_match_like_apps() {
        let device = |id: &str, name: &str| AudioDeviceInfo {
            id: id.into(),
            name: name.into(),
            is_default: false,
            is_virtual: false,
        };
        let devices = [device("alsa_output.usb-0", "Scarlett Solo"), device("bluez_output.1", "WH-1000XM4 Headphones")];
        let id = |q: &str| find_device(&devices, q).map(|d| d.id.as_str()).map_err(|e| e.to_string());

        assert_eq!(id("BLUEZ_OUTPUT.1"), Ok("bluez_output.1"));
        assert_eq!(id("scarlett solo"), Ok("alsa_output.usb-0"));
        assert_eq!(id("headphones"), Ok("bluez_output.1"));
        assert_eq!(id("HDMI"), Err("no audio device matches \"HDMI\"".into()));
    }

    #[test]
    fn volumes_are_clamped() {
        assert_eq!(clamp_volume(1.5).unwrap(), 1.0);
        assert_eq!(clamp_volume(-0.2).unwrap(), 0.0);
        assert!(clamp_volume(f32::NAN).is_err());
    }
}
