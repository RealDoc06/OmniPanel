//! End-to-end deck protocol over a real WebSocket against a running Host.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use omni_core::{
    Action, ClientInfo, Control, ControlKind, DeckToHost, HostEvent, HostToDeck, Interpreter, NavTarget, Style,
    ToastLevel, VoiceOp, new_id,
};
use omni_host::{Host, HostOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn start_host() -> (Host, u16) {
    let data_dir = std::env::temp_dir().join(format!("omni-host-test-{}", new_id()));
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut config = omni_core::Config::default();
    config.settings.port = port;
    config.settings.usb.enabled = false;
    omni_core::store::save(&data_dir, &config).unwrap();
    let host = Host::start(HostOptions { data_dir, version: "test".into() }).await.unwrap();
    (host, port)
}

async fn connect(port: u16) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await.unwrap();
    ws
}

async fn send(ws: &mut Ws, message: DeckToHost) {
    ws.send(Message::Text(serde_json::to_string(&message).unwrap().into())).await.unwrap();
}

async fn recv(ws: &mut Ws) -> HostToDeck {
    loop {
        let frame =
            tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("deck message").unwrap().unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Skip `state` pushes (they depend on timing) until another message arrives.
async fn recv_skip_state(ws: &mut Ws) -> HostToDeck {
    loop {
        match recv(ws).await {
            HostToDeck::State { .. } => continue,
            other => return other,
        }
    }
}

fn hello(token: Option<String>) -> DeckToHost {
    DeckToHost::Hello {
        token,
        client: ClientInfo {
            user_agent: "Mozilla/5.0 (Linux; Android 4.4.2; Nexus 7)".into(),
            ..ClientInfo::default()
        },
    }
}

#[tokio::test]
async fn pair_press_navigate_reconnect_unpair() {
    let (host, port) = start_host().await;
    let mut ws = connect(port).await;

    send(&mut ws, hello(None)).await;
    assert!(matches!(recv(&mut ws).await, HostToDeck::NeedPair { .. }));

    // pairing is off until the desktop starts it
    send(&mut ws, DeckToHost::Pair { code: "123456".into(), name: "Tablet".into() }).await;
    let HostToDeck::PairResult { ok: false, error: Some(error), .. } = recv(&mut ws).await else { panic!("paired") };
    assert_eq!(error, "Pairing is off. Click \"Connect a device\" in OmniPanel on your computer.");

    let code = host.start_pairing().code;
    tokio::time::sleep(Duration::from_millis(1100)).await; // pairing cooldown
    send(&mut ws, DeckToHost::Pair { code, name: "Kitchen tablet".into() }).await;
    let HostToDeck::PairResult { ok: true, token: Some(token), .. } = recv(&mut ws).await else {
        panic!("pairing failed")
    };
    assert!(matches!(recv(&mut ws).await, HostToDeck::Welcome { .. }));
    let HostToDeck::Layout { layout } = recv_skip_state(&mut ws).await else { panic!("expected layout") };
    assert_eq!(layout.page_index, 0);
    let device = host.config().devices[0].clone();
    assert_eq!(device.name, "Kitchen tablet");
    assert_eq!(device.platform, "Android tablet");

    // add a script button and a broken one: the deck gets the new layout
    let mut profile = host.config().profiles[0].clone();
    let script = Control {
        id: new_id(),
        x: 0,
        y: 2,
        w: 1,
        h: 1,
        style: Style { label: "Echo".into(), ..Style::default() },
        kind: ControlKind::Button {
            action: Some(Action::RunScript {
                interpreter: Interpreter::Shell,
                script: "echo hello".into(),
                show_output: true,
                timeout_secs: 5,
            }),
        },
        hotkey: None,
    };
    let broken = Control {
        id: new_id(),
        kind: ControlKind::Button {
            action: Some(Action::Voice { op: VoiceOp::SetPreset { preset_id: "nope".into() } }),
        },
        style: Style { label: "Broken".into(), ..Style::default() },
        x: 1,
        ..script.clone()
    };
    profile.pages[0].controls.retain(|c| c.y != 2);
    profile.pages[0].controls.extend([script.clone(), broken.clone()]);
    host.save_profile(profile).unwrap();
    let HostToDeck::Layout { layout } = recv_skip_state(&mut ws).await else { panic!("expected new layout") };
    assert!(layout.controls.iter().any(|c| c.id == script.id));

    send(&mut ws, DeckToHost::Press { control: script.id.clone() }).await;
    let HostToDeck::Toast { level: ToastLevel::Info, .. } = recv_skip_state(&mut ws).await else {
        panic!("expected toast")
    };
    send(&mut ws, DeckToHost::Press { control: broken.id.clone() }).await;
    let HostToDeck::Toast { level: ToastLevel::Error, text } = recv_skip_state(&mut ws).await else {
        panic!("expected error")
    };
    assert!(text.starts_with("Broken: "), "{text}");

    send(&mut ws, DeckToHost::Nav { to: NavTarget::NextPage }).await;
    let HostToDeck::Layout { layout } = recv_skip_state(&mut ws).await else { panic!("expected page 2") };
    assert_eq!(layout.page_index, 1);
    // no wrap past the last page
    send(&mut ws, DeckToHost::Nav { to: NavTarget::NextPage }).await;
    send(&mut ws, DeckToHost::Ping).await;
    assert!(matches!(recv_skip_state(&mut ws).await, HostToDeck::Pong));
    assert_eq!(host.runtime().sessions.len(), 1);
    drop(ws);

    // reconnect with the token: same device, last page restored
    let mut ws = connect(port).await;
    send(&mut ws, hello(Some(token))).await;
    assert!(matches!(recv(&mut ws).await, HostToDeck::Welcome { .. }));
    let HostToDeck::Layout { layout } = recv_skip_state(&mut ws).await else { panic!("expected layout") };
    assert_eq!(layout.page_index, 1);
    assert_eq!(host.config().devices.len(), 1);

    assert_eq!(layout.pages.len(), 2);

    // "Disconnect this device" on the deck removes it on the host too
    send(&mut ws, DeckToHost::Unpair).await;
    assert!(matches!(recv_skip_state(&mut ws).await, HostToDeck::Unpaired));
    assert!(host.config().device(&device.id).is_none());
    host.shutdown().await;
}

#[tokio::test]
async fn rejects_unknown_tokens_and_cross_site_sockets() {
    let (host, port) = start_host().await;
    let mut ws = connect(port).await;
    send(&mut ws, hello(Some("forged".into()))).await;
    assert!(matches!(recv(&mut ws).await, HostToDeck::NeedPair { .. }));

    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    request.headers_mut().insert("Origin", "https://evil.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_err());
    host.shutdown().await;
}

async fn pair_error(port: u16, code: &str) -> String {
    let mut ws = connect(port).await;
    send(&mut ws, DeckToHost::Pair { code: code.into(), name: "Tablet".into() }).await;
    let HostToDeck::PairResult { ok: false, error: Some(error), .. } = recv(&mut ws).await else { panic!("paired") };
    error
}

/// Wrong codes from many sockets count together (the per-socket cooldown
/// doesn't multiply guesses): the code rotates, the desktop gets the new
/// one, and pairing is locked for everyone until "Pair a device" again.
#[tokio::test]
async fn wrong_codes_lock_pairing_across_connections() {
    let (host, port) = start_host().await;
    let mut events = host.subscribe();
    let first = host.start_pairing().code;
    let wrong = if first == "000000" { "111111" } else { "000000" };
    let mut errors = vec![];
    for _ in 0..5 {
        errors.push(pair_error(port, wrong).await);
    }
    assert!(errors[0].starts_with("Wrong code."), "{}", errors[0]);
    assert!(errors[4].starts_with("Too many wrong codes."), "{}", errors[4]);

    let rotated = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(HostEvent::Runtime { runtime }) = events.recv().await
                && let Some(pairing) = runtime.pairing.filter(|p| p.code != first)
            {
                return pairing.code;
            }
        }
    })
    .await
    .expect("runtime event with the new code");
    assert!(pair_error(port, &rotated).await.starts_with("Too many wrong codes."));

    let code = host.start_pairing().code;
    let mut ws = connect(port).await;
    send(&mut ws, DeckToHost::Pair { code, name: "Tablet".into() }).await;
    assert!(matches!(recv(&mut ws).await, HostToDeck::PairResult { ok: true, .. }));
    host.shutdown().await;
}

