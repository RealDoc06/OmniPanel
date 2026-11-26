//! App updates on top of `tauri-plugin-updater`: a daily check, a status for the
//! tray and the Settings screen, and download / install when the user asks.
//! The check runs here, not in the UI, because the webview is gone while
//! OmniPanel sits in the tray. `apps/desktop/src/lib/updates.ts` is the UI side.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use omni_host::Host;
use serde::Serialize;
use tauri::utils::config::BundleType;
use tauri::utils::platform::bundle_type;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Error, Update, Updater, UpdaterExt};

use crate::{tray, window};

const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the loop looks whether a check is due. Also the wait after a
/// failed check (offline at login) and after checks are turned on.
const CHECK_TICK: Duration = Duration::from_secs(60 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// File in the data folder, written before an install: the next start shows
/// the window even when it would start hidden, so the user sees the app come
/// back. Empty, or what went wrong when the app restarted without updating.
const AFTER_UPDATE_NOTE: &str = "after-update";
/// How long an exit that can't be held back waits for a running install.
const EXIT_WAIT: Duration = Duration::from_secs(10);

/// Pushed to the UI as `update-status`; `UpdateStatus` in `lib/backend.ts` mirrors it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Status {
    /// Not checked yet.
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// `can_install` is false when this copy can't replace itself (see `can_install`):
    /// the user is told and downloads the new package.
    Available {
        version: String,
        can_install: bool,
    },
    Downloading {
        version: String,
        percent: Option<u8>,
    },
    /// Downloaded and signature-checked, waiting for `install_update`. Only
    /// while the window is open: it is the window that finishes the install.
    Ready {
        version: String,
    },
    /// The app's files are being replaced; a restart follows.
    Installing {
        version: String,
    },
}

#[derive(Default)]
pub struct Updates {
    inner: Mutex<Inner>,
    /// True while the app's files are being replaced: quitting is held back
    /// (`is_replacing`), so it can't cut a half-written app short.
    replacing: AtomicBool,
    /// Windows: the engine was shut down for an installer (see `updater`).
    engine_stopped: AtomicBool,
}

#[derive(Default)]
struct Inner {
    status: Status,
    /// A check is in flight. Its own flag, because a standing offer keeps its
    /// status while it is refreshed.
    checking: bool,
    /// The release behind every status that carries a version.
    update: Option<Update>,
    /// Its verified package, while `Ready`.
    package: Option<Vec<u8>>,
}

/// Checks at launch, then once a day while `settings.check_for_updates` is on.
/// The day is wall-clock time: timers stand still while the computer sleeps.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut checked_at: Option<SystemTime> = None;
        loop {
            let due = checked_at.and_then(|at| at.elapsed().ok()).is_none_or(|age| age >= CHECK_INTERVAL);
            let enabled = app.state::<Host>().config().settings.check_for_updates;
            if due && enabled && matches!(check(&app).await, Ok(true)) {
                checked_at = Some(SystemTime::now());
            }
            tokio::time::sleep(CHECK_TICK).await;
        }
    });
}

/// `Some` once after an install was started: this start must show the window.
/// The note is empty, or says why the app is back without the update.
pub fn take_after_update_note(data_dir: &Path) -> Option<String> {
    let path = data_dir.join(AFTER_UPDATE_NOTE);
    let note = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(path);
    Some(note)
}

/// True while an install is replacing the app's files. Exits wait for it.
pub fn is_replacing(app: &AppHandle) -> bool {
    app.state::<Updates>().replacing.load(Ordering::SeqCst)
}

