//! App updates on top of `tauri-plugin-updater`: a daily check, a status for the
//! tray and the Settings screen, and download / install when the user asks.
//! The check runs here, not in the UI, because the webview is gone while
//! OmniPanel sits in the tray. `apps/desktop/src/lib/updates.ts` is the UI side.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use omni_host::Host;
use serde::Serialize;
use tauri::utils::config::BundleType;
use tauri::utils::platform::bundle_type;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Error, Update, Updater, UpdaterExt};

use crate::{tray, window};

const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// After a failed check (offline at login), and while checks are turned off.
const RECHECK_SOON: Duration = Duration::from_secs(60 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

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
    /// Downloaded and signature-checked, waiting for `install_update`.
    Ready {
        version: String,
    },
}

#[derive(Default)]
pub struct Updates(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    status: Status,
    /// The release behind `Available`, `Downloading` and `Ready`.
    update: Option<Update>,
    /// Its verified package, while `Ready`.
    package: Option<Vec<u8>>,
}

/// Checks at launch, then once a day while `settings.check_for_updates` is on.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let enabled = app.state::<Host>().config().settings.check_for_updates;
            let checked = enabled && check_for_update(app.clone()).await.is_ok();
            tokio::time::sleep(if checked { CHECK_INTERVAL } else { RECHECK_SOON }).await;
        }
    });
}

/// deb and rpm installs belong to the package manager (root, dependencies), and a
/// dev build has no installed app to replace. Those are told about the update only.
fn can_install() -> bool {
    !tauri::is_dev()
        && matches!(bundle_type(), Some(BundleType::AppImage | BundleType::Nsis | BundleType::Msi | BundleType::App))
}

fn updater(app: &AppHandle) -> Result<Updater, Error> {
    let handle = app.clone();
    app.updater_builder()
        .timeout(CHECK_TIMEOUT)
        // Windows starts the installer and exits inside `Update::install`,
        // skipping `RunEvent::Exit`.
        .on_before_exit(move || {
            crate::shutdown_engine(&handle);
            handle.cleanup_before_exit();
        })
        .build()
}

fn lock(app: &AppHandle) -> MutexGuard<'_, Inner> {
    app.state::<Updates>().inner().0.lock().unwrap()
}

fn set_status(app: &AppHandle, status: Status) {
    lock(app).status = status.clone();
    if app.get_webview_window(window::MAIN).is_some() {
        let _ = app.emit("update-status", &status);
    }
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> Status {
    lock(&app).status.clone()
}

/// Asks the update server once. On failure the status goes back to what it was
/// and the error is a message for the user. Does nothing while busy.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<(), String> {
    let previous = {
        let mut inner = lock(&app);
        if matches!(inner.status, Status::Checking | Status::Downloading { .. } | Status::Ready { .. }) {
            return Ok(());
        }
        std::mem::take(&mut inner.status)
    };
    set_status(&app, Status::Checking);
    let found = async { updater(&app)?.check().await }.await;
    let (status, result) = match found {
        Ok(update) => {
            let version = update.as_ref().map(|update| update.version.clone());
            tray::set_update(&app, version.as_deref());
            lock(&app).update = update;
            let status = match version {
                Some(version) => Status::Available { version, can_install: can_install() },
                None => Status::UpToDate,
            };
            (status, Ok(()))
        }
        Err(e) => {
            tracing::info!("update check failed: {e}");
            (previous, Err(format!("Couldn't check for updates: {e}")))
        }
    };
    set_status(&app, status);
    result
}

/// Downloads the available update and checks its signature.
#[tauri::command]
pub async fn download_update(app: AppHandle) -> Result<(), String> {
    let (mut update, version) = {
        let mut inner = lock(&app);
        let (Status::Available { version, can_install: true }, Some(update)) =
            (inner.status.clone(), inner.update.clone())
        else {
            return Err("There is no update to install.".into());
        };
        inner.status = Status::Downloading { version: version.clone(), percent: None };
        (update, version)
    };
    set_status(&app, Status::Downloading { version: version.clone(), percent: None });
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
    match update.download(on_chunk, || {}).await {
        Ok(package) => {
            lock(&app).package = Some(package);
            set_status(&app, Status::Ready { version });
            Ok(())
        }
        Err(e) => {
            set_status(&app, Status::Available { version, can_install: true });
            let refused =
                matches!(e, Error::Minisign(_) | Error::SignedVersionMismatch { .. } | Error::MissingSignedVersion);
            let what = if refused {
                "The update failed its signature check and was not installed"
            } else {
                "Couldn't download the update"
            };
            Err(format!("{what}: {e}"))
        }
    }
}

/// Replaces the app with the downloaded package and restarts into it. The UI
/// saves pending edits first. On Windows the installer takes over and the
/// process exits inside `Update::install`.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let (update, package, version) = {
        let mut inner = lock(&app);
        let (Status::Ready { version }, Some(update)) = (inner.status.clone(), inner.update.clone()) else {
            return Err("There is no update to install.".into());
        };
        // Already installing.
        let Some(package) = inner.package.take() else {
            return Ok(());
        };
        (update, package, version)
    };
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(package)).await;
    match installed.map_err(|e| e.to_string()).and_then(|result| result.map_err(|e| e.to_string())) {
        Ok(()) => {
            app.request_restart();
            Ok(())
        }
        Err(e) => {
            set_status(&app, Status::Available { version, can_install: true });
            Err(format!("Couldn't install the update: {e}"))
        }
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
        assert_eq!(to_value(Status::Ready { version }).unwrap(), json!({"state": "ready", "version": "1.2.3"}));
    }
}
