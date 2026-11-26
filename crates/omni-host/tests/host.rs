//! Host-level behaviour that doesn't need a deck connection.

use omni_core::{Action, ControlKind, NavTarget, Settings, VoiceOp, new_id};
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
async fn sharing_an_app_toggles_and_does_not_survive_a_restart() {
    let host = start_host().await;
    let share = |op: fn(String) -> VoiceOp| Action::Voice { op: op("Spotify".into()) };
    let toggle = share(|app| VoiceOp::ToggleShareApp { app });
    let shared = |host: &Host| host.config().settings.voice.shared_apps.clone();
    if let Some(reason) = omni_voice::share_unsupported() {
        // Where apps cannot be captured the action says so and changes nothing.
        let error = host.run_action(toggle).await.unwrap_err();
        assert_eq!(error.to_string(), reason);
        assert!(shared(&host).is_empty());
        host.shutdown().await;
        return;
    }

    assert_eq!(host.run_action(toggle.clone()).await.unwrap().message.as_deref(), Some("Sharing Spotify"));
    // Sharing needs no voice effect, and another spelling of the app is the same app.
    assert!(!host.config().settings.voice.enabled);
    host.run_action(Action::Voice { op: VoiceOp::ShareApp { app: " spotify ".into() } }).await.unwrap();
    assert_eq!(shared(&host), ["Spotify"]);
    host.run_action(toggle.clone()).await.unwrap();
    assert!(shared(&host).is_empty());
    host.run_action(share(|app| VoiceOp::UnshareApp { app })).await.unwrap();
    assert!(host.run_action(Action::Voice { op: VoiceOp::ShareApp { app: " ".into() } }).await.is_err());

    host.run_action(toggle).await.unwrap();
    let data_dir = host.data_dir().to_path_buf();
    host.shutdown().await;
    let restarted = Host::start(HostOptions { data_dir, version: "test".into() }).await.unwrap();
    assert!(shared(&restarted).is_empty(), "a later call must not carry the app's sound unasked");
    restarted.shutdown().await;
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

fn nav(to: NavTarget) -> Option<Action> {
    Some(Action::Navigate { to })
}

#[tokio::test]
async fn profiles_import_with_fresh_ids_and_last_profile_is_kept() {
    let host = start_host().await;
    let mut original = host.config().profiles[0].clone();
    assert!(host.delete_profile(&original.id).is_err());
    // Buttons that go to its second page, to itself and to another profile.
    let second_page = NavTarget::Page { page_id: original.pages[1].id.clone() };
    let itself = NavTarget::Profile { profile_id: original.id.clone() };
    let elsewhere = NavTarget::Profile { profile_id: "other-profile".into() };
    let toggle = Action::Toggle { on: Box::new(nav(itself).unwrap()), off: Box::new(nav(elsewhere.clone()).unwrap()) };
    let controls = &mut original.pages[0].controls;
    controls[0].kind = ControlKind::Button { action: nav(second_page) };
    controls[1].kind = ControlKind::Button { action: Some(toggle) };
    host.save_profile(original.clone()).unwrap();

    let file = host.data_dir().join("export.json");
    host.export_profile(&original.id, &file).unwrap();
    let imported = host.import_profile(&file).unwrap();
    assert_ne!(imported.id, original.id);
    assert_eq!(imported.name, "Default (imported)");
    assert_eq!(imported.pages.len(), original.pages.len());
    assert!(imported.pages[0].controls.iter().all(|c| original.pages[0].controls.iter().all(|o| o.id != c.id)));
    // Its own navigation follows the new ids, other profiles stay.
    let actions: Vec<_> = imported.pages[0].controls[..2]
        .iter()
        .map(|c| match &c.kind {
            ControlKind::Button { action } => action.clone(),
            ControlKind::Slider { .. } => None,
        })
        .collect();
    let toggle = Action::Toggle {
        on: Box::new(nav(NavTarget::Profile { profile_id: imported.id.clone() }).unwrap()),
        off: Box::new(nav(elsewhere).unwrap()),
    };
    assert_eq!(actions, [nav(NavTarget::Page { page_id: imported.pages[1].id.clone() }), Some(toggle)]);

    host.delete_profile(&original.id).unwrap();
    assert_eq!(host.config().profiles.len(), 1);
    host.shutdown().await;
}

/// A save that fails changes nothing, so the retry saves for real.
#[tokio::test]
async fn failed_saves_leave_the_config_alone() {
    let host = start_host().await;
    let path = omni_core::store::config_path(host.data_dir());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap(); // the save can't replace a directory
    let before = host.config().settings.clone();
    let settings = Settings { host_name: "Studio".into(), ..before.clone() };
    assert!(host.set_settings(settings.clone()).is_err());
    assert_eq!(host.config().settings, before);

    std::fs::remove_dir(&path).unwrap();
    host.set_settings(settings).unwrap();
    assert_eq!(omni_core::store::load(host.data_dir()).unwrap().settings.host_name, "Studio");
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

#[tokio::test]
async fn settings_patches_preserve_remote_fields_and_reject_invalid_changes_atomically() {
    let host = start_host().await;
    // A remote settings update lands after the UI prepared its host-name edit.
    host.patch_settings(
        serde_json::from_value(serde_json::json!({"startHidden": true, "ai": {"baseUrl": "http://127.0.0.1:9"}}))
            .unwrap(),
    )
    .unwrap();
    host.handle(omni_core::HostRequest::PatchSettings {
        patch: omni_core::SettingsPatch { host_name: Some("Studio".into()), ..Default::default() },
    })
    .await
    .unwrap();
    assert!(host.config().settings.start_hidden);
    assert_eq!(host.config().settings.host_name, "Studio");
    assert_eq!(host.config().settings.ai.base_url.as_deref(), Some("http://127.0.0.1:9"));
    host.patch_settings(serde_json::from_value(serde_json::json!({"ai": {"baseUrl": null}})).unwrap()).unwrap();
    assert!(host.config().settings.ai.base_url.is_none());
    assert!(
        serde_json::from_value::<omni_core::HostRequest>(serde_json::json!({
            "cmd": "patchSettings", "args": {"patch": {"hostName": "Lost", "port": "bad"}}
        }))
        .is_err()
    );
    assert_eq!(host.config().settings.host_name, "Studio");
    assert_eq!(omni_core::store::load(host.data_dir()).unwrap().settings, host.config().settings);
    host.shutdown().await;
}

#[tokio::test]
async fn late_old_saves_cannot_overwrite_a_newer_snapshot() {
    let host = start_host().await;
    let mut profile = host.config().profiles[0].clone();
    for (revision, label) in [(2, "LATEST"), (1, "FIRST"), (2, "duplicate")] {
        profile.pages[0].controls[0].style.label = label.into();
        host.handle(omni_core::HostRequest::Commit {
            client: "test-window".into(),
            revision,
            request: Box::new(omni_core::HostRequest::SaveProfile { profile: profile.clone() }),
        })
        .await
        .unwrap();
    }
    assert_eq!(host.config().profiles[0].pages[0].controls[0].style.label, "LATEST");
    // A new webview has its own revision sequence.
    profile.pages[0].controls[0].style.label = "New window".into();
    host.handle(omni_core::HostRequest::Commit {
        client: "new-window".into(),
        revision: 1,
        request: Box::new(omni_core::HostRequest::SaveProfile { profile }),
    })
    .await
    .unwrap();
    assert_eq!(host.config().profiles[0].pages[0].controls[0].style.label, "New window");
    host.shutdown().await;
}
