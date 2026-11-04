//! HTTP + WebSocket server for decks: the deck client, `/ws`, `/img/<id>`, `/api/info`.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request, State, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::{Extension, Router};
use omni_core::{PROTOCOL_VERSION, ServerStatus};
use rust_embed::RustEmbed;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::{Host, images, net, ws};

/// The deck client build (`pnpm build:deck`). Read from disk in debug builds.
#[derive(RustEmbed)]
#[folder = "../../apps/deck/dist"]
#[allow_missing = true]
struct DeckAssets;

/// Largest WebSocket message or frame a deck may send; its messages are tiny.
const MAX_MESSAGE: usize = 64 * 1024;
/// How long a stopping server waits for in-flight HTTP requests.
const STOP_TIMEOUT: Duration = Duration::from_secs(1);

/// The deck server's listener. Lives behind an async mutex so restarts and
/// shutdown run one at a time, in order.
#[derive(Default)]
pub struct ServerControl {
    running: Option<Running>,
    /// [`Host::shutdown`] ran: never listen again.
    closed: bool,
}

struct Running {
    port: u16,
    /// Dropping it stops the listener and closes this server's WebSockets.
    stop: watch::Sender<()>,
    task: JoinHandle<()>,
}

impl Running {
    /// Stop accepting, close this server's sockets and wait until the
    /// listener is gone, so its port can be bound again.
    async fn stop(self) {
        let Running { stop, mut task, .. } = self;
        drop(stop);
        if tokio::time::timeout(STOP_TIMEOUT, &mut task).await.is_err() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Host {
    /// (Re)bind the deck server on the configured port. Calls queue up and
    /// each one binds the latest port, so quick port edits can't leave an
    /// older one running. Failures (port in use) end up in
    /// `runtime.server.error` instead of stopping the app.
    pub(crate) async fn restart_server(&self) {
        let mut server = self.server.lock().await;
        let port = self.config().settings.port;
        if server.closed || server.running.as_ref().is_some_and(|r| r.port == port) {
            return;
        }
        self.close_all_sessions();
        if let Some(running) = server.running.take() {
            running.stop().await;
        }
        let bound = match port {
            // The OS would pick a random port that the shown URLs can't name.
            0 => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "pick a port from 1 to 65535")),
            _ => tokio::net::TcpListener::bind(("0.0.0.0", port)).await,
        };
        let status = match bound {
            Ok(listener) => {
                let (stop, mut stopped) = watch::channel(());
                let app = router(self.clone(), stopped.clone()).into_make_service_with_connect_info::<SocketAddr>();
                let task = tokio::spawn(async move {
                    let shutdown = async move {
                        let _ = stopped.changed().await;
                    };
                    if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
                        tracing::error!("deck server stopped: {e}");
                    }
                });
                server.running = Some(Running { port, stop, task });
                self.log(omni_core::LogLevel::Info, format!("Ready for devices on port {port}"));
                ServerStatus { running: true, port, urls: vec![], error: None }
            }
            Err(e) => {
                let error = if e.kind() == std::io::ErrorKind::AddrInUse {
                    format!("Port {port} is already in use. Pick another port in Settings.")
                } else {
                    format!("Devices can't connect on port {port}: {e}")
                };
                self.log(omni_core::LogLevel::Error, error.clone());
                ServerStatus { running: false, port, urls: vec![], error: Some(error) }
            }
        };
        *self.server_status.lock().unwrap() = status;
        self.emit_runtime();
    }

    /// Stop the server for good: pending and later restarts do nothing.
    pub(crate) async fn stop_server(&self) {
        let mut server = self.server.lock().await;
        server.closed = true;
        if let Some(running) = server.running.take() {
            running.stop().await;
        }
        self.server_status.lock().unwrap().running = false;
    }
}

