//! `volume` actions: master / mic / per-app through `omni-audio`, OBS inputs through OBS.

use std::sync::Arc;

use omni_audio::AudioController;
use omni_core::{VolumeOp, VolumeState, VolumeTarget};
use serde_json::json;

use crate::obs::ObsClient;
use crate::{ActionError, Result, blocking};

pub(crate) async fn run(
    audio: &Arc<AudioController>,
    obs: &ObsClient,
    target: &VolumeTarget,
    op: &VolumeOp,
) -> Result<()> {
    match target {
        VolumeTarget::ObsInput { input } => obs_input(obs, input, op).await,
        VolumeTarget::Master | VolumeTarget::Mic | VolumeTarget::App { .. } => {
            let (audio, target, op) = (audio.clone(), target.clone(), op.clone());
            blocking(move || apply(&audio, &target, &op)).await
        }
    }
}

/// Raise / lower the system volume by `delta` (clamped), used by the media volume fallbacks.
pub(crate) fn step_master(audio: &AudioController, delta: f32) -> Result<()> {
    apply(audio, &VolumeTarget::Master, &VolumeOp::Step { delta })
}

pub(crate) fn toggle_master_mute(audio: &AudioController) -> Result<()> {
    apply(audio, &VolumeTarget::Master, &VolumeOp::ToggleMute)
}

fn read(audio: &AudioController, target: &VolumeTarget) -> Result<VolumeState> {
    Ok(match target {
        VolumeTarget::Master => audio.master()?,
        VolumeTarget::Mic => audio.mic()?,
        VolumeTarget::App { app } => audio.app(app)?,
        VolumeTarget::ObsInput { .. } => return Err(ActionError::Invalid("OBS inputs are not audio devices".into())),
    })
}

fn set_volume(audio: &AudioController, target: &VolumeTarget, volume: f32) -> Result<()> {
    match target {
        VolumeTarget::Master => audio.set_master_volume(volume)?,
        VolumeTarget::Mic => audio.set_mic_volume(volume)?,
        VolumeTarget::App { app } => audio.set_app_volume(app, volume)?,
        VolumeTarget::ObsInput { .. } => return Err(ActionError::Invalid("OBS inputs are not audio devices".into())),
    }
    Ok(())
}

fn set_muted(audio: &AudioController, target: &VolumeTarget, muted: bool) -> Result<()> {
    match target {
        VolumeTarget::Master => audio.set_master_muted(muted)?,
        VolumeTarget::Mic => audio.set_mic_muted(muted)?,
        VolumeTarget::App { app } => audio.set_app_muted(app, muted)?,
        VolumeTarget::ObsInput { .. } => return Err(ActionError::Invalid("OBS inputs are not audio devices".into())),
    }
    Ok(())
}

fn apply(audio: &AudioController, target: &VolumeTarget, op: &VolumeOp) -> Result<()> {
    match op {
        VolumeOp::Set { value } => set_volume(audio, target, clamp(*value)),
        VolumeOp::Step { delta } => {
            let current = read(audio, target)?;
            set_volume(audio, target, clamp(current.volume + delta))
        }
        VolumeOp::ToggleMute => {
            let current = read(audio, target)?;
            set_muted(audio, target, !current.muted)
        }
        VolumeOp::Mute => set_muted(audio, target, true),
        VolumeOp::Unmute => set_muted(audio, target, false),
    }
}

fn clamp(volume: f32) -> f32 {
    if volume.is_nan() { 0.0 } else { volume.clamp(0.0, 1.0) }
}

pub(crate) async fn obs_input(obs: &ObsClient, input: &str, op: &VolumeOp) -> Result<()> {
    match op {
        VolumeOp::Set { value } => set_obs_volume(obs, input, clamp(*value)).await,
        VolumeOp::Step { delta } => {
            let current = match obs.state().input_volume.get(input) {
                Some(volume) => *volume,
                None => {
                    let data = obs.request("GetInputVolume", json!({ "inputName": input })).await?;
                    data["inputVolumeMul"].as_f64().unwrap_or(1.0) as f32
                }
            };
            set_obs_volume(obs, input, clamp(current + delta)).await
        }
        VolumeOp::ToggleMute => obs.request("ToggleInputMute", json!({ "inputName": input })).await.map(drop),
        VolumeOp::Mute => set_obs_mute(obs, input, true).await,
        VolumeOp::Unmute => set_obs_mute(obs, input, false).await,
    }
}

async fn set_obs_volume(obs: &ObsClient, input: &str, volume: f32) -> Result<()> {
    obs.request("SetInputVolume", json!({ "inputName": input, "inputVolumeMul": volume })).await.map(drop)
}

async fn set_obs_mute(obs: &ObsClient, input: &str, muted: bool) -> Result<()> {
    obs.request("SetInputMute", json!({ "inputName": input, "inputMuted": muted })).await.map(drop)
}
