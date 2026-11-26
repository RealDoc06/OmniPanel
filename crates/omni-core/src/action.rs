//! Everything a deck button (or a global hotkey) can do.
//!
//! Leaf actions are executed by `omni-actions`. Flow-control actions
//! (`multi`, `toggle`, `delay`, `navigate`, `voice`) are interpreted by
//! `omni-host`, because they need per-device or per-control state.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum Action {
    /// Press a key combination, e.g. `"Ctrl+Shift+M"`, `"Cmd+Space"`, `"F13"`.
    /// Modifiers: Ctrl, Shift, Alt (Option), Cmd (Win/Super/Meta), CmdOrCtrl.
    Hotkey { keys: String },
    /// Type a string as if it was typed on the keyboard.
    TypeText { text: String },
    /// Launch an application. `app` is an app name ("Spotify"), a bundle or
    /// executable path, or a Linux desktop entry id.
    OpenApp {
        app: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Open a URL, file or folder with the system default handler.
    Open { target: String },
    /// Run a script with the chosen interpreter.
    RunScript {
        interpreter: Interpreter,
        script: String,
        /// Show the script output as a toast on the deck.
        #[serde(default)]
        show_output: bool,
        #[serde(default = "default_timeout")]
        timeout_secs: u32,
    },
    /// System-wide media key.
    Media { command: MediaCommand },
    /// Change a volume (system output, microphone, a single app or an OBS input).
    Volume { target: VolumeTarget, op: VolumeOp },
    /// Choose the device one app plays on or records from. Where the platform cannot
    /// do that (see `AudioCapabilities`), the action fails with the reason.
    AppDevice {
        /// Matched like `VolumeTarget::App`.
        app: String,
        direction: AudioDirection,
        /// `AudioDeviceInfo::id` or a device name; `None` is the system default.
        #[serde(default)]
        device: Option<String>,
    },
    /// Control the microphone voice changer.
    Voice { op: VoiceOp },
    /// Control OBS Studio through obs-websocket v5.
    Obs { op: ObsOp },
    /// Control Home Assistant through its REST API.
    HomeAssistant { op: HaOp },
    /// Send an HTTP request (webhooks, local APIs, ...).
    Http {
        method: HttpMethod,
        url: String,
        #[serde(default)]
        headers: Vec<HttpHeader>,
        #[serde(default)]
        body: Option<String>,
    },
    /// Power / session commands.
    System { command: SystemCommand },
    /// Change what the deck shows.
    Navigate { to: NavTarget },
    /// Run several actions in order.
    Multi {
        steps: Vec<Action>,
        /// Pause between steps.
        #[serde(default)]
        delay_ms: u32,
    },
    /// Alternate between two actions on each press. The control shows the
    /// active state after `on` ran.
    Toggle { on: Box<Action>, off: Box<Action> },
    /// Wait, only useful inside `multi`.
    Delay { ms: u32 },
}

fn default_timeout() -> u32 {
    30
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Interpreter {
    /// `/bin/sh -c` on macOS/Linux, `cmd /C` on Windows.
    Shell,
    Bash,
    #[serde(rename = "powershell")]
    PowerShell,
    Python,
    Node,
    /// macOS only, runs through `osascript`.
    #[serde(rename = "applescript")]
    AppleScript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
    Stop,
    VolumeUp,
    VolumeDown,
    Mute,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum VolumeTarget {
    /// Default output device.
    Master,
    /// Default input device.
    Mic,
    /// A single application, matched by audio session id or name (case-insensitive).
    App { app: String },
    /// An OBS input (requires the OBS integration).
    ObsInput { input: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum VolumeOp {
    /// Absolute volume, 0.0 - 1.0.
    Set {
        value: f32,
    },
    /// Relative change, e.g. 0.05 or -0.05.
    Step {
        delta: f32,
    },
    ToggleMute,
    Mute,
    Unmute,
}

/// Which side of an app's audio: what it plays or what it records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AudioDirection {
    Output,
    Input,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum VoiceOp {
    Toggle,
    Enable,
    Disable,
    /// Switch to a voice preset (built-in or custom id) and enable the voice changer.
    SetPreset {
        preset_id: String,
    },
    /// Cycle through the given presets (all presets when empty).
    Cycle {
        #[serde(default)]
        preset_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ObsOp {
    SetScene {
        scene: String,
    },
    ToggleRecord,
    StartRecord,
    StopRecord,
    PauseRecord,
    ToggleStream,
    StartStream,
    StopStream,
    ToggleReplayBuffer,
    SaveReplay,
    ToggleVirtualCam,
    ToggleMute {
        input: String,
    },
    /// Show / hide a source inside a scene.
    ToggleSource {
        scene: String,
        source: String,
    },
    ToggleStudioMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum HaOp {
    /// `homeassistant.toggle` on an entity, e.g. `light.desk`.
    Toggle { entity_id: String },
    /// Any service call, e.g. domain `scene`, service `turn_on`.
    CallService {
        domain: String,
        service: String,
        #[serde(default)]
        entity_id: Option<String>,
        /// Extra service data as a JSON object.
        #[serde(default)]
        data: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
#[ts(export)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SystemCommand {
    Lock,
    Sleep,
    DisplaySleep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum NavTarget {
    NextPage,
    PrevPage,
    Page {
        page_id: String,
    },
    /// Switch the device to another profile (first page).
    Profile {
        profile_id: String,
    },
}

impl Action {
    /// Visit this action and every nested action (multi steps, toggle branches).
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Action)) {
        f(self);
        match self {
            Action::Multi { steps, .. } => steps.iter().for_each(|s| s.walk(f)),
            Action::Toggle { on, off } => {
                on.walk(f);
                off.walk(f);
            }
            _ => {}
        }
    }
}
