// OmniPanel desktop shell: starts the omni-host engine, then owns the window,
// tray, global hotkeys and launch-at-login. Closing the window destroys the
// webview (freeing its memory); the engine keeps running in the tray.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod hotkeys;
mod tray;
mod window;

use std::time::Duration;

use omni_core::{HostEvent, store};
use omni_host::{Host, HostOptions};
use tauri::{AppHandle, Emitter, Manager, RunEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

/// Passed by the autostart entry: start in the tray.
const HIDDEN_ARG: &str = "--hidden";
/// Quit waits this long at most for the engine to close decks and audio.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,omni=info,omnipanel=info".into()),
        )
        .init();
    // `args_os`: `args` panics on an argument that isn't valid Unicode.
    let launched_hidden = std::env::args_os().any(|a| a == HIDDEN_ARG);

    let built = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| window::show(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![HIDDEN_ARG])))
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(hotkeys::on_shortcut).build())
        .plugin(tauri_plugin_dialog::init())
        .manage(hotkeys::Registered::default())
        .invoke_handler(commands::handler())
        // Tauri panics (aborting the app) when setup fails, so failures are
        // reported here and end with a clean exit instead.
        .setup(move |app| {
            if let Err(e) = start(app.handle(), launched_hidden) {
                tracing::error!("OmniPanel can't start: {e:#}");
                let handle = app.handle().clone();
                app.dialog()
                    .message(format!("OmniPanel can't start: {e:#}"))
                    .kind(MessageDialogKind::Error)
                    .title("OmniPanel")
                    .show(move |_| handle.exit(1));
            }
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match built {
        Ok(app) => app,
        Err(e) => {
            tracing::error!("can't build OmniPanel: {e}");
            std::process::exit(1);
        }
    };

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
                // The timer is created inside `block_on`: tokio panics when a
                // timeout is made outside the runtime (this is the main thread).
                let shutdown = async { tokio::time::timeout(SHUTDOWN_TIMEOUT, host.shutdown()).await };
                if tauri::async_runtime::block_on(shutdown).is_err() {
                    tracing::warn!("engine shutdown timed out");
                }
            }
        }
        _ => {}
    });
}

/// Lock the data folder, start the engine, then the tray, hotkeys and window.
fn start(app: &AppHandle, launched_hidden: bool) -> anyhow::Result<()> {
    let data_dir = app.path().app_data_dir()?;
    match store::lock(&data_dir) {
        Ok(Some(lock)) => {
            app.manage(lock);
        }
        // Another OmniPanel owns the data folder: both started before
        // single-instance could tell. Leave without a trace (`app.exit` would
        // also delete the single-instance socket the other one listens on).
        Ok(None) => {
            tracing::info!("OmniPanel is already running");
            std::process::exit(0);
        }
        // Locking unsupported by the file system: run unguarded.
        Err(e) => tracing::warn!("can't lock {}: {e}", data_dir.display()),
    }
    let version = app.package_info().version.to_string();
    let host = tauri::async_runtime::block_on(Host::start(HostOptions { data_dir, version }))?;
    app.manage(host.clone());
    tray::create(app)?;
    sync_with_config(app, &host);
    forward_events(app.clone(), host.clone());
    if launched_hidden || host.config().settings.start_hidden {
        window::set_dock_visible(app, false);
    } else {
        window::show(app);
    }
    Ok(())
}

/// Pushes host events to the UI and re-syncs hotkeys / autostart on config changes.
fn forward_events(app: AppHandle, host: Host) {
    let mut events = host.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    if matches!(event, HostEvent::Config { .. }) {
                        // Waits on the main thread (hotkeys) and the disk (autostart).
                        let (app, host) = (app.clone(), host.clone());
                        let _ = tauri::async_runtime::spawn_blocking(move || sync_with_config(&app, &host)).await;
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

/// Give the webview time to flush, with a fallback if it cannot answer.
fn request_quit(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(window::MAIN) {
        let fallback = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(4)).await;
            fallback.exit(0);
        });
        if let Err(error) = window.eval("window.dispatchEvent(new Event('omni-quit'))")
            && let Some(host) = app.try_state::<Host>()
        {
            host.log(omni_core::LogLevel::Error, format!("Couldn't ask the window to save before quitting: {error}"));
        }
    } else {
        app.exit(0);
    }
}

fn sync_with_config(app: &AppHandle, host: &Host) {
    hotkeys::sync(app, host);
    let wanted = host.config().settings.launch_at_login;
    let autostart = app.autolaunch();
    let result = match autostart.is_enabled() {
        Ok(enabled) if enabled == wanted => Ok(()),
        _ if wanted => autostart.enable(),
        _ => autostart.disable(),
    };
    if let Err(e) = result {
        host.log(omni_core::LogLevel::Warn, format!("Couldn't change whether OmniPanel opens at login: {e}"));
    }
}
