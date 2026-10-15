//! Tauri commands. Everything engine-related goes through `host`, which runs
//! on the async runtime; `apps/desktop/src/lib/api.ts` is the typed client.

use omni_core::HostRequest;
use omni_host::Host;
use tauri::{AppHandle, State};

pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![host, quit_app]
}

#[tauri::command]
async fn host(host: State<'_, Host>, request: HostRequest) -> Result<serde_json::Value, String> {
    host.handle(request).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}
