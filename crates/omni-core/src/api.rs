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
    /// Ordered UI save. Only config-save requests are accepted inside this envelope.
    Commit {
        client: String,
        #[ts(type = "number")]
        revision: u64,
        request: Box<HostRequest>,
    },
    /// Merge changed fields into the current settings under the config lock.
    PatchSettings {
        patch: SettingsPatch,
    },
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

/// Deserializing a present field wraps even an explicit null in `Some`.
/// Missing fields use `Default::default`, keeping nullable patch fields tri-state.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Changed settings fields only; omitted fields retain their current values.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub host_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub launch_at_login: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub start_hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub theme_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub default_profile_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub usb: Option<UsbSettingsPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub ai: Option<AiSettingsPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub obs: Option<ObsSettingsPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub home_assistant: Option<HomeAssistantSettingsPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub voice: Option<VoiceSettingsPatch>,
}

impl SettingsPatch {
    /// Apply under the caller's config lock.
    pub fn apply(self, settings: &mut Settings) {
        if let Some(value) = self.host_name {
            settings.host_name = value;
        }
        if let Some(value) = self.port {
            settings.port = value;
        }
        if let Some(value) = self.launch_at_login {
            settings.launch_at_login = value;
        }
        if let Some(value) = self.start_hidden {
            settings.start_hidden = value;
        }
        if let Some(value) = self.theme_id {
            settings.theme_id = value;
        }
        if let Some(value) = self.default_profile_id {
            settings.default_profile_id = value;
        }
        if let Some(value) = self.usb {
            value.apply(&mut settings.usb);
        }
        if let Some(value) = self.ai {
            value.apply(&mut settings.ai);
        }
        if let Some(value) = self.obs {
            value.apply(&mut settings.obs);
        }
        if let Some(value) = self.home_assistant {
            value.apply(&mut settings.home_assistant);
        }
        if let Some(value) = self.voice {
            value.apply(&mut settings.voice);
        }
    }
}

/// Changed settings fields only; omitted fields retain their current values.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct UsbSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub adb_path: Option<Option<String>>,
}

impl UsbSettingsPatch {
    /// Apply under the caller's config lock.
    pub fn apply(self, settings: &mut crate::UsbSettings) {
        if let Some(value) = self.enabled {
            settings.enabled = value;
        }
        if let Some(value) = self.adb_path {
            settings.adb_path = value;
        }
    }
}

/// Changed settings fields only; omitted fields retain their current values.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct AiSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub provider: Option<crate::AiProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub base_url: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub api_key: Option<String>,
}

impl AiSettingsPatch {
    /// Apply under the caller's config lock.
    pub fn apply(self, settings: &mut crate::AiSettings) {
        if let Some(value) = self.provider {
            settings.provider = value;
        }
        if let Some(value) = self.model {
            settings.model = value;
        }
        if let Some(value) = self.base_url {
            settings.base_url = value;
        }
        if let Some(value) = self.api_key {
            settings.api_key = value;
        }
    }
}

/// Changed settings fields only; omitted fields retain their current values.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ObsSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub password: Option<String>,
}

impl ObsSettingsPatch {
    /// Apply under the caller's config lock.
    pub fn apply(self, settings: &mut crate::ObsSettings) {
        if let Some(value) = self.enabled {
            settings.enabled = value;
        }
        if let Some(value) = self.host {
            settings.host = value;
        }
        if let Some(value) = self.port {
            settings.port = value;
        }
        if let Some(value) = self.password {
            settings.password = value;
        }
    }
}

/// Changed settings fields only; omitted fields retain their current values.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct HomeAssistantSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub token: Option<String>,
}

impl HomeAssistantSettingsPatch {
    /// Apply under the caller's config lock.
    pub fn apply(self, settings: &mut crate::HomeAssistantSettings) {
        if let Some(value) = self.enabled {
            settings.enabled = value;
        }
        if let Some(value) = self.url {
            settings.url = value;
        }
        if let Some(value) = self.token {
            settings.token = value;
        }
    }
}

/// Changed settings fields only; omitted fields retain their current values.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct VoiceSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub preset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub input_device: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub output_device: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub monitor: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub monitor_device: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub input_gain_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub output_gain_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub noise_gate: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    #[ts(optional)]
    pub gate_threshold_db: Option<f32>,
}

impl VoiceSettingsPatch {
    /// Apply under the caller's config lock.
    pub fn apply(self, settings: &mut crate::VoiceSettings) {
        if let Some(value) = self.enabled {
            settings.enabled = value;
        }
        if let Some(value) = self.preset_id {
            settings.preset_id = value;
        }
        if let Some(value) = self.input_device {
            settings.input_device = value;
        }
        if let Some(value) = self.output_device {
            settings.output_device = value;
        }
        if let Some(value) = self.monitor {
            settings.monitor = value;
        }
        if let Some(value) = self.monitor_device {
            settings.monitor_device = value;
        }
        if let Some(value) = self.input_gain_db {
            settings.input_gain_db = value;
        }
        if let Some(value) = self.output_gain_db {
            settings.output_gain_db = value;
        }
        if let Some(value) = self.noise_gate {
            settings.noise_gate = value;
        }
        if let Some(value) = self.gate_threshold_db {
            settings.gate_threshold_db = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_patch_merges_nested_fields_and_distinguishes_null_from_missing() {
        let mut settings = Settings::default();
        settings.ai.base_url = Some("http://127.0.0.1:9".into());
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({"ai": {"model": "test-model"}})).unwrap();
        assert_eq!(serde_json::to_value(&patch).unwrap(), serde_json::json!({"ai": {"model": "test-model"}}));
        patch.apply(&mut settings);
        assert_eq!(settings.ai.model, "test-model");
        assert_eq!(settings.ai.base_url.as_deref(), Some("http://127.0.0.1:9"));
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({"ai": {"baseUrl": null}})).unwrap();
        assert_eq!(patch.ai.as_ref().unwrap().base_url, Some(None));
        assert_eq!(serde_json::to_value(&patch).unwrap(), serde_json::json!({"ai": {"baseUrl": null}}));
        patch.apply(&mut settings);
        assert!(settings.ai.base_url.is_none());
        assert_eq!(settings.ai.model, "test-model");
        for invalid in [
            serde_json::json!({"port": "bad"}),
            serde_json::json!({"port": null}),
            serde_json::json!({"ai": {"unknown": true}}),
            serde_json::json!({"unknown": true}),
        ] {
            assert!(serde_json::from_value::<SettingsPatch>(invalid).is_err());
        }
    }

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
