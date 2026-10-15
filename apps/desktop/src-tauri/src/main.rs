// OmniPanel desktop shell: starts the omni-host engine, then owns the window,
// tray, global hotkeys and launch-at-login. Closing the window destroys the
// webview (freeing its memory); the engine keeps running in the tray.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod hotkeys;
mod tray;
mod window;

use omni_core::HostEvent;
use omni_host::{Host, HostOptions};
use tauri::{Emitter, Manager, RunEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// Passed by the autostart entry: start in the tray.
const HIDDEN_ARG: &str = "--hidden";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,omni=info,omnipanel=info".into()),
        )
        .init();
    let launched_hidden = std::env::args().any(|a| a == HIDDEN_ARG);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| window::show(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![HIDDEN_ARG])))
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(hotkeys::on_shortcut).build())
        .plugin(tauri_plugin_dialog::init())
        .manage(hotkeys::Registered::default())
        .invoke_handler(commands::handler())
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            let version = app.package_info().version.to_string();
            let host = tauri::async_runtime::block_on(Host::start(HostOptions { data_dir, version }))?;
            app.manage(host.clone());
            tray::create(app.handle())?;
            sync_with_config(app.handle(), &host);
            forward_events(app.handle().clone(), host.clone());
            if launched_hidden || host.config().settings.start_hidden {
                window::set_dock_visible(app.handle(), false);
            } else {
                window::show(app.handle());
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build OmniPanel");

    app.run(|app, event| match event {
        // Last window closed: keep running in the tray.
        RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } if label == window::MAIN => {
            window::on_destroyed(app);
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => window::show(app),
        RunEvent::Exit => {
            if let Some(host) = app.try_state::<Host>() {
                tauri::async_runtime::block_on(host.shutdown());
            }
        }
        _ => {}
    });
}

/// Pushes host events to the UI and re-syncs hotkeys / autostart on config changes.
fn forward_events(app: tauri::AppHandle, host: Host) {
    let mut events = host.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    if matches!(event, HostEvent::Config { .. }) {
                        sync_with_config(&app, &host);
                    }
                    if app.get_webview_window(window::MAIN).is_some() {
                        let _ = app.emit("host-event", &event);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn sync_with_config(app: &tauri::AppHandle, host: &Host) {
    hotkeys::sync(app, host);
    let wanted = host.config().settings.launch_at_login;
    let autostart = app.autolaunch();
    let result = match autostart.is_enabled() {
        Ok(enabled) if enabled == wanted => Ok(()),
        _ if wanted => autostart.enable(),
        _ => autostart.disable(),
    };
    if let Err(e) = result {
        host.log(omni_core::LogLevel::Warn, format!("Launch at login: {e}"));
    }
}
