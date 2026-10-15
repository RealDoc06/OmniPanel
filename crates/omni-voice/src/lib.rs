//! Real-time voice changer: microphone -> effect chain -> output device
//! (normally a virtual cable that other apps use as their microphone).
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

use std::sync::{Arc, Mutex, PoisonError};

pub use omni_core::{AudioDeviceInfo, AudioDevices, Effect, VoiceLevels, VoicePreset, VoiceSettings, VoiceStatus};

use audio::{Selection, Shared, Worker};

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

/// Thread-safe (`Send + Sync`) handle. Audio streams run on an internal thread.
pub struct VoiceEngine {
    state: Mutex<State>,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct State {
    worker: Option<Worker>,
    /// What the running worker was last configured with.
    applied: Option<(Selection, Vec<Effect>)>,
}

impl VoiceEngine {
    pub fn new() -> Self {
        Self { state: Mutex::default(), shared: Arc::default() }
    }

    /// Inputs and outputs; ids round-trip into `VoiceSettings` device fields.
    pub fn devices(&self) -> Result<AudioDevices> {
        let devices = devices::list()?;
        self.shared.set_has_cable(devices.outputs.iter().any(|d| devices::cable_rank(&d.name).is_some()));
        Ok(devices)
    }

    /// Start, stop or reconfigure to match `settings` (`settings.enabled`
    /// decides running or not). Swapping only the preset must not restart
    /// the audio streams. Calling it with unchanged values is a no-op.
    ///
    /// Gains and the noise gate apply instantly; a device change reopens the
    /// streams; after a device error, calling it again retries. With no
    /// `output_device` the first virtual cable is used (never the speakers,
    /// which would feed back into the microphone).
    pub fn apply(&self, settings: &VoiceSettings, preset: &VoicePreset) -> Result<()> {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut *guard;
        self.shared.status().preset_id.clone_from(&preset.id);

        if !settings.enabled {
            state.applied = None;
            drop(state.worker.take()); // closes the streams and joins the audio thread
            let mut status = self.shared.status();
            status.running = false;
            status.error = None;
            status.virtual_mic = None;
            self.shared.levels.take();
            return Ok(());
        }

        self.shared.params.store(settings.into());
        let selection = Selection {
            input: settings.input_device.clone(),
            output: settings.output_device.clone(),
            monitor: settings.monitor.then(|| settings.monitor_device.clone()),
        };
        let healthy = self.shared.status().running;
        if let (Some(worker), Some((current, effects))) = (state.worker.as_ref(), state.applied.as_mut())
            && healthy
            && *current == selection
        {
            if *effects != preset.effects {
                worker.set_effects(preset.effects.clone())?;
                effects.clone_from(&preset.effects);
            }
            return Ok(());
        }

        let worker = match state.worker.take() {
            Some(worker) => worker,
            None => Worker::spawn(self.shared.clone())?,
        };
        let result = worker.open(selection.clone(), preset.effects.clone());
        state.worker = Some(worker);
        state.applied = Some((selection, preset.effects.clone()));
        result
    }

    pub fn status(&self) -> VoiceStatus {
        let mut status = self.shared.status().clone();
        let has_cable = *self.shared.has_cable.lock().unwrap_or_else(PoisonError::into_inner);
        if has_cable == Some(false) {
            status.hint = Some(devices::install_hint().into());
        }
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
