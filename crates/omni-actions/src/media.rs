//! `media` actions: play / pause, track skipping and volume keys.
//!
//! macOS and Windows get real media key events, so the OS shows its volume
//! overlay and the frontmost player reacts like it does to a keyboard. Linux
//! prefers `playerctl` (MPRIS) and controls volume through `omni-audio`.

use std::sync::Arc;

use enigo::Key;
use omni_audio::AudioController;
use omni_core::MediaCommand;

use crate::{Result, blocking, keys, volume};

/// Volume change of the `omni-audio` paths.
const VOLUME_STEP: f32 = 0.06;

pub(crate) async fn run(audio: &Arc<AudioController>, command: MediaCommand) -> Result<()> {
    #[cfg(not(any(target_os = "macos", windows)))]
    if let Some(playerctl) = crate::proc::find_in_path("playerctl")
        && let Some(action) = playerctl_action(command)
    {
        return playerctl_run(playerctl, action).await;
    }

    let audio = audio.clone();
    blocking(move || {
        // No OS volume overlay to trigger on Linux: go straight to the mixer.
        if cfg!(not(any(target_os = "macos", windows)))
            && let Some(result) = set_volume(&audio, command)
        {
            return result;
        }
        match media_key(command).and_then(keys::tap) {
            // Volume still works without keyboard access (e.g. missing macOS Accessibility permission).
            Err(error) => set_volume(&audio, command).unwrap_or(Err(error)),
            ok => ok,
        }
    })
    .await
}

/// Change the system volume through `omni-audio`; `None` for non-volume commands.
fn set_volume(audio: &AudioController, command: MediaCommand) -> Option<Result<()>> {
    match command {
        MediaCommand::VolumeUp => Some(volume::step_master(audio, VOLUME_STEP)),
        MediaCommand::VolumeDown => Some(volume::step_master(audio, -VOLUME_STEP)),
        MediaCommand::Mute => Some(volume::toggle_master_mute(audio)),
        MediaCommand::PlayPause | MediaCommand::Next | MediaCommand::Previous | MediaCommand::Stop => None,
    }
}

fn media_key(command: MediaCommand) -> Result<Key> {
    Ok(match command {
        MediaCommand::PlayPause => Key::MediaPlayPause,
        MediaCommand::Next => Key::MediaNextTrack,
        MediaCommand::Previous => Key::MediaPrevTrack,
        #[cfg(not(target_os = "macos"))]
        MediaCommand::Stop => Key::MediaStop,
        #[cfg(target_os = "macos")]
        MediaCommand::Stop => {
            return Err(crate::ActionError::Unsupported("macOS has no stop media key, use play/pause".into()));
        }
        MediaCommand::VolumeUp => Key::VolumeUp,
        MediaCommand::VolumeDown => Key::VolumeDown,
        MediaCommand::Mute => Key::VolumeMute,
    })
}

#[cfg(not(any(target_os = "macos", windows)))]
fn playerctl_action(command: MediaCommand) -> Option<&'static str> {
    match command {
        MediaCommand::PlayPause => Some("play-pause"),
        MediaCommand::Next => Some("next"),
        MediaCommand::Previous => Some("previous"),
        MediaCommand::Stop => Some("stop"),
        MediaCommand::VolumeUp | MediaCommand::VolumeDown | MediaCommand::Mute => None,
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
async fn playerctl_run(playerctl: std::path::PathBuf, action: &str) -> Result<()> {
    let mut cmd = tokio::process::Command::new(playerctl);
    cmd.arg(action);
    let out = crate::proc::capture(&mut cmd, std::time::Duration::from_secs(5)).await?;
    if out.status.success() {
        return Ok(());
    }
    let reason = out.stderr.trim();
    Err(crate::ActionError::Failed(if reason.is_empty() {
        "no media player is running".into()
    } else {
        reason.to_owned()
    }))
}
