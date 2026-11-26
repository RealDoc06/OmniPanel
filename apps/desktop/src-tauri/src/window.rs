//! Main window lifecycle. The window is created on demand and destroyed on
//! close, so a backgrounded OmniPanel holds no webview in memory.

use omni_host::Host;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const MAIN: &str = "main";

/// Show the main window, creating it when needed. Runs on the main thread
/// (inline when already there), so callers on other threads (single-instance
/// relaunch) and the tray / Reopen can't both pass the check and create two.
pub fn show(app: &AppHandle) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || show_now(&handle)) {
        tracing::error!("can't open the window: {e}");
    }
}

fn show_now(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::default())
        .title("OmniPanel")
        .inner_size(1200.0, 780.0)
        .min_inner_size(900.0, 580.0)
        .theme(Some(tauri::Theme::Dark))
        .background_color(tauri::webview::Color(0, 0, 0, 255))
        .build();
    match built {
        Ok(window) => {
            let _ = window.set_focus();
            set_dock_visible(app, true);
            if let Some(host) = app.try_state::<Host>() {
                host.set_ui_active(true);
            }
        }
        Err(e) => tracing::error!("can't open the window: {e}"),
    }
}

pub fn on_destroyed(app: &AppHandle) {
    crate::updater::on_window_closed(app);
    if let Some(host) = app.try_state::<Host>() {
        host.set_ui_active(false);
    }
    set_dock_visible(app, false);
}

/// macOS: show the Dock icon only while the window is open.
pub fn set_dock_visible(app: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        let policy = if visible { tauri::ActivationPolicy::Regular } else { tauri::ActivationPolicy::Accessory };
        let _ = app.set_activation_policy(policy);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
}