/// For an exit that can't be held back (macOS Dock Quit goes straight to
/// `RunEvent::Exit`): gives a running install a moment to finish. Bounded,
/// because an install that needs the macOS admin prompt waits for this thread;
/// one stuck there has not touched the installed app yet.
pub fn wait_for_install(app: &AppHandle) {
    let deadline = Instant::now() + EXIT_WAIT;
    while is_replacing(app) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The window that would finish the install is gone: let go of the downloaded
/// package (tens of MB) and offer the update again next time.
pub fn on_window_closed(app: &AppHandle) {
    let mut inner = state(app);
    if let Status::Ready { version } = inner.status.clone() {
        inner.package = None;
        inner.status = Status::Available { version, can_install: true };
    }
}

/// deb and rpm installs belong to the package manager (root, dependencies), and a
/// dev build has no installed app to replace. Those are told about the update only.
fn can_install() -> bool {
    !tauri::is_dev()
        && match bundle_type() {
            Some(BundleType::AppImage | BundleType::Nsis) => true,
            // Reported for every macOS build. Outside a bundle the updater would
            // replace the folder the binary sits in.
            Some(BundleType::App) => in_app_bundle(),
            _ => false,
        }
}

/// The executable is `<name>.app/Contents/MacOS/<binary>`.
fn in_app_bundle() -> bool {
    let Ok(exe) = tauri::utils::platform::current_exe() else { return false };
    let mut above = exe.ancestors().skip(1);
    above.next().is_some_and(|dir| dir.ends_with("MacOS"))
        && above.next().is_some_and(|dir| dir.ends_with("Contents"))
        && above.next().is_some_and(|dir| dir.extension() == Some("app".as_ref()))
}

fn updater(app: &AppHandle) -> Result<Updater, Error> {
    let handle = app.clone();
    app.updater_builder()
        .timeout(CHECK_TIMEOUT)
        // Windows starts the installer and exits inside `Update::install`,
        // skipping `RunEvent::Exit`. The plugin runs this just before it
        // starts the installer, and that start can still fail.
        .on_before_exit(move || {
            handle.state::<Updates>().engine_stopped.store(true, Ordering::SeqCst);
            crate::shutdown_engine(&handle);
            handle.cleanup_before_exit();
        })
        .build()
}

fn state(app: &AppHandle) -> MutexGuard<'_, Inner> {
    app.state::<Updates>().inner().inner.lock().unwrap()
}

fn set_status(app: &AppHandle, status: Status) {
    state(app).status = status.clone();
    publish(app, &status);
}

