//! The desktop UI's command surface. The Tauri shell forwards these to
//! `omni_host::Host::handle` through its single `host` command, the dev bridge
//! (`cargo run -p omni-host --example serve -- --bridge`) over HTTP.
//! `apps/desktop/src/lib/api.ts` wraps each variant with its result type.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::action::{Action, VolumeTarget};
use crate::ai::GenerateRequest;
use crate::model::{DeckSettings, Profile, Settings, Theme, VoicePreset};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "cmd", content = "args", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum HostRequest {
    GetState,
    /// Every config mutation persists and pushes a `config` host event.
    SetSettings {
        settings: Settings,
    },
    SaveProfile {
        profile: Profile,
    },
    DeleteProfile {
        id: String,
    },
    /// Writes the profile as JSON to `path`.
    ExportProfile {
        id: String,
        #[ts(type = "string")]
        path: PathBuf,
    },
    /// Reads a profile JSON, gives it fresh ids, saves and returns it.
    ImportProfile {
        #[ts(type = "string")]
        path: PathBuf,
    },
    UpdateDevice {
        id: String,
        name: String,
        profile_id: Option<String>,
        deck: DeckSettings,
    },
    UnpairDevice {
        id: String,
    },
    SaveTheme {
        theme: Theme,
    },
    DeleteTheme {
        id: String,
    },
    SaveVoicePreset {
        preset: VoicePreset,
    },
    DeleteVoicePreset {
        id: String,
    },
    /// The code stays valid until it expires or a device pairs.
    StartPairing,
    StopPairing,
    /// QR code for a deck URL, as standalone SVG markup.
    QrSvg {
        text: String,
    },
    /// Run an action now (the editor "Test" button).
    RunAction {
        action: Action,
    },
    InstalledApps,
    /// Copies and downscales an image into the data dir, returns its id
    /// (served at `http://127.0.0.1:<port>/img/<id>`).
    ImportImage {
        #[ts(type = "string")]
        path: PathBuf,
    },
    GetMixer,
    SetVolume {
        target: VolumeTarget,
        value: f32,
    },
    SetMuted {
        target: VolumeTarget,
        muted: bool,
    },
    VoiceDevices,
    VoiceLevels,
    /// Linux: creates the OmniPanel virtual mic, returns its output device id.
    CreateVirtualMic,
    ObsInventory,
    HaEntities,
    AiGenerate {
        request: GenerateRequest,
    },
    AiModels,
    AiTest,
    OpenDataDir,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_with_and_without_args() {
        let get: HostRequest = serde_json::from_str(r#"{"cmd":"getState"}"#).unwrap();
        assert!(matches!(get, HostRequest::GetState));
        let unpair: HostRequest = serde_json::from_str(r#"{"cmd":"unpairDevice","args":{"id":"abc"}}"#).unwrap();
        assert!(matches!(unpair, HostRequest::UnpairDevice { id } if id == "abc"));
        let update: HostRequest = serde_json::from_value(serde_json::json!({
            "cmd": "updateDevice",
            "args": { "id": "d", "name": "Tab", "profileId": null, "deck": DeckSettings::default() },
        }))
        .unwrap();
        assert!(matches!(update, HostRequest::UpdateDevice { profile_id: None, .. }));
    }
}
