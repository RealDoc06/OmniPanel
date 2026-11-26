//! Live button state: toggles, mute, app devices, voice changer, OBS status, slider values.
//!
//! The loop sleeps until something changes (an action ran, OBS sent an
//! event, a deck connected). Audio has no change events, so while a visible
//! control depends on a volume it is re-read once per second.

use std::collections::HashMap;
use std::time::Duration;

use omni_actions::obs::ObsState;
use omni_core::{
    Action, AppAudio, AudioDirection, Config, Control, ControlKind, ControlState, HostToDeck, NavTarget, ObsOp,
    VoiceOp, VoiceSettings, VolumeOp, VolumeState, VolumeTarget,
};

use crate::Host;
use crate::sessions::{profile_of, send};

const AUDIO_POLL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Default)]
pub struct AudioSnapshot {
    pub master: Option<VolumeState>,
    pub mic: Option<VolumeState>,
    pub apps: Vec<AppAudio>,
}

/// Everything needed to compute control states, captured once per pass.
pub struct StateContext {
    audio: AudioSnapshot,
    obs: ObsState,
    voice: VoiceSettings,
    toggles: HashMap<String, bool>,
}

impl StateContext {
    pub fn state_for(&self, control: &Control, profile_id: &str, page_id: &str) -> ControlState {
        let mut state = ControlState { id: control.id.clone(), ..ControlState::default() };
        match &control.kind {
            ControlKind::Button { action: Some(action) } => {
                (state.active, state.disabled) = self.action_state(action, &control.id, profile_id, page_id);
            }
            ControlKind::Button { action: None } => {}
            ControlKind::Slider { target, .. } => match self.volume(target) {
                Some(v) => {
                    state.value = Some(v.volume);
                    state.active = v.muted;
                    state.label = Some(format!("{}%", (v.volume * 100.0).round() as i32));
                }
                None => state.disabled = true,
            },
        }
        state
    }

    /// (active, disabled) for an action.
    fn action_state(&self, action: &Action, control_id: &str, profile_id: &str, page_id: &str) -> (bool, bool) {
        match action {
            Action::Toggle { .. } => (self.toggles.get(control_id).copied().unwrap_or(false), false),
            Action::Volume { target, op: VolumeOp::ToggleMute | VolumeOp::Mute | VolumeOp::Unmute } => {
                self.volume(target).map_or((false, true), |v| (v.muted, false))
            }
            // Lit while the app is on that device. Known by id only: a device given by
            // name, or the system default, runs but never lights up.
            Action::AppDevice { app, direction, device } => {
                let current = find_app(&self.audio.apps, app).and_then(|a| match direction {
                    AudioDirection::Output => a.output.as_deref(),
                    AudioDirection::Input => a.input.as_deref(),
                });
                (current.zip(device.as_deref()).is_some_and(|(on, wanted)| on.eq_ignore_ascii_case(wanted)), false)
            }
            Action::Voice { op } => match op {
                VoiceOp::SetPreset { preset_id } => (self.voice.enabled && self.voice.preset_id == *preset_id, false),
                _ => (self.voice.enabled, false),
            },
            Action::Obs { op } => {
                if !self.obs.connected {
                    return (false, true);
                }
                let obs = &self.obs;
                let active = match op {
                    ObsOp::SetScene { scene } => obs.current_scene.as_deref() == Some(scene),
                    ObsOp::ToggleRecord | ObsOp::StartRecord | ObsOp::StopRecord => obs.recording,
                    ObsOp::PauseRecord => obs.recording_paused,
                    ObsOp::ToggleStream | ObsOp::StartStream | ObsOp::StopStream => obs.streaming,
                    ObsOp::ToggleReplayBuffer | ObsOp::SaveReplay => obs.replay_buffer,
                    ObsOp::ToggleVirtualCam => obs.virtual_cam,
                    ObsOp::ToggleStudioMode => obs.studio_mode,
                    ObsOp::ToggleMute { input } => obs.input_muted.get(input).copied().unwrap_or(false),
                    ObsOp::ToggleSource { scene, source } => {
                        obs.source_visible.get(&(scene.clone(), source.clone())).copied().unwrap_or(false)
                    }
                };
                (active, false)
            }
            Action::Navigate { to: NavTarget::Page { page_id: target } } => (target == page_id, false),
            Action::Navigate { to: NavTarget::Profile { profile_id: target } } => (target == profile_id, false),
            // A multi-action looks like its first step (e.g. "record + scene" lights up while recording).
            Action::Multi { steps, .. } => {
                steps.first().map_or((false, false), |s| self.action_state(s, control_id, profile_id, page_id))
            }
            _ => (false, false),
        }
    }