/// `stop` resolves when this server instance stops; its WebSockets close then.
fn router(host: Host, stop: watch::Receiver<()>) -> Router {
    let names: Arc<[String]> = net::local_names().into();
    Router::new()
        .route("/ws", get(websocket))
        .route("/img/{id}", get(image))
        .route("/api/info", get(info))
        .fallback(get(asset))
        .layer(Extension(stop))
        .layer(middleware::from_fn_with_state(names, this_machine_only))
        .with_state(host)
}

async fn websocket(
    State(host): State<Host>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(stop): Extension<watch::Receiver<()>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| ws::run(host, socket, addr, stop))
        .into_response()
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

/// Blocks DNS rebinding: a site that points its own name at this machine
/// passes the Origin check, but its name is in the Host header. Decks use
/// an IP address, `localhost` (USB) or this machine's name on a private domain.
async fn this_machine_only(State(names): State<Arc<[String]>>, request: Request, next: Next) -> Response {
    let host = request.headers().get(header::HOST).and_then(|h| h.to_str().ok());
    let host = host.or_else(|| request.uri().authority().map(|a| a.as_str()));
    if !host.is_some_and(|h| names_this_machine(h, &names)) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

/// Accept IP literals, known names and this machine's short names on private domains.
fn names_this_machine(host: &str, names: &[String]) -> bool {
    let name = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if name.parse::<IpAddr>().is_ok() || names.contains(&name) {
        return true;
    }
    let Some((short, domain)) = name.split_once('.') else { return false };
    short != "localhost"
        && names.iter().any(|n| n == short)
        && ["ts.net", "local", "lan", "home", "home.arpa", "internal", "localdomain", "fritz.box"]
            .iter()
            .any(|suffix| domain == *suffix || domain.strip_suffix(suffix).is_some_and(|prefix| prefix.ends_with('.')))
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
        // Another site framing a paired deck could trick the user into pressing its buttons.
        .header(header::X_FRAME_OPTIONS, "DENY")
        .header(header::CONTENT_SECURITY_POLICY, "frame-ancestors 'none'")
        .body(Body::from(file.data.into_owned()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn port_zero_is_refused_instead_of_binding_a_random_port() {
        let host = crate::tests::start_host().await;
        host.restart_server().await;
        let mut settings = host.config().settings.clone();
        settings.port = 0;
        host.set_settings(settings).unwrap();
        let server = loop {
            let server = host.runtime().server;
            if !server.running {
                break server;
            }
            tokio::task::yield_now().await;
        };
        assert!(server.error.unwrap().contains("pick a port"));
        host.shutdown().await;
    }

    #[tokio::test]
    async fn deck_pages_refuse_framing() {
        let response = asset(Uri::from_static("/"), HeaderMap::new()).await;
        // Without `pnpm build:deck` there is no page to protect.
        if response.status() == StatusCode::OK {
            assert_eq!(response.headers()[header::X_FRAME_OPTIONS], "DENY");
        }
    }

    #[test]
    fn only_this_machine_names_pass() {
        let names = ["localhost", "studio", "studio.local", "bonjour-name", "bonjour-name.local"].map(String::from);
        for host in [
            "192.168.1.5:7710",
            "10.0.0.2",
            "[::1]:7710",
            "localhost:7710",
            "Studio.local:7710",
            "studio.",
            "studio.tailnet.ts.net:7710",
            "Studio.Fritz.Box:7710",
            "studio.home.arpa.",
            "studio.lan",
            "studio.home",
            "studio.localdomain",
            "studio.internal",
            "bonjour-name.tailnet.ts.net",
            "bonjour-name.home.arpa",
        ] {
            assert!(names_this_machine(host, &names), "{host}");
        }
        for host in [
            "evil.example:7710",
            "studio.evil.example",
            "localhost.evil.example",
            "",
            "::1",
            "192.168.1.5.nip.io",
            "other.tailnet.ts.net",
            "studio.evilts.net",
            "studio.ts.net.evil.example",
            "studio.evillan",
            "localhost.tailnet.ts.net",
            "bonjour-name.evil.example",
        ] {
            assert!(!names_this_machine(host, &names), "{host}");
        }
    }

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
