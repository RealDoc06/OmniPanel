//! WebSocket protocol between a deck (browser client) and the host.
//!
//! Every message is a JSON object tagged by `t`. Flow:
//! 1. deck connects to `ws://host:port/ws` and sends `hello` (with its token if paired)
//! 2. host answers `welcome` + `layout` + `state`, or `needPair`
//! 3. unpaired decks send `pair` with the code shown in the desktop app,
//!    host answers `pairResult` (token to store) followed by `welcome`/`layout`
//! 4. deck sends `press` / `slide` / `nav`, host pushes `layout` / `state` / `toast`

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::action::NavTarget;
use crate::model::{DeckSettings, LabelPosition, ThemeColors};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "t", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum DeckToHost {
    Hello {
        #[serde(default)]
        token: Option<String>,
        client: ClientInfo,
    },
    Pair {
        code: String,
        name: String,
    },
    Press {
        control: String,
    },
    /// Slider moved. `value` is 0.0 - 1.0.
    Slide {
        control: String,
        value: f32,
    },
    Nav {
        to: NavTarget,
    },
    /// Deck screen size changed (rotation); host may re-send the layout.
    Resize {
        width: u32,
        height: u32,
    },
    /// "Unpair this device" on the deck: the host forgets it.
    Unpair,
    Ping,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ClientInfo {
    pub protocol: u32,
    pub user_agent: String,
    /// CSS pixels.
    pub width: u32,
    pub height: u32,
    pub pixel_ratio: f32,
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self { protocol: PROTOCOL_VERSION, user_agent: String::new(), width: 0, height: 0, pixel_ratio: 1.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "t", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum HostToDeck {
    /// Token missing or unknown: show the pairing screen.
    NeedPair {
        host_name: String,
    },
    PairResult {
        ok: bool,
        token: Option<String>,
        error: Option<String>,
    },
    Welcome {
        device_id: String,
        device_name: String,
        host_name: String,
        host_version: String,
    },
    Layout {
        layout: Box<DeckLayout>,
    },
    /// Partial state update: only the listed controls changed.
    State {
        states: Vec<ControlState>,
    },
    Toast {
        text: String,
        level: ToastLevel,
    },
    /// The device was removed in the desktop app: forget the token.
    Unpaired,
    Pong,
}

/// Everything a deck needs to render the current page, already resolved
/// (theme colors, icon markup, image URLs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeckLayout {
    pub profile_id: String,
    pub profile_name: String,
    pub page_id: String,
    pub page_name: String,
    pub page_index: u32,
    pub page_count: u32,
    /// Every page of the profile, in order (for page dots).
    pub pages: Vec<PageRef>,
    pub rows: u8,
    pub cols: u8,
    pub theme: ThemeColors,
    pub radius: u8,
    pub settings: DeckSettings,
    pub controls: Vec<DeckControl>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PageRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeckControl {
    pub id: String,
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
    pub kind: DeckControlKind,
    pub label: String,
    pub icon: Option<DeckIcon>,
    pub bg: String,
    pub fg: String,
    pub label_position: LabelPosition,
    /// Look used while `state.active` is true.
    pub active: ActiveStyle,
    pub state: ControlState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ActiveStyle {
    pub bg: String,
    pub label: Option<String>,
    pub icon: Option<DeckIcon>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum DeckControlKind {
    Button,
    Slider { vertical: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum DeckIcon {
    /// Inner SVG markup for a 24x24 viewBox, stroke = currentColor.
    Svg {
        markup: String,
    },
    Emoji {
        value: String,
    },
    /// Path relative to the host, e.g. `/img/ab12.png`.
    Image {
        url: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ControlState {
    pub id: String,
    /// Toggled on / muted / recording / current scene...
    pub active: bool,
    /// Slider position 0.0 - 1.0.
    pub value: Option<f32>,
    /// Dynamic label (e.g. "42%"); `None` keeps the configured label.
    pub label: Option<String>,
    /// The control can't work right now (OBS offline, app not running...).
    pub disabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ToastLevel {
    Info,
    Success,
    Error,
}
