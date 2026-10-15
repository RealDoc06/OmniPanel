//! Host-level behaviour that doesn't need a deck connection.

use omni_core::{Action, VoiceOp, new_id};
use omni_host::{Host, HostOptions};

async fn start_host() -> Host {
    let data_dir = std::env::temp_dir().join(format!("omni-host-test-{}", new_id()));
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut config = omni_core::Config::default();
    config.settings.port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    config.settings.usb.enabled = false;
    // Keep the real voice engine away from the microphone when a test enables it.
    config.settings.voice.input_device = Some("omnipanel-test-missing-device".into());
    omni_core::store::save(&data_dir, &config).unwrap();
    Host::start(HostOptions { data_dir, version: "test".into() }).await.unwrap()
}

#[tokio::test]
async fn voice_cycle_walks_the_given_presets_and_persists() {
    let host = start_host().await;
    let cycle = Action::Voice { op: VoiceOp::Cycle { preset_ids: vec!["robot".into(), "deep".into()] } };
    assert_eq!(host.config().settings.voice.preset_id, "robot");

    let outcome = host.run_action(cycle.clone()).await.unwrap();
    assert_eq!(outcome.message.as_deref(), Some("Deep"));
    assert!(host.config().settings.voice.enabled);
    host.run_action(cycle).await.unwrap();
    assert_eq!(host.config().settings.voice.preset_id, "robot");

    let reloaded = omni_core::store::load(host.data_dir()).unwrap();
    assert_eq!(reloaded.settings.voice.preset_id, "robot");
    host.run_action(Action::Voice { op: VoiceOp::Disable }).await.unwrap();
    assert!(host.run_action(Action::Voice { op: VoiceOp::SetPreset { preset_id: "nope".into() } }).await.is_err());
    host.shutdown().await;
}

#[tokio::test]
async fn hotkeys_are_deduplicated_case_insensitively() {
    let host = start_host().await;
    let mut profile = host.config().profiles[0].clone();
    let controls = &mut profile.pages[0].controls;
    controls[0].hotkey = Some("CmdOrCtrl+Shift+1".into());
    controls[1].hotkey = Some("cmdorctrl+shift+1".into());
    controls[2].hotkey = Some("F13".into());
    host.save_profile(profile).unwrap();
    assert_eq!(host.hotkeys(), vec!["CmdOrCtrl+Shift+1".to_string(), "F13".to_string()]);
    host.shutdown().await;
}

#[tokio::test]
async fn profiles_import_with_fresh_ids_and_last_profile_is_kept() {
    let host = start_host().await;
    let original = host.config().profiles[0].clone();
    assert!(host.delete_profile(&original.id).is_err());

    let file = host.data_dir().join("export.json");
    host.export_profile(&original.id, &file).unwrap();
    let imported = host.import_profile(&file).unwrap();
    assert_ne!(imported.id, original.id);
    assert_eq!(imported.name, "Default (imported)");
    assert_eq!(imported.pages.len(), original.pages.len());
    assert!(imported.pages[0].controls.iter().all(|c| original.pages[0].controls.iter().all(|o| o.id != c.id)));
    host.delete_profile(&original.id).unwrap();
    assert_eq!(host.config().profiles.len(), 1);
    host.shutdown().await;
}

/// The Tauri shell calls into the host from its main thread, which has no
/// tokio runtime: settings changes that restart the server, the USB bridge
/// and the voice engine must not panic there.
#[tokio::test(flavor = "multi_thread")]
async fn settings_changes_work_from_threads_without_a_runtime() {
    let host = start_host().await;
    let mut settings = host.config().settings.clone();
    settings.port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    settings.usb.enabled = true;
    settings.usb.adb_path = Some("/nonexistent/adb".into());
    settings.voice.enabled = true;
    let off_runtime = host.clone();
    std::thread::spawn(move || off_runtime.set_settings(settings)).join().expect("no panic").unwrap();
    host.shutdown().await;
}
