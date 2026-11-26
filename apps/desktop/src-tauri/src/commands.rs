//! Tauri commands. Everything engine-related goes through `host`, which runs
//! on the async runtime; `apps/desktop/src/lib/api.ts` is the typed client.
//! The update commands live in `updater.rs`.

use omni_core::HostRequest;
use omni_host::Host;
use tauri::{AppHandle, State};

use crate::updater;

pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        host,
        quit_app,
        updater::update_status,
        updater::check_for_update,
        updater::download_update,
        updater::install_update
    ]
}

#[tauri::command]
async fn host(host: State<'_, Host>, request: HostRequest) -> Result<serde_json::Value, String> {
    host.handle(request).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    crate::finish_quit(&app);
}