/// `GET /api/info` with this Host header; returns the status code.
async fn http_status(port: u16, host: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!("GET /api/info HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or_default()
}

/// DNS rebinding: a site whose own name points at this machine must not
/// reach the deck server. IPs, localhost (USB) and the hostname do.
#[tokio::test]
async fn only_names_of_this_machine_reach_the_server() {
    let (host, port) = start_host().await;
    let socket_with_host = |name: &str| {
        let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
        request.headers_mut().insert("Host", format!("{name}:{port}").parse().unwrap());
        tokio_tungstenite::connect_async(request)
    };
    let hostname = gethostname::gethostname().to_string_lossy().split('.').next().unwrap_or_default().to_string();
    for name in ["127.0.0.1", "localhost", &hostname, &format!("{hostname}.local")] {
        assert_eq!(http_status(port, name).await, 200, "{name}");
        assert!(socket_with_host(name).await.is_ok(), "{name}");
    }
    assert_eq!(http_status(port, "rebind.evil.example").await, 403);
    assert!(socket_with_host("rebind.evil.example").await.is_err());
    host.shutdown().await;
}

#[tokio::test]
async fn device_and_computer_renames_reach_the_live_session() {
    let (host, port) = start_host().await;
    let mut ws = connect(port).await;
    send(&mut ws, hello(None)).await;
    assert!(matches!(recv(&mut ws).await, HostToDeck::NeedPair { .. }));
    send(&mut ws, DeckToHost::Pair { code: host.start_pairing().code, name: "Tablet".into() }).await;
    assert!(matches!(recv(&mut ws).await, HostToDeck::PairResult { ok: true, .. }));
    assert!(matches!(recv(&mut ws).await, HostToDeck::Welcome { .. }));
    assert!(matches!(recv_skip_state(&mut ws).await, HostToDeck::Layout { .. }));
    let device = host.config().devices[0].clone();
    host.update_device(&device.id, "Kitchen tablet".into(), device.profile_id, device.deck).unwrap();
    assert!(
        matches!(recv_skip_state(&mut ws).await, HostToDeck::Welcome { device_name, .. } if device_name == "Kitchen tablet")
    );
    host.patch_settings(omni_core::SettingsPatch { host_name: Some("Studio".into()), ..Default::default() }).unwrap();
    assert!(
        matches!(recv_skip_state(&mut ws).await, HostToDeck::Welcome { device_name, host_name, .. } if device_name == "Kitchen tablet" && host_name == "Studio")
    );
    ws.close(None).await.ok();
    host.shutdown().await;
}
