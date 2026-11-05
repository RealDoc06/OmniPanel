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

/// Register exactly the accelerators the config uses. Called from setup, then
/// only from the event-forwarding task, so calls never overlap.
///
/// The plugin runs (un)registration on the main thread and waits for it, and
/// [`on_shortcut`] takes the `Registered` lock on the main thread (macOS), so
/// that lock is never held across a plugin call.
pub fn sync(app: &AppHandle, host: &Host) {
    let wanted = host.hotkeys();
    let shortcuts = app.global_shortcut();
    let registered = &app.state::<Registered>().inner().0;
    let is_wanted = |accelerator: &str| wanted.iter().any(|w| w.eq_ignore_ascii_case(accelerator));

    let stale: Vec<(u32, String)> = registered
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, accelerator)| !is_wanted(accelerator))
        .map(|(id, accelerator)| (*id, accelerator.clone()))
        .collect();
    for (id, accelerator) in stale {
        if let Ok(shortcut) = accelerator.parse::<Shortcut>() {
            let _ = shortcuts.unregister(shortcut);
        }
        registered.lock().unwrap().remove(&id);
    }
    for accelerator in &wanted {
        let known = registered.lock().unwrap().values().any(|r| r.eq_ignore_ascii_case(accelerator));
        if known {
            continue;
        }
        let display = accelerator
            .split('+')
            .map(|part| {
                if part.eq_ignore_ascii_case("CmdOrCtrl") || part.eq_ignore_ascii_case("CommandOrControl") {
                    if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" }
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join("+");
        let label = host
            .config()
            .profiles
            .iter()
            .flat_map(|p| p.controls())
            .find(|c| c.hotkey.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(accelerator)))
            .map(|c| c.style.label.clone())
            .filter(|label| !label.is_empty());
        let button = label.map(|label| format!(" for \"{label}\"")).unwrap_or_default();
        match accelerator.parse::<Shortcut>() {
            Ok(shortcut) => match shortcuts.register(shortcut) {
                Ok(()) => {
                    registered.lock().unwrap().insert(shortcut.id(), accelerator.clone());
                }
                Err(e) => {
                    tracing::warn!("shortcut {accelerator}: {e}");
                    host.log(LogLevel::Warn, format!("The keyboard shortcut {display}{button} couldn't be set up (another app may be using it). Choose a different one."));
                }
            },
            Err(e) => {
                tracing::warn!("shortcut {accelerator}: {e}");
                host.log(
                    LogLevel::Warn,
                    format!("The keyboard shortcut {display}{button} isn't valid. Choose a different one."),
                );
            }
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
