//! Persisted configuration: settings, profiles (deck layouts), paired devices,
//! user themes and user voice presets.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::action::{Action, VolumeTarget};

pub const CONFIG_SCHEMA: u32 = 1;
pub const MAX_GRID: u8 = 12;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Config {
    pub schema: u32,
    pub settings: Settings,
    pub profiles: Vec<Profile>,
    pub devices: Vec<Device>,
    /// User themes. Built-in themes live in [`crate::defaults::builtin_themes`].
    pub themes: Vec<Theme>,
    /// User voice presets. Built-ins live in [`crate::defaults::builtin_voice_presets`].
    pub voice_presets: Vec<VoicePreset>,
    /// Per-app volumes to restore at startup (see `omni_audio::AudioController::restore_app_levels`).
    pub app_levels: Vec<crate::AppLevel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Settings {
    /// Name shown on decks. Empty means "use the machine hostname".
    pub host_name: String,
    /// Port of the deck server (HTTP + WebSocket).
    pub port: u16,
    pub launch_at_login: bool,
    /// Start in the tray without opening the window.
    pub start_hidden: bool,
    /// Theme of the desktop app.
    pub theme_id: String,
    /// Profile assigned to newly paired devices.
    pub default_profile_id: Option<String>,
    pub usb: UsbSettings,
    pub ai: AiSettings,
    pub obs: ObsSettings,
    pub home_assistant: HomeAssistantSettings,
    pub voice: VoiceSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct UsbSettings {
    /// Forward the deck server to Android devices over USB (`adb reverse`).
    pub enabled: bool,
    /// Custom adb binary. `None` searches PATH and the usual SDK locations.
    pub adb_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AiProvider {
    Openai,
    Anthropic,
    Ollama,
    Openrouter,
    /// Any OpenAI-compatible endpoint (LM Studio, vLLM, LocalAI, ...).
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AiSettings {
    pub provider: AiProvider,
    pub model: String,
    /// Override of the provider endpoint. Required for `custom`.
    pub base_url: Option<String>,
    pub api_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ObsSettings {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct HomeAssistantSettings {
    pub enabled: bool,
    /// e.g. `http://homeassistant.local:8123`
    pub url: String,
    /// Long-lived access token.
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct VoiceSettings {
    pub enabled: bool,
    pub preset_id: String,
    /// Input device id. `None` = system default microphone.
    pub input_device: Option<String>,
    /// Where processed audio goes, normally a virtual cable
    /// (BlackHole, VB-CABLE, or the OmniPanel virtual mic on Linux).
    pub output_device: Option<String>,
    /// Also play the processed voice on `monitor_device` (headphones).
    pub monitor: bool,
    pub monitor_device: Option<String>,
    pub input_gain_db: f32,
    pub output_gain_db: f32,
    pub noise_gate: bool,
    pub gate_threshold_db: f32,
}

/// A deck layout. Devices pick one profile each.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub rows: u8,
    pub cols: u8,
    /// Deck theme. `None` follows the desktop theme.
    pub theme_id: Option<String>,
    pub pages: Vec<Page>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Page {
    pub id: String,
    pub name: String,
    pub controls: Vec<Control>,
}

/// A button or slider placed on the grid. `x`/`y` are 0-based cells,
/// `w`/`h` the span in cells.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Control {
    pub id: String,
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
    pub style: Style,
    pub kind: ControlKind,
    /// Global shortcut that triggers this control, e.g. `"CmdOrCtrl+Alt+1"`.
    pub hotkey: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ControlKind {
    Button {
        #[serde(default)]
        action: Option<Action>,
    },
    Slider {
        target: VolumeTarget,
        #[serde(default = "yes")]
        vertical: bool,
    },
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Style {
    pub label: String,
    pub icon: Option<Icon>,
    /// Background color `#rrggbb`. `None` uses the theme button color.
    pub bg: Option<String>,
    /// Text / icon color. `None` uses the theme button text color.
    pub fg: Option<String>,
    /// Overrides used while the control is active (toggled on, muted, recording...).
    pub active_bg: Option<String>,
    pub active_label: Option<String>,
    pub active_icon: Option<Icon>,
    pub label_position: LabelPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum Icon {
    /// A Lucide icon name, e.g. `"mic-off"`.
    Builtin {
        name: String,
    },
    Emoji {
        value: String,
    },
    /// An image imported into the app data folder.
    Image {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LabelPosition {
    #[default]
    Bottom,
    Top,
    Center,
    Hidden,
}

/// A paired deck (phone, tablet, browser).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// SHA-256 (hex) of the device token. The token itself only lives on the device.
    pub token_hash: String,
    /// Profile shown on this device. `None` uses the default profile.
    pub profile_id: Option<String>,
    /// Short platform description derived from the user agent.
    pub platform: String,
    /// Unix milliseconds.
    #[ts(type = "number")]
    pub paired_at: u64,
    #[ts(type = "number")]
    pub last_seen: u64,
    pub deck: DeckSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct DeckSettings {
    /// Vibrate on press (Android).
    pub haptics: bool,
    /// Try to keep the screen on.
    pub keep_awake: bool,
    /// Dim overlay 0.0 - 0.8, handy on OLED/AMOLED screens at night.
    pub dim: f32,
    /// Show the page / connection bar.
    pub show_status_bar: bool,
    /// Transpose the grid when the screen orientation doesn't match it
    /// (a 5x3 profile becomes 3x5 on a portrait phone).
    pub auto_rotate: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub colors: ThemeColors,
    /// Corner radius in px for buttons and panels.
    pub radius: u8,
}

/// Every color is `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ThemeColors {
    pub background: String,
    pub surface: String,
    pub surface_hover: String,
    pub border: String,
    pub text: String,
    pub text_muted: String,
    pub accent: String,
    pub accent_text: String,
    pub success: String,
    pub warning: String,
    pub danger: String,
    /// Default deck button background / foreground.
    pub button: String,
    pub button_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct VoicePreset {
    pub id: String,
    pub name: String,
    /// Processed in order.
    pub effects: Vec<Effect>,
}

/// One stage of the voice effect chain. Mix values are 0.0 (dry) - 1.0 (wet).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum Effect {
    /// Pitch shift in semitones (-12 .. 12).
    Pitch {
        semitones: f32,
    },
    /// Robotic voice: fixed-pitch resynthesis around `frequency` Hz.
    Robot {
        frequency: f32,
        mix: f32,
    },
    RingMod {
        frequency: f32,
        mix: f32,
    },
    Echo {
        delay_ms: f32,
        feedback: f32,
        mix: f32,
    },
    Reverb {
        room_size: f32,
        damping: f32,
        mix: f32,
    },
    Distortion {
        drive: f32,
        mix: f32,
    },
    Filter {
        kind: FilterKind,
        frequency: f32,
        q: f32,
    },
    Chorus {
        rate_hz: f32,
        depth_ms: f32,
        mix: f32,
    },
    Flanger {
        rate_hz: f32,
        depth_ms: f32,
        feedback: f32,
        mix: f32,
    },
    Tremolo {
        rate_hz: f32,
        depth: f32,
    },
    Bitcrush {
        bits: u8,
        downsample: u8,
    },
    Gain {
        db: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FilterKind {
    Lowpass,
    Highpass,
    Bandpass,
}

impl Config {
    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    /// Profile for a device: its own, then the default profile, then the first one.
    pub fn profile_for_device(&self, device: &Device) -> Option<&Profile> {
        device
            .profile_id
            .as_deref()
            .and_then(|id| self.profile(id))
            .or_else(|| self.settings.default_profile_id.as_deref().and_then(|id| self.profile(id)))
            .or_else(|| self.profiles.first())
    }

    /// Repair anything a hand-edited, imported or older config could break:
    /// empty profiles, layouts (see [`Profile::normalize`]) and duplicate ids.
    /// Lookups and toggle state go by id, so a later duplicate profile or
    /// control gets a fresh id and the first one keeps every reference.
    pub fn normalize(&mut self) {
        self.schema = CONFIG_SCHEMA;
        if self.profiles.is_empty() {
            self.profiles.push(crate::defaults::default_profile());
        }
        let (mut profile_ids, mut control_ids) = (HashSet::new(), HashSet::new());
        for profile in &mut self.profiles {
            unique_id(&mut profile.id, &mut profile_ids);
            profile.normalize();
            for control in profile.pages.iter_mut().flat_map(|p| &mut p.controls) {
                unique_id(&mut control.id, &mut control_ids);
            }
        }
        let has_default =
            self.settings.default_profile_id.as_deref().is_some_and(|id| self.profiles.iter().any(|p| p.id == id));
        if !has_default {
            self.settings.default_profile_id = self.profiles.first().map(|p| p.id.clone());
        }
        for device in &mut self.devices {
            device.deck.dim = device.deck.dim.clamp(0.0, 0.8);
        }
    }
}

impl Profile {
    pub fn page(&self, id: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.id == id)
    }

    /// Repair the layout: grid bounds, empty or duplicate page ids, and
    /// controls outside the grid or overlapping an earlier one. Those keep
    /// their span (clamped to the grid) and move to the first free cells
    /// (row-major) of their page, else of the following pages, else of a new
    /// page, so no control is lost. Valid layouts are left untouched.
    pub fn normalize(&mut self) {
        self.rows = self.rows.clamp(1, MAX_GRID);
        self.cols = self.cols.clamp(1, MAX_GRID);
        if self.pages.is_empty() {
            self.pages.push(Page { id: crate::new_id(), name: "Page 1".into(), controls: vec![] });
        }
        let mut page_ids = HashSet::new();
        for page in &mut self.pages {
            unique_id(&mut page.id, &mut page_ids);
        }
        let (rows, cols) = (self.rows, self.cols);
        // Controls that found no room, carried to the next page.
        let mut homeless = Vec::new();
        let mut index = 0;
        while index < self.pages.len() || !homeless.is_empty() {
            if index == self.pages.len() {
                let mut id = String::new();
                unique_id(&mut id, &mut page_ids);
                self.pages.push(Page { id, name: format!("Page {}", index + 1), controls: vec![] });
            }
            let page = &mut self.pages[index];
            let mut cells = Cells::new(rows, cols);
            let mut moving = Vec::new();
            for mut c in std::mem::take(&mut page.controls) {
                if cells.keep(&mut c) { page.controls.push(c) } else { moving.push(c) }
            }
            moving.append(&mut homeless);
            for mut c in moving {
                if cells.relocate(&mut c) { page.controls.push(c) } else { homeless.push(c) }
            }
            index += 1;
        }
    }

    /// Every control of every page.
    pub fn controls(&self) -> impl Iterator<Item = &Control> {
        self.pages.iter().flat_map(|p| p.controls.iter())
    }
}

/// Give `id` a fresh value when it's empty or already `seen`, then mark it seen.
fn unique_id(id: &mut String, seen: &mut HashSet<String>) {
    while id.is_empty() || seen.contains(id) {
        *id = crate::new_id();
    }
    seen.insert(id.clone());
}

/// Cells taken on one page, for [`Profile::normalize`].
struct Cells {
    rows: u8,
    cols: u8,
    taken: Vec<bool>,
}

impl Cells {
    fn new(rows: u8, cols: u8) -> Self {
        Self { rows, cols, taken: vec![false; usize::from(rows) * usize::from(cols)] }
    }

    /// Claim the span if it's inside the grid and free.
    fn claim(&mut self, x: u8, y: u8, w: u8, h: u8) -> bool {
        let inside = x < self.cols && y < self.rows && w <= self.cols - x && h <= self.rows - y;
        let cols = usize::from(self.cols);
        let cells = || (y..y + h).flat_map(move |r| (x..x + w).map(move |c| usize::from(r) * cols + usize::from(c)));
        if !inside || cells().any(|i| self.taken[i]) {
            return false;
        }
        cells().for_each(|i| self.taken[i] = true);
        true
    }

    /// Keep `c` where it is, its span clamped to the grid edge, if those cells are free.
    fn keep(&mut self, c: &mut Control) -> bool {
        if c.x >= self.cols || c.y >= self.rows {
            return false;
        }
        let (w, h) = (c.w.clamp(1, self.cols - c.x), c.h.clamp(1, self.rows - c.y));
        let kept = self.claim(c.x, c.y, w, h);
        if kept {
            (c.w, c.h) = (w, h);
        }
        kept
    }

    /// Move `c`, its span clamped to the grid, to the first free spot
    /// (row-major). Always succeeds on an empty page.
    fn relocate(&mut self, c: &mut Control) -> bool {
        let (rows, cols) = (self.rows, self.cols);
        let (w, h) = (c.w.clamp(1, cols), c.h.clamp(1, rows));
        for (y, x) in (0..=rows - h).flat_map(|y| (0..=cols - w).map(move |x| (y, x))) {
            if self.claim(x, y, w, h) {
                (c.x, c.y, c.w, c.h) = (x, y, w, h);
                return true;
            }
        }
        false
    }
}

impl Default for Config {
    fn default() -> Self {
        let profile = crate::defaults::default_profile();
        Self {
            schema: CONFIG_SCHEMA,
            settings: Settings { default_profile_id: Some(profile.id.clone()), ..Settings::default() },
            profiles: vec![profile],
            devices: vec![],
            themes: vec![],
            voice_presets: vec![],
            app_levels: vec![],
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            host_name: String::new(),
            port: 7710,
            launch_at_login: false,
            start_hidden: false,
            theme_id: "midnight".into(),
            default_profile_id: None,
            usb: UsbSettings::default(),
            ai: AiSettings::default(),
            obs: ObsSettings::default(),
            home_assistant: HomeAssistantSettings::default(),
            voice: VoiceSettings::default(),
        }
    }
}

impl Default for UsbSettings {
    fn default() -> Self {
        Self { enabled: true, adb_path: None }
    }
}

impl Default for AiSettings {
    fn default() -> Self {
        Self { provider: AiProvider::Openai, model: "gpt-6-luna".into(), base_url: None, api_key: String::new() }
    }
}

impl Default for ObsSettings {
    fn default() -> Self {
        Self { enabled: false, host: "127.0.0.1".into(), port: 4455, password: String::new() }
    }
}

impl Default for HomeAssistantSettings {
    fn default() -> Self {
        Self { enabled: false, url: "http://homeassistant.local:8123".into(), token: String::new() }
    }
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            preset_id: "robot".into(),
            input_device: None,
            output_device: None,
            monitor: false,
            monitor_device: None,
            input_gain_db: 0.0,
            output_gain_db: 0.0,
            noise_gate: true,
            gate_threshold_db: -50.0,
        }
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self { id: crate::new_id(), name: "Profile".into(), rows: 3, cols: 5, theme_id: None, pages: vec![] }
    }
}

impl Default for Page {
    fn default() -> Self {
        Self { id: crate::new_id(), name: "Page".into(), controls: vec![] }
    }
}

impl Default for Control {
    fn default() -> Self {
        Self {
            id: crate::new_id(),
            x: 0,
            y: 0,
            w: 1,
            h: 1,
            style: Style::default(),
            kind: ControlKind::Button { action: None },
            hotkey: None,
        }
    }
}

impl Default for Device {
    fn default() -> Self {
        Self {
            id: crate::new_id(),
            name: "Deck".into(),
            token_hash: String::new(),
            profile_id: None,
            platform: String::new(),
            paired_at: 0,
            last_seen: 0,
            deck: DeckSettings::default(),
        }
    }
}

impl Default for DeckSettings {
    fn default() -> Self {
        Self { haptics: true, keep_awake: true, dim: 0.0, show_status_bar: true, auto_rotate: true }
    }
}

/// Fields missing from a stored theme fall back to the default built-in theme.
impl Default for Theme {
    fn default() -> Self {
        Self { id: format!("custom-{}", crate::new_id()), name: "Theme".into(), ..crate::defaults::default_theme() }
    }
}

impl Default for ThemeColors {
    fn default() -> Self {
        crate::defaults::default_theme().colors
    }
}

impl Default for VoicePreset {
    fn default() -> Self {
        Self { id: crate::new_id(), name: "Preset".into(), effects: vec![] }
    }
}