    fn volume(&self, target: &VolumeTarget) -> Option<VolumeState> {
        match target {
            VolumeTarget::Master => self.audio.master,
            VolumeTarget::Mic => self.audio.mic,
            VolumeTarget::App { app } => {
                find_app(&self.audio.apps, app).map(|a| VolumeState { volume: a.volume, muted: a.muted })
            }
            VolumeTarget::ObsInput { input } => {
                let volume = *self.obs.input_volume.get(input)?;
                Some(VolumeState { volume, muted: self.obs.input_muted.get(input).copied().unwrap_or(false) })
            }
        }
    }
}

/// Same matching rule as omni-audio: id first, then name substring.
fn find_app<'a>(apps: &'a [AppAudio], query: &str) -> Option<&'a AppAudio> {
    let query = query.to_lowercase();
    apps.iter()
        .find(|a| a.id.to_lowercase() == query)
        .or_else(|| apps.iter().find(|a| a.name.to_lowercase().contains(&query)))
}

/// Does this control need fresh audio readings to show its state?
fn needs_audio(control: &Control) -> bool {
    let is_audio = |t: &VolumeTarget| matches!(t, VolumeTarget::Master | VolumeTarget::Mic | VolumeTarget::App { .. });
    match &control.kind {
        ControlKind::Slider { target, .. } => is_audio(target),
        ControlKind::Button { action: Some(action) } => {
            let mut needed = false;
            action.walk(&mut |a| match a {
                Action::Volume { target, .. } => needed |= is_audio(target),
                Action::AppDevice { .. } => needed = true,
                _ => {}
            });
            needed
        }
        ControlKind::Button { action: None } => false,
    }
}

impl Host {
    pub(crate) fn state_context(&self, config: &Config) -> StateContext {
        StateContext {
            audio: self.audio_snapshot.lock().unwrap().clone(),
            obs: self.executor.obs().state(),
            voice: config.settings.voice.clone(),
            toggles: self.toggles.lock().unwrap().clone(),
        }
    }

    /// Whether any deck currently shows a control that depends on audio levels.
    fn decks_need_audio(&self) -> bool {
        let config = self.config();
        let sessions = self.sessions.lock().unwrap();
        sessions.by_id.values().any(|s| {
            profile_of(&config, &s.device_id)
                .and_then(|p| p.page(&s.page_id))
                .is_some_and(|page| page.controls.iter().any(needs_audio))
        })
    }

    /// Recompute states for every deck and send only the ones that changed.
    fn push_states(&self) {
        let config = self.config();
        let cx = self.state_context(&config);
        let mut sessions = self.sessions.lock().unwrap();
        for session in sessions.by_id.values_mut() {
            let Some(profile) = profile_of(&config, &session.device_id) else { continue };
            let Some(page) = profile.page(&session.page_id) else { continue };
            let changed: Vec<ControlState> = page
                .controls
                .iter()
                .map(|c| cx.state_for(c, &profile.id, &page.id))
                .filter(|state| session.states.get(&state.id) != Some(state))
                .collect();
            if changed.is_empty() {
                continue;
            }
            for state in &changed {
                session.states.insert(state.id.clone(), state.clone());
            }
            send(&session.tx, &HostToDeck::State { states: changed });
        }
    }

    async fn refresh_audio(&self) {
        let audio = self.audio.clone();
        let snapshot = tokio::task::spawn_blocking(move || AudioSnapshot {
            master: audio.master().ok(),
            mic: audio.mic().ok(),
            apps: audio.apps().unwrap_or_default(),
        })
        .await;
        if let Ok(snapshot) = snapshot {
            *self.audio_snapshot.lock().unwrap() = snapshot;
        }
    }
}

