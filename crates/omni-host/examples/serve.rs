//! Run the engine without the desktop app:
//! `cargo run -p omni-host --example serve -- [--data-dir DIR] [--port N] [--bridge N]`
//!
//! Prints the deck URLs and a pairing code, and logs host events. `--port`
//! sets the deck server port in the config. `--bridge N` also serves the
//! desktop UI's API on 127.0.0.1:N (POST /invoke, SSE /events) so the UI runs
//! in a browser against this engine: open `http://localhost:1420/?bridge=http://127.0.0.1:N`
//! (`pnpm --filter @omni/desktop dev`). Dev only, never shipped.

use std::convert::Infallible;
use std::path::PathBuf;

use axum::extract::Request;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::Stream;
use omni_core::{HostEvent, HostRequest, store};
use omni_host::{Host, HostOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut data_dir = std::env::temp_dir().join("omnipanel-dev");
    let (mut port, mut bridge) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| anyhow::anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--data-dir" => data_dir = PathBuf::from(value()?),
            "--port" => port = Some(value()?.parse::<u16>()?),
            "--bridge" => bridge = Some(value()?.parse::<u16>()?),
            other => data_dir = PathBuf::from(other), // legacy positional data dir
        }
    }
    if let Some(port) = port {
        let mut config = store::load(&data_dir)?;
        config.settings.port = port;
        store::save(&data_dir, &config)?;
    }

    let host =
        Host::start(HostOptions { data_dir: data_dir.clone(), version: env!("CARGO_PKG_VERSION").into() }).await?;
    let runtime = host.runtime();
    println!("data dir: {}", data_dir.display());
    match &runtime.server.error {
        Some(error) => println!("server error: {error}"),
        None => println!("deck urls: http://localhost:{} {}", runtime.server.port, runtime.server.urls.join(" ")),
    }
    if let Some(bridge) = bridge {
        host.set_ui_active(true);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", bridge)).await?;
        let app = Router::new()
            .route("/invoke", post(invoke).options(preflight))
            .route("/events", get(events))
            .layer(axum::middleware::from_fn(local_origins_only))
            .with_state(host.clone());
        tokio::spawn(async move { axum::serve(listener, app).await });
        println!("ui bridge: http://127.0.0.1:{bridge} (open the UI with ?bridge=http://127.0.0.1:{bridge})");
    } else {
        println!("pairing code: {}", host.start_pairing().code);
    }

    let mut events = host.subscribe();
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(HostEvent::Paired { device }) => println!("paired {} ({})", device.name, device.platform),
                Ok(HostEvent::Log { entry }) => println!("[{:?}] {}", entry.level, entry.message),
                Ok(_) | Err(_) => {}
            },
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    host.shutdown().await;
    Ok(())
}

async fn invoke(State(host): State<Host>, Json(request): Json<HostRequest>) -> Response {
    match host.handle(request).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    }
}

async fn events(State(host): State<Host>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = futures_util::stream::unfold(host.subscribe(), |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    return Some((Ok(Event::default().data(data)), rx));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// The bridge can run scripts, so only pages served from this machine may use it.
async fn local_origins_only(request: Request, next: Next) -> Response {
    let origin = request.headers().get(header::ORIGIN).cloned();
    let local = origin.as_ref().and_then(|o| o.to_str().ok()).is_none_or(|o| {
        let host = o.split("://").nth(1).unwrap_or_default().split(':').next().unwrap_or_default();
        matches!(host, "localhost" | "127.0.0.1")
    });
    if !local {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(request).await;
    if let Some(origin) = origin {
        let headers = response.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("content-type"));
        headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("POST, OPTIONS"));
    }
    response
}
