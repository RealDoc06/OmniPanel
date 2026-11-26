//! Real-time voice changer: microphone -> effect chain -> output device
//! (normally a virtual cable that other apps use as their microphone). The
//! sound of chosen apps can be mixed into that output too ([`share`]), so a
//! call hears what the user hears.
//!
//! [`VoiceEngine`] is a cheap handle: no thread and no open device until it is
//! first enabled. While enabled, a dedicated audio thread owns the streams; DSP
//! runs in the input callback ([`dsp`]) and reaches each output through a
//! lock-free ring with drift-compensated resampling.

mod audio;
mod devices;
pub mod dsp;
#[cfg(target_os = "linux")]
mod pulse;
mod reader;
mod share;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

pub use omni_core::{AudioDeviceInfo, AudioDevices, Effect, VoiceLevels, VoicePreset, VoiceSettings, VoiceStatus};

use audio::{Selection, Shared, Wanted, Worker};
use share::Shares;

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("audio device not found: {0}")]
    DeviceNotFound(String),
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, VoiceError>;

/// Why app sound cannot be shared into the voice output on this system
/// (`None` = it can). Also in [`VoiceStatus::share_unsupported`].
pub fn share_unsupported() -> Option<&'static str> {
    share::unsupported()
}

/// Thread-safe (`Send + Sync`) handle. Audio streams run on an internal thread.
pub struct VoiceEngine {
    state: Mutex<State>,
    /// The shared apps as of [`Self::commit_shares`].
    committed: Mutex<Shares>,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct State {
    worker: Option<Worker>,
    /// What the running worker was last configured with.
    applied: Option<Wanted>,
}

impl VoiceEngine {
    pub fn new() -> Self {
        Self { state: Mutex::default(), committed: Mutex::default(), shared: Arc::default() }
    }

    /// Inputs and outputs; ids round-trip into `VoiceSettings` device fields.
    pub fn devices(&self) -> Result<AudioDevices> {
        let devices = devices::list()?;
        self.shared.set_has_cable(devices.outputs.iter().any(|d| devices::cable_rank(&d.name).is_some()));
        Ok(devices)
    }

    /// Start, stop or reconfigure to match `settings`. The streams run while
    /// the voice changer is enabled or an app is shared; with only apps shared
    /// the voice passes through unchanged (no effects, gains, noise gate or
    /// monitor: those belong to the voice changer) and the output must be a
    /// virtual device. The apps are heard once [`Self::commit_shares`] named
    /// the same list. Swapping the preset or the
    /// shared apps must not restart the audio streams. Calling it with
    /// unchanged values is a no-op.
    ///
    /// Gains and the noise gate apply instantly; a device change reopens the
    /// streams; after a device error, calling it again retries. With no
    /// `output_device` the first virtual cable is used (never the speakers,
    /// which would feed back into the microphone). A lost monitor never stops
    /// the voice. Except on Linux it also comes back by itself, and with no
    /// `input_device` the voice follows the system default microphone.
    pub fn apply(&self, settings: &VoiceSettings, preset: &VoicePreset) -> Result<()> {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut *guard;
        self.shared.status().preset_id.clone_from(&preset.id);

        // Nothing to capture apps with: their sound cannot keep the streams open.
        let shares =
            if share::unsupported().is_some() { Shares::default() } else { self.numbered(&settings.shared_apps) };
        self.shared.share_gain.store(dsp::share_gain(settings).to_bits(), Ordering::Relaxed);
        if !settings.enabled && shares.apps.is_empty() {
            state.applied = None;
            drop(state.worker.take()); // closes the streams and joins the audio thread (bounded wait)
            let mut status = self.shared.status();
            status.running = false;
            status.error = None;
            status.virtual_mic = None;
            status.share_error = None;
            status.share_waiting.clear();
            self.shared.levels.take();
            return Ok(());
        }

        self.shared.params.store(if settings.enabled { settings.into() } else { dsp::Params::default() });
        let wanted = Wanted {
            selection: Selection {
                input: settings.input_device.clone(),
                output: settings.output_device.clone(),
                monitor: (settings.enabled && settings.monitor).then(|| settings.monitor_device.clone()),
            },
            voice: settings.enabled,
            effects: if settings.enabled { preset.effects.clone() } else { Vec::new() },
            shares,
        };
        let (healthy, virtual_output) = {
            let status = self.shared.status();
            (status.running, status.virtual_mic.is_some())
        };
        if let (Some(worker), Some(applied)) = (state.worker.as_ref(), state.applied.as_mut())
            && healthy
            && applied.selection == wanted.selection
            // Turning the voice changer off leaves only the apps: on speakers that must stop the streams.
            && (applied.voice == wanted.voice || virtual_output)
        {
            applied.voice = wanted.voice;
            if applied.effects != wanted.effects {
                worker.set_effects(wanted.effects.clone())?;
                applied.effects = wanted.effects;
            }
            if applied.shares != wanted.shares {
                worker.set_shares(wanted.shares.clone())?;
                applied.shares = wanted.shares;
            }
            return Ok(());
        }

        let worker = match state.worker.take() {
            Some(worker) => worker,
            None => Worker::spawn(self.shared.clone())?,
        };
        let result = worker.open(wanted.clone());
        state.worker = Some(worker);
        state.applied = Some(wanted);
        result
    }

