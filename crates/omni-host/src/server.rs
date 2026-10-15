//! HTTP + WebSocket server for decks: the deck client, `/ws`, `/img/<id>`, `/api/info`.

use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, State, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use omni_core::{PROTOCOL_VERSION, ServerStatus};
use rust_embed::RustEmbed;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::{Host, images, ws};

/// The deck client build (`pnpm build:deck`). Read from disk in debug builds.
#[derive(RustEmbed)]
#[folder = "../../apps/deck/dist"]
#[allow_missing = true]
struct DeckAssets;

#[derive(Default)]
pub struct ServerHandle {
    pub status: ServerStatus,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.status.running = false;
    }
}

impl Host {
    /// (Re)bind the deck server on `port`. Failures (port in use) end up in
    /// `runtime.server.error` instead of stopping the app.
    pub(crate) async fn restart_server(&self, port: u16) {
        self.close_all_sessions();
        self.server.lock().unwrap().stop();
        let status = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
            Ok(listener) => {
                let (tx, rx) = oneshot::channel::<()>();
                let app = router(self.clone()).into_make_service_with_connect_info::<SocketAddr>();
                let task = tokio::spawn(async move {
                    let shutdown = async {
                        let _ = rx.await;
                    };
                    if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
                        tracing::error!("deck server stopped: {e}");
                    }
                });
                let mut server = self.server.lock().unwrap();
                server.shutdown = Some(tx);
                server.task = Some(task);
                ServerStatus { running: true, port, urls: vec![], error: None }
            }
            Err(e) => {
                let error = if e.kind() == std::io::ErrorKind::AddrInUse {
                    format!("Port {port} is already in use. Pick another port in Settings.")
                } else {
                    format!("Can't start the deck server on port {port}: {e}")
                };
                self.log(omni_core::LogLevel::Error, error.clone());
                ServerStatus { running: false, port, urls: vec![], error: Some(error) }
            }
        };
        self.server.lock().unwrap().status = status;
        self.emit_runtime();
    }
}

fn router(host: Host) -> Router {
    Router::new()
        .route("/ws", get(websocket))
        .route("/img/{id}", get(image))
        .route("/api/info", get(info))
        .fallback(get(asset))
        .with_state(host)
}

async fn websocket(
    State(host): State<Host>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade.max_message_size(64 * 1024).on_upgrade(move |socket| ws::run(host, socket, addr)).into_response()
}

/// Blocks cross-site WebSocket hijacking: a page on another origin must not
/// talk to the deck server from the user's browser. Decks load the client
/// from this server, so their Origin matches Host.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) else { return true };
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
    let origin_host = origin.split("://").nth(1).unwrap_or(origin);
    origin_host.eq_ignore_ascii_case(host)
}

async fn image(State(host): State<Host>, Path(id): Path<String>) -> Response {
    let id = id.trim_end_matches(".png");
    let Some(path) = images::path(&host.data_dir, id) else { return StatusCode::NOT_FOUND.into_response() };
    match tokio::fs::read(path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("image/png")),
                (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable")),
                (header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*")),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn info(State(host): State<Host>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "app": "OmniPanel",
        "version": host.version,
        "hostName": host.host_name(),
        "protocol": PROTOCOL_VERSION,
    }))
}

/// Static deck client files with ETag revalidation (old devices on slow Wi-Fi).
async fn asset(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(file) = DeckAssets::get(path) else {
        if path == "index.html" {
            return (StatusCode::SERVICE_UNAVAILABLE, "Deck client not built. Run `pnpm build:deck`.").into_response();
        }
        return StatusCode::NOT_FOUND.into_response();
    };
    let etag = format!("\"{}\"", hex::encode(&file.metadata.sha256_hash()[..12]));
    if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let mime = if path.ends_with(".webmanifest") {
        "application/manifest+json".to_string()
    } else {
        mime_guess::from_path(path).first_or_octet_stream().to_string()
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(file.data.into_owned()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_check() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("192.168.1.5:7710"));
        assert!(same_origin(&headers));
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://192.168.1.5:7710"));
        assert!(same_origin(&headers));
        headers.insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert!(!same_origin(&headers));
    }
}
