//! Live button state: toggles, mute, app devices, voice changer, OBS status, slider values.
//!
//! The loop sleeps until something changes (an action ran, OBS sent an
//! event, a deck connected). Audio has no change events, so while a visible
//! control depends on a volume it is re-read once per second.

use std::collections::HashMap;
use std::time::Duration;

use omni_actions::obs::ObsState;
use omni_audio::{find_app, find_device};
use omni_core::{
    Action, AppAudio, AppRoute, AudioDevices, AudioDirection, Config, Control, ControlKind, ControlState, HostToDeck,
    NavTarget, ObsOp, VoiceOp, VoiceSettings, VolumeOp, VolumeState, VolumeTarget,
};

use crate::Host;
use crate::sessions::{profile_of, send};

const AUDIO_POLL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Default)]
pub struct AudioSnapshot {
    pub master: Option<VolumeState>,
    pub mic: Option<VolumeState>,
    pub apps: Vec<AppAudio>,
    /// Read only while a visible control chooses an app's device.
    pub devices: AudioDevices,
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
            // Lit while the app plays on (records from) that device, matched like the
            // action matches it. Not while the device is unplugged, even if the app is
            // kept for it, and never for the system default: that is no device to be on.
            Action::AppDevice { app, direction, device } => {
                let app = find_app(&self.audio.apps, app);
                let (route, devices) = match direction {
                    AudioDirection::Output => (app.and_then(|a| a.output.as_ref()), &self.audio.devices.outputs),
                    AudioDirection::Input => (app.and_then(|a| a.input.as_ref()), &self.audio.devices.inputs),
                };
                let wanted = device.as_deref().and_then(|d| find_device(devices, d).ok());
                (matches!((route, wanted), (Some(AppRoute::Device { id }), Some(wanted)) if *id == wanted.id), false)
            }
            Action::Voice { op } => match op {
                VoiceOp::SetPreset { preset_id } => (self.voice.enabled && self.voice.preset_id == *preset_id, false),
                VoiceOp::ToggleShareApp { app } | VoiceOp::ShareApp { app } | VoiceOp::UnshareApp { app } => {
                    (self.voice.shares(app), omni_voice::share_unsupported().is_some())
                }
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
            // An app that only records has no volume: its controls show as unavailable.
            VolumeTarget::App { app } => find_app(&self.audio.apps, app)
                .filter(|a| a.output.is_some() || a.input.is_none())
                .map(|a| VolumeState { volume: a.volume, muted: a.muted }),
            VolumeTarget::ObsInput { input } => {
                let volume = *self.obs.input_volume.get(input)?;
                Some(VolumeState { volume, muted: self.obs.input_muted.get(input).copied().unwrap_or(false) })
            }
        }
    }
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
                Action::AppDevice { device: Some(_), .. } => needed = true,
                _ => {}
            });
            needed
        }
        ControlKind::Button { action: None } => false,
    }
}

