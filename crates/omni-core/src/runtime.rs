//! Live (not persisted) state shared between the host and the desktop UI,
//! plus the data types returned by the audio / voice / integration subsystems.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{Config, Device, Theme, VoicePreset};

/// Snapshot returned by the `get_state` command.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppState {
    pub config: Config,
    pub runtime: RuntimeState,
    pub builtin_themes: Vec<Theme>,
    pub builtin_voice_presets: Vec<VoicePreset>,
    pub info: AppInfo,
    /// Recent activity, including startup warnings before the UI subscribed.
    pub logs: Vec<LogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppInfo {
    pub version: String,
    /// `macos` | `windows` | `linux`
    pub os: String,
    pub arch: String,
    /// Hostname used when `settings.hostName` is empty.
    pub machine_name: String,
    pub data_dir: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RuntimeState {
    pub server: ServerStatus,
    pub sessions: Vec<DeckSession>,
    pub pairing: Option<PairingInfo>,
    pub usb: UsbStatus,
    pub obs: ConnectionStatus,
    pub home_assistant: ConnectionStatus,
    pub voice: VoiceStatus,
    pub audio: AudioCapabilities,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerStatus {
    pub running: bool,
    pub port: u16,
    /// Deck URLs to open on the device, one per LAN interface.
    pub urls: Vec<String>,
    pub error: Option<String>,
}

/// A connected deck.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeckSession {
    pub device_id: String,
    pub transport: Transport,
    pub address: String,
    pub profile_id: String,
    pub page_id: String,
    #[ts(type = "number")]
    pub connected_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Transport {
    Wifi,
    /// Android over `adb reverse` (connection arrives on loopback).
    Usb,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PairingInfo {
    /// 6 digits.
    pub code: String,
    /// Unix milliseconds.
    #[ts(type = "number")]
    pub expires_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UsbStatus {
    pub enabled: bool,
    /// Resolved adb binary, `None` when not found.
    pub adb_path: Option<String>,
    pub devices: Vec<UsbDevice>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UsbDevice {
    pub serial: String,
    pub model: String,
    /// adb state: `device`, `unauthorized`, `offline`...
    pub state: String,
    /// `adb reverse` is active, the deck can open `http://localhost:<port>`.
    pub forwarded: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConnectionStatus {
    pub state: ConnState,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ConnState {
    #[default]
    Disabled,
    Connecting,
    Connected,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceStatus {
    pub running: bool,
    pub preset_id: String,
    pub error: Option<String>,
    /// Name of the virtual device apps should pick as microphone, when known.
    pub virtual_mic: Option<String>,
    /// Shown when no virtual cable is installed (install hint per OS).
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioCapabilities {
    pub per_app_volume: bool,
    /// Why per-app volume is limited/unavailable, or a permission hint.
    pub note: Option<String>,
    /// Apps can be sent to an output device of their own.
    pub per_app_output: bool,
    /// Why they cannot, a permission hint, or what such a choice means on this system.
    pub output_note: Option<String>,
    /// Apps can be given a microphone of their own.
    pub per_app_input: bool,
    /// Why they cannot, or what such a choice means on this system.
    pub input_note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VolumeState {
    /// 0.0 - 1.0
    pub volume: f32,
    pub muted: bool,
}

/// An application currently known to the audio system.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppAudio {
    /// Stable key used in `VolumeTarget::App` (bundle id, exe name, binary name).
    pub id: String,
    /// Display name.
    pub name: String,
    pub pid: Option<u32>,
    pub volume: f32,
    pub muted: bool,
    /// Currently producing sound.
    pub playing: bool,
    /// Where the app plays. `None` for an app that only records, and where
    /// `AudioCapabilities::per_app_output` is false.
    pub output: Option<AppRoute>,
    /// What the app records from, while it records. Always `None` where
    /// `AudioCapabilities::per_app_input` is false.
    pub input: Option<AppRoute>,
}

/// The device one side of an app's audio is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum AppRoute {
    /// It follows the system default device.
    Default,
    /// One device, by `AudioDeviceInfo::id`. Where OmniPanel keeps the choice itself
    /// (macOS) the device may not be connected right now.
    Device { id: String },
    /// Its streams are on different devices.
    Several,
}

/// A per-app level and output device the user set, persisted so they survive
/// restarts on platforms that don't remember them themselves (macOS).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppLevel {
    /// `AppAudio::id`
    pub app: String,
    pub volume: f32,
    pub muted: bool,
    /// Output device the app is sent to; `None` follows the system default.
    #[serde(default)]
    pub output: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MixerState {
    pub master: Option<VolumeState>,
    pub mic: Option<VolumeState>,
    pub apps: Vec<AppAudio>,
    pub capabilities: AudioCapabilities,
    /// Devices an app can be sent to (`Action::AppDevice`); empty where unsupported.
    pub devices: AudioDevices,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    /// Looks like a virtual cable (BlackHole, VB-CABLE, OmniPanel virtual mic...).
    pub is_virtual: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstalledApp {
    pub name: String,
    /// Value to put in `Action::OpenApp.app`.
    pub launch: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ObsInventory {
    pub scenes: Vec<String>,
    pub inputs: Vec<String>,
    /// scene name -> source names inside it
    pub scene_items: Vec<ObsSceneItems>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ObsSceneItems {
    pub scene: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HaEntity {
    pub entity_id: String,
    pub name: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ActionOutcome {
    /// Text to show (script output, HTTP status...).
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogEntry {
    #[ts(type = "number")]
    pub at: u64,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Pushed to the desktop UI on the `host-event` channel.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum HostEvent {
    Config { config: Config },
    Runtime { runtime: RuntimeState },
    Paired { device: Device },
    Log { entry: LogEntry },
}

/// Voice changer meter, peak levels 0.0 - 1.0 since the previous read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceLevels {
    pub input: f32,
    pub output: f32,
}