pub async fn run(host: Host) {
    let mut obs = host.executor.obs().subscribe();
    loop {
        let poll_audio = host.decks_need_audio();
        let tick =
            async { if poll_audio { tokio::time::sleep(AUDIO_POLL).await } else { std::future::pending().await } };
        tokio::select! {
            _ = tick => {}
            _ = host.feedback.notified() => {
                // Let the action settle (e.g. the OS applying a mute) before reading back.
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
            changed = obs.changed() => if changed.is_err() { return },
        }
        if host.decks_need_audio() {
            host.refresh_audio().await;
        }
        host.push_states();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_core::{Style, new_id};

    fn button(action: Action) -> Control {
        Control {
            id: new_id(),
            x: 0,
            y: 0,
            w: 1,
            h: 1,
            style: Style::default(),
            kind: ControlKind::Button { action: Some(action) },
            hotkey: None,
        }
    }

    fn cx() -> StateContext {
        StateContext {
            audio: AudioSnapshot {
                master: Some(VolumeState { volume: 0.42, muted: false }),
                mic: Some(VolumeState { volume: 1.0, muted: true }),
                apps: vec![AppAudio {
                    id: "com.spotify.client".into(),
                    name: "Spotify".into(),
                    pid: Some(1),
                    volume: 0.3,
                    muted: false,
                    playing: true,
                    output: Some("headphones".into()),
                    input: None,
                }],
            },
            obs: ObsState {
                connected: true,
                recording: true,
                current_scene: Some("Live".into()),
                ..ObsState::default()
            },
            voice: VoiceSettings { enabled: true, preset_id: "robot".into(), ..VoiceSettings::default() },
            toggles: HashMap::new(),
        }
    }

    #[test]
    fn states_follow_their_sources() {
        let cx = cx();
        let mic = button(Action::Volume { target: VolumeTarget::Mic, op: VolumeOp::ToggleMute });
        assert!(cx.state_for(&mic, "p", "pg").active);

        let rec = button(Action::Obs { op: ObsOp::ToggleRecord });
        assert!(cx.state_for(&rec, "p", "pg").active);
        let scene = button(Action::Obs { op: ObsOp::SetScene { scene: "BRB".into() } });
        assert!(!cx.state_for(&scene, "p", "pg").active);

        let preset = button(Action::Voice { op: VoiceOp::SetPreset { preset_id: "robot".into() } });
        assert!(cx.state_for(&preset, "p", "pg").active);

        let missing_app =
            button(Action::Volume { target: VolumeTarget::App { app: "discord".into() }, op: VolumeOp::ToggleMute });
        assert!(cx.state_for(&missing_app, "p", "pg").disabled);
    }

    #[test]
    fn app_device_button_lights_while_the_app_is_on_it() {
        let to = |direction, device: Option<&str>| {
            button(Action::AppDevice { app: "spotify".into(), direction, device: device.map(str::to_string) })
        };
        let cx = cx();
        let lit = |control: &Control| cx.state_for(control, "p", "pg").active;
        assert!(lit(&to(AudioDirection::Output, Some("Headphones"))));
        assert!(!lit(&to(AudioDirection::Output, Some("speakers"))));
        assert!(!lit(&to(AudioDirection::Output, None)));
        assert!(!lit(&to(AudioDirection::Input, Some("headphones"))), "it is not recording");
        // Never disabled: where it cannot work, pressing it says why.
        assert!(!cx.state_for(&to(AudioDirection::Input, None), "p", "pg").disabled);
        assert!(needs_audio(&to(AudioDirection::Output, None)));
    }

    #[test]
    fn slider_shows_app_volume() {
        let slider = Control {
            kind: ControlKind::Slider { target: VolumeTarget::App { app: "spotify".into() }, vertical: true },
            ..button(Action::Delay { ms: 0 })
        };
        let state = cx().state_for(&slider, "p", "pg");
        assert_eq!(state.value, Some(0.3));
        assert_eq!(state.label.as_deref(), Some("30%"));
        assert!(needs_audio(&slider));
    }

    #[test]
    fn obs_controls_disabled_when_offline() {
        let mut cx = cx();
        cx.obs.connected = false;
        let rec = button(Action::Obs { op: ObsOp::ToggleRecord });
        assert!(cx.state_for(&rec, "p", "pg").disabled);
    }
}