    /// Tell the engine which apps are shared as of now (the same list `apply` gets
    /// in `settings.shared_apps`). Call it the moment the list changes, before
    /// `apply`, which may be busy with devices: app sound only reaches the voice
    /// output once the capture is lined up with this very change. So "stopped"
    /// is heard within one audio callback, and nothing comes through of an app
    /// that was shared before, or shared and stopped again while the capture was
    /// still busy with it.
    pub fn commit_shares(&self, apps: &[String]) {
        let mut committed = self.committed.lock().unwrap_or_else(PoisonError::into_inner);
        if committed.apps != apps {
            committed.apps = apps.to_vec();
            committed.revision += 1;
            self.shared.shares_committed.store(committed.revision, Ordering::Relaxed);
        }
    }

    /// `apps` with the number of the change that committed them. A list that is
    /// not the committed one (any more, or yet) has none: it stays unheard.
    fn numbered(&self, apps: &[String]) -> Shares {
        let committed = self.committed.lock().unwrap_or_else(PoisonError::into_inner);
        Shares { apps: apps.to_vec(), revision: if committed.apps == apps { committed.revision } else { 0 } }
    }

    pub fn status(&self) -> VoiceStatus {
        let mut status = self.shared.status().clone();
        let has_cable = *self.shared.has_cable.lock().unwrap_or_else(PoisonError::into_inner);
        if has_cable == Some(false) {
            status.hint = Some(devices::install_hint().into());
        }
        status.share_unsupported = share_unsupported().map(Into::into);
        status
    }

    /// Peak levels since the previous call, for the UI meter.
    pub fn levels(&self) -> VoiceLevels {
        self.shared.levels.take()
    }

    /// Linux: create (if missing) a PipeWire/PulseAudio virtual microphone and
    /// return the output device id to send processed audio to.
    /// macOS / Windows: `Unsupported` with an install hint (BlackHole / VB-CABLE).
    pub fn create_virtual_mic(&self) -> Result<String> {
        #[cfg(target_os = "linux")]
        {
            let id = devices::linux::create_virtual_mic()?;
            self.shared.set_has_cable(true);
            Ok(id)
        }
        #[cfg(not(target_os = "linux"))]
        Err(VoiceError::Unsupported(devices::install_hint().into()))
    }
}

impl Default for VoiceEngine {
    fn default() -> Self {
        Self::new()
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<VoiceEngine>();
};

#[cfg(test)]
mod tests {
    use super::*;

    /// An app shared and stopped again while the capture is still busy with it must
    /// not be heard: going back to the list from before is a change of its own, and
    /// the mix stays shut until the capture caught up with it.
    #[test]
    fn going_back_to_an_earlier_list_of_shared_apps_is_a_new_change() {
        let engine = VoiceEngine::new();
        let commit = |apps: &[&str]| {
            engine.commit_shares(&apps.iter().map(|a| a.to_string()).collect::<Vec<_>>());
            engine.shared.shares_committed.load(Ordering::Relaxed)
        };
        let first = commit(&["Spotify"]);
        assert_ne!(first, 0);
        assert_eq!(commit(&["Spotify"]), first, "the same list again is no change");
        assert_ne!(commit(&["Spotify", "Firefox"]), first);
        let back = commit(&["Spotify"]);
        assert_ne!(back, first);
        assert_eq!(engine.numbered(&["Spotify".into()]).revision, back);
        assert_eq!(engine.numbered(&["Firefox".into()]).revision, 0, "not the committed list: never heard");
    }
}
