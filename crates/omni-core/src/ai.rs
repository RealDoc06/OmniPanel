//! Types for the AI assistant that turns a description into a control.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::action::Action;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GenerateRequest {
    /// What the user wants, e.g. "mute discord and switch OBS to the BRB scene".
    pub prompt: String,
    /// Previous turns, to refine a result ("make it also pause spotify").
    #[serde(default)]
    pub history: Vec<ChatMessage>,
    /// Action currently on the button, if any, so the model can edit it.
    #[serde(default)]
    pub current: Option<Action>,
}

/// Machine facts the host adds to every prompt.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AiContext {
    /// `macos` | `windows` | `linux`
    pub os: String,
    pub obs_scenes: Vec<String>,
    pub obs_inputs: Vec<String>,
    pub audio_apps: Vec<String>,
    pub voice_presets: Vec<String>,
    pub home_assistant: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GeneratedControl {
    /// Short button label (max ~14 chars).
    pub label: String,
    /// Lucide icon name, e.g. "mic-off".
    pub icon: Option<String>,
    pub action: Action,
    /// One or two sentences about what it does and any setup needed.
    pub explanation: String,
}
