//! Serves [`HostRequest`]s from the desktop UI (Tauri shell or dev bridge).

use omni_core::HostRequest;
use serde::Serialize;
use serde_json::Value;

use crate::{Host, Result};

impl Host {
    /// Handle one UI request; the result is the JSON the UI expects for it.
    pub async fn handle(&self, request: HostRequest) -> Result<Value> {
        use HostRequest as R;
        match request {
            R::GetState => json(self.state()),
            R::SetSettings { settings } => json(self.set_settings(settings)?),
            R::SaveProfile { profile } => json(self.save_profile(profile)?),
            R::DeleteProfile { id } => json(self.delete_profile(&id)?),
            R::ExportProfile { id, path } => json(self.export_profile(&id, &path)?),
            R::ImportProfile { path } => json(self.import_profile(&path)?),
            R::UpdateDevice { id, name, profile_id, deck } => json(self.update_device(&id, name, profile_id, deck)?),
            R::UnpairDevice { id } => json(self.unpair_device(&id)?),
            R::SaveTheme { theme } => json(self.save_theme(theme)?),
            R::DeleteTheme { id } => json(self.delete_theme(&id)?),
            R::SaveVoicePreset { preset } => json(self.save_voice_preset(preset)?),
            R::DeleteVoicePreset { id } => json(self.delete_voice_preset(&id)?),
            R::StartPairing => json(self.start_pairing()),
            R::StopPairing => {
                self.stop_pairing();
                Ok(Value::Null)
            }
            R::QrSvg { text } => json(qr_svg(&text)?),
            R::RunAction { action } => json(self.run_action(action).await?),
            R::InstalledApps => json(self.installed_apps().await),
            R::ImportImage { path } => json(self.import_image(path).await?),
            R::GetMixer => json(self.mixer().await),
            R::SetVolume { target, value } => json(self.set_volume(target, value).await?),
            R::SetMuted { target, muted } => json(self.set_muted(target, muted).await?),
            R::VoiceDevices => json(self.voice_devices().await?),
            R::VoiceLevels => json(self.voice_levels()),
            R::CreateVirtualMic => json(self.create_virtual_mic().await?),
            R::ObsInventory => json(self.obs_inventory().await?),
            R::HaEntities => json(self.ha_entities().await?),
            R::AiGenerate { request } => json(self.ai_generate(request).await?),
            R::AiModels => json(self.ai_models().await?),
            R::AiTest => json(self.ai_test().await?),
            R::OpenDataDir => json(open_in_file_manager(self.data_dir())?),
        }
    }
}

fn json(value: impl Serialize) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

/// Standalone SVG QR code (black on white, quiet zone included).
fn qr_svg(text: &str) -> Result<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(text.as_bytes())?;
    Ok(code
        .render::<svg::Color<'_>>()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

/// Tokio reaps the opener once it exits.
fn open_in_file_manager(dir: &std::path::Path) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    tokio::process::Command::new(opener).arg(dir).spawn()?;
    Ok(())
}