/// Does this control also need the device list (to tell which device its action names)?
fn needs_devices(control: &Control) -> bool {
    let mut needed = false;
    if let ControlKind::Button { action: Some(action) } = &control.kind {
        action.walk(&mut |a| needed |= matches!(a, Action::AppDevice { device: Some(_), .. }));
    }
    needed
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

    /// Whether any deck currently shows a control that `needs` something
    /// (`needs_audio`, `needs_devices`).
    fn decks_show(&self, needs: fn(&Control) -> bool) -> bool {
        let config = self.config();
        let sessions = self.sessions.lock().unwrap();
        sessions.by_id.values().any(|s| {
            profile_of(&config, &s.device_id)
                .and_then(|p| p.page(&s.page_id))
                .is_some_and(|page| page.controls.iter().any(needs))
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
        let (audio, devices) = (self.audio.clone(), self.decks_show(needs_devices));
        let snapshot = tokio::task::spawn_blocking(move || AudioSnapshot {
            master: audio.master().ok(),
            mic: audio.mic().ok(),
            apps: audio.apps().unwrap_or_default(),
            devices: if devices { audio.devices().unwrap_or_default() } else { AudioDevices::default() },
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
        let poll_audio = host.decks_show(needs_audio);
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
        if host.decks_show(needs_audio) {
            host.refresh_audio().await;
        }
        host.push_states();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_core::{AudioDeviceInfo, Style, new_id};

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
                    output: Some(AppRoute::Device { id: "bluez_output.1".into() }),
                    input: None,
                }],
                devices: AudioDevices {
                    outputs: vec![AudioDeviceInfo {
                        id: "bluez_output.1".into(),
                        name: "Headphones".into(),
                        is_default: false,
                        is_virtual: false,
                    }],
                    inputs: Vec::new(),
                },
            },
            obs: ObsState {
                connected: true,
                recording: true,
                current_scene: Some("Live".into()),
                ..ObsState::default()
            },
            voice: VoiceSettings {
                enabled: true,
                preset_id: "robot".into(),
                shared_apps: vec!["Spotify".into()],
                ..VoiceSettings::default()
            },
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

        let share = |app: &str| button(Action::Voice { op: VoiceOp::ToggleShareApp { app: app.into() } });
        assert!(cx.state_for(&share("spotify"), "p", "pg").active, "lit while the app is shared");
        assert!(!cx.state_for(&share("Discord"), "p", "pg").active);

        let missing_app =
            button(Action::Volume { target: VolumeTarget::App { app: "discord".into() }, op: VolumeOp::ToggleMute });
        assert!(cx.state_for(&missing_app, "p", "pg").disabled);
    }

    #[test]
    fn app_device_button_lights_while_the_app_is_on_it() {
        let to = |direction, device: Option<&str>| {
            button(Action::AppDevice { app: "spotify".into(), direction, device: device.map(str::to_string) })
        };
        let mut cx = cx();
        let lit = |cx: &StateContext, control: &Control| cx.state_for(control, "p", "pg").active;
        // By id as the editor saves it, or by name as an assistant or a person writes it.
        assert!(lit(&cx, &to(AudioDirection::Output, Some("bluez_output.1"))));
        assert!(lit(&cx, &to(AudioDirection::Output, Some("headphones"))));
        assert!(!lit(&cx, &to(AudioDirection::Output, Some("speakers"))));
        assert!(!lit(&cx, &to(AudioDirection::Output, None)));
        assert!(!lit(&cx, &to(AudioDirection::Input, Some("headphones"))), "it is not recording");
        // Never disabled: where it cannot work, pressing it says why.
        assert!(!cx.state_for(&to(AudioDirection::Input, None), "p", "pg").disabled);
        // Only a button that can light up keeps the audio readings coming.
        let headphones = to(AudioDirection::Output, Some("headphones"));
        assert!(needs_audio(&headphones) && needs_devices(&headphones));
        let default = to(AudioDirection::Output, None);
        assert!(!needs_audio(&default) && !needs_devices(&default));

        // Split across devices, the app is on none of them.
        cx.audio.apps[0].output = Some(AppRoute::Several);
        assert!(!lit(&cx, &headphones));
        // Unplugged: the app is kept for the headphones but does not play there now.
        cx.audio.apps[0].output = Some(AppRoute::Device { id: "bluez_output.1".into() });
        cx.audio.devices.outputs.clear();
        assert!(!lit(&cx, &to(AudioDirection::Output, Some("bluez_output.1"))));

        // An app that only records has a microphone, and no volume to show.
        cx.audio.apps[0].output = None;
        cx.audio.apps[0].input = Some(AppRoute::Default);
        let mute =
            button(Action::Volume { target: VolumeTarget::App { app: "spotify".into() }, op: VolumeOp::ToggleMute });
        assert!(cx.state_for(&mute, "p", "pg").disabled);
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
