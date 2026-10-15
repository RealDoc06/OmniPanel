//! Global shortcuts bound to controls (`Control::hotkey`).

use std::collections::HashMap;
use std::sync::Mutex;

use omni_core::LogLevel;
use omni_host::Host;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

/// Registered shortcut id -> accelerator as written in the config.
#[derive(Default)]
pub struct Registered(Mutex<HashMap<u32, String>>);

/// Register exactly the accelerators the config uses.
pub fn sync(app: &AppHandle, host: &Host) {
    let wanted = host.hotkeys();
    let shortcuts = app.global_shortcut();
    let state = app.state::<Registered>();
    let mut registered = state.0.lock().unwrap();

    registered.retain(|_, accelerator| {
        let keep = wanted.iter().any(|w| w.eq_ignore_ascii_case(accelerator));
        if !keep && let Ok(shortcut) = accelerator.parse::<Shortcut>() {
            let _ = shortcuts.unregister(shortcut);
        }
        keep
    });
    for accelerator in wanted {
        if registered.values().any(|r| r.eq_ignore_ascii_case(&accelerator)) {
            continue;
        }
        match accelerator.parse::<Shortcut>() {
            Ok(shortcut) => match shortcuts.register(shortcut) {
                Ok(()) => {
                    registered.insert(shortcut.id(), accelerator);
                }
                Err(e) => host.log(LogLevel::Warn, format!("Hotkey {accelerator} is unavailable: {e}")),
            },
            Err(e) => host.log(LogLevel::Warn, format!("Hotkey {accelerator} is invalid: {e}")),
        }
    }
}

pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state() != ShortcutState::Pressed {
        return;
    }
    let Some(accelerator) = app.state::<Registered>().0.lock().unwrap().get(&shortcut.id()).cloned() else { return };
    let Some(host) = app.try_state::<Host>().map(|h| h.inner().clone()) else { return };
    tauri::async_runtime::spawn(async move { host.trigger_hotkey(&accelerator).await });
}