/// Tells the window, if there is one.
fn publish(app: &AppHandle, status: &Status) {
    if app.get_webview_window(window::MAIN).is_some() {
        let _ = app.emit("update-status", status);
    }
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> Status {
    state(&app).status.clone()
}

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<(), String> {
    check(&app).await.map(drop)
}

/// Asks the update server once. `Ok(false)` when the answer wasn't used: a check,
/// a download or an install was under way, or the user started on the offer meanwhile. On failure the status goes back to what
/// it was and the error is a message for the user.
async fn check(app: &AppHandle) -> Result<bool, String> {
    let previous = {
        let mut inner = state(app);
        if inner.checking || !matches!(inner.status, Status::Idle | Status::UpToDate | Status::Available { .. }) {
            return Ok(false);
        }
        // Claimed under the lock, so two checks can't both start.
        inner.checking = true;
        let previous = inner.status.clone();
        // A standing offer stays up, and installable, while it is refreshed.
        if !matches!(previous, Status::Available { .. }) {
            inner.status = Status::Checking;
        }
        previous
    };
    if !matches!(previous, Status::Available { .. }) {
        publish(app, &Status::Checking);
    }
    let found = async { updater(app)?.check().await }.await;
    let (changed, result) = {
        let mut inner = state(app);
        inner.checking = false;
        // False when the user started on the offer (or its download failed) meanwhile: that stands.
        let untouched = inner.status == Status::Checking || inner.status == previous;
        match found {
            Ok(update) if untouched => {
                inner.status = match &update {
                    Some(update) => Status::Available { version: update.version.clone(), can_install: can_install() },
                    None => Status::UpToDate,
                };
                inner.update = update;
                (Some(inner.status.clone()), Ok(true))
            }
            // Thrown away, so nothing was learned: the next tick asks again.
            Ok(_) => (None, Ok(false)),
            Err(e) => {
                tracing::info!("update check failed: {e}");
                let restored = (inner.status == Status::Checking).then(|| previous.clone());
                if let Some(previous) = &restored {
                    inner.status = previous.clone();
                }
                (restored, Err(format!("Couldn't check for updates: {e}")))
            }
        }
    };
    if let Some(status) = &changed {
        if result.is_ok() {
            tray::set_update(
                app,
                match status {
                    Status::Available { version, .. } => Some(version.as_str()),
                    _ => None,
                },
            );
        }
        publish(app, status);
    }
    result
}

/// Downloads the available update and checks its signature.
#[tauri::command]
pub async fn download_update(app: AppHandle) -> Result<(), String> {
    let (mut update, version) = {
        let mut inner = state(&app);
        let (Status::Available { version, can_install: true }, Some(update)) =
            (inner.status.clone(), inner.update.clone())
        else {
            return Err("There is no update to install.".into());
        };
        // Claimed under the lock, so two downloads can't both start.
        inner.status = Status::Downloading { version: version.clone(), percent: None };
        (update, version)
    };
    publish(&app, &Status::Downloading { version: version.clone(), percent: None });
    update.timeout = Some(DOWNLOAD_TIMEOUT);
    let (mut received, mut shown) = (0u64, None);
    let on_chunk = |chunk: usize, total: Option<u64>| {
        received += chunk as u64;
        let percent = total.filter(|total| *total > 0).map(|total| (received * 100 / total).min(100) as u8);
        if percent != shown {
            shown = percent;
            set_status(&app, Status::Downloading { version: version.clone(), percent });
        }
    };
    let downloaded = update.download(on_chunk, || {}).await;
    let (status, result) = {
        let mut inner = state(&app);
        let result = match downloaded {
            // Kept only for a window that is still there to finish the install (`on_window_closed`).
            Ok(package) if app.get_webview_window(window::MAIN).is_some() => {
                inner.package = Some(package);
                inner.status = Status::Ready { version };
                Ok(())
            }
            Ok(_) => {
                inner.status = Status::Available { version, can_install: true };
                Ok(())
            }
            // The offer may be what is wrong (a package replaced since the check):
            // drop it, so the next "Check now" fetches a fresh one.
            Err(e) => {
                inner.status = Status::Idle;
                inner.update = None;
                let refused = matches!(
                    e,
                    Error::Minisign(_)
                        | Error::Base64(_)
                        | Error::SignatureUtf8(_)
                        | Error::SignedVersionMismatch { .. }
                        | Error::MissingSignedVersion
                );
                let what = if refused {
                    "The update failed its signature check and was not installed"
                } else {
                    "Couldn't download the update"
                };
                Err(format!("{what}: {e}"))
            }
        };
        (inner.status.clone(), result)
    };
    if status == Status::Idle {
        tray::set_update(&app, None);
    }
    publish(&app, &status);
    result
}

/// Replaces the app with the downloaded package and restarts into it. The UI
/// saves pending edits first. On Windows the installer takes over and the
/// process exits inside `Update::install`; everywhere else, and when the
/// install fails, `end_after_install` decides how this ends.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let (update, package, version) = {
        let mut inner = state(&app);
        let (Status::Ready { version }, Some(update), Some(package)) =
            (inner.status.clone(), inner.update.clone(), inner.package.take())
        else {
            return Err("There is no update to install.".into());
        };
        inner.status = Status::Installing { version: version.clone() };
        // With the claim, before the worker starts: no exit slips in between.
        app.state::<Updates>().replacing.store(true, Ordering::SeqCst);
        (update, package, version)
    };
    publish(&app, &Status::Installing { version: version.clone() });
    let note = app.state::<Host>().data_dir().join(AFTER_UPDATE_NOTE);
    let _ = std::fs::write(&note, "");
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(package)).await;
    let updates = app.state::<Updates>();
    updates.replacing.store(false, Ordering::SeqCst);
    let failure = match installed {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(format!("Couldn't install the update: {e}")),
        Err(e) => Some(format!("Couldn't install the update: {e}")),
    };
    // Windows: the installer didn't start, after the engine was shut down for it.
    // Restarting brings this version back whole, and the note says what happened.
    let engine_stopped = failure.is_some() && updates.engine_stopped.load(Ordering::SeqCst);
    if let (Some(problem), true) = (&failure, engine_stopped) {
        let _ = std::fs::write(&note, problem);
    }
    if !crate::end_after_install(&app, failure.is_none() || engine_stopped) {
        let _ = std::fs::remove_file(&note);
    }
    match failure {
        Some(problem) if !engine_stopped => {
            set_status(&app, Status::Available { version, can_install: true });
            Err(problem)
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, to_value};

    use super::Status;

    /// `UpdateStatus` in `lib/backend.ts` is written by hand against this shape.
    #[test]
    fn status_serializes_as_the_ui_expects() {
        let version = String::from("1.2.3");
        assert_eq!(to_value(Status::UpToDate).unwrap(), json!({"state": "upToDate"}));
        assert_eq!(
            to_value(Status::Available { version: version.clone(), can_install: false }).unwrap(),
            json!({"state": "available", "version": "1.2.3", "canInstall": false})
        );
        assert_eq!(
            to_value(Status::Downloading { version: version.clone(), percent: None }).unwrap(),
            json!({"state": "downloading", "version": "1.2.3", "percent": null})
        );
        assert_eq!(
            to_value(Status::Ready { version: version.clone() }).unwrap(),
            json!({"state": "ready", "version": "1.2.3"})
        );
        assert_eq!(
            to_value(Status::Installing { version }).unwrap(),
            json!({"state": "installing", "version": "1.2.3"})
        );
    }
}
