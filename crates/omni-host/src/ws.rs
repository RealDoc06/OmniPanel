//! One deck WebSocket connection: authentication, pairing, then input.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use omni_core::{ClientInfo, DeckToHost, Device, HostEvent, HostToDeck, LogLevel, now_ms};
use tokio::sync::mpsc;

use crate::Host;
use crate::pairing::{self, Verify};
use crate::sessions::{DeckTx, Outbound, send};

/// Decks ping every 20 s; silence longer than this means the socket is dead
/// (phone went to sleep, Wi-Fi dropped).
const IDLE_TIMEOUT: Duration = Duration::from_secs(75);
/// Minimum time between pairing attempts on one connection.
const PAIR_COOLDOWN: Duration = Duration::from_secs(1);

pub async fn run(host: Host, socket: WebSocket, addr: SocketAddr) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Outbound>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let sent = match frame {
                Outbound::Text(text) => sink.send(Message::Text(text.as_ref().into())).await,
                Outbound::Close => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            };
            if sent.is_err() {
                break;
            }
        }
    });

    let mut conn = Connection { host: host.clone(), tx, addr, session: None, client: None, last_pair: None };
    while let Ok(Some(Ok(message))) = tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await {
        match message {
            Message::Text(text) => {
                if let Ok(message) = serde_json::from_str::<DeckToHost>(&text) {
                    conn.handle(message);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    if let Some(id) = conn.session {
        host.unregister_session(id);
    }
    drop(conn);
    writer.abort();
}

struct Connection {
    host: Host,
    tx: DeckTx,
    addr: SocketAddr,
    session: Option<u64>,
    client: Option<ClientInfo>,
    last_pair: Option<Instant>,
}

impl Connection {
    fn handle(&mut self, message: DeckToHost) {
        match message {
            DeckToHost::Ping => send(&self.tx, &HostToDeck::Pong),
            DeckToHost::Hello { token, client } => self.hello(token, client),
            DeckToHost::Pair { code, name } => self.pair(&code, &name),
            DeckToHost::Press { control } => {
                if let Some(session) = self.session {
                    let host = self.host.clone();
                    tokio::spawn(async move { host.press(session, &control).await });
                }
            }
            DeckToHost::Slide { control, value } => {
                if let Some(session) = self.session {
                    self.host.slide(session, &control, value);
                }
            }
            DeckToHost::Nav { to } => {
                if let Some(session) = self.session {
                    self.host.navigate(&[session], &to);
                }
            }
            DeckToHost::Unpair => {
                let device = self
                    .session
                    .and_then(|id| self.host.sessions.lock().unwrap().by_id.get(&id).map(|s| s.device_id.clone()));
                if let Some(device) = device {
                    self.host.log(LogLevel::Info, "A deck unpaired itself");
                    let _ = self.host.unpair_device(&device);
                }
            }
            DeckToHost::Resize { width, height } => {
                if let Some(client) = &mut self.client {
                    (client.width, client.height) = (width, height);
                }
                if let Some(session) = self.session {
                    self.host.resize_session(session, width, height);
                }
            }
        }
    }

    fn hello(&mut self, token: Option<String>, client: ClientInfo) {
        if self.session.is_some() {
            return;
        }
        let device_id = token.map(|t| pairing::hash_token(&t)).and_then(|hash| {
            let config = self.host.config();
            config.devices.iter().find(|d| d.token_hash == hash).map(|d| d.id.clone())
        });
        match device_id {
            Some(device_id) => {
                self.session = Some(self.host.register_session(&device_id, self.tx.clone(), self.addr, &client));
            }
            None => send(&self.tx, &HostToDeck::NeedPair { host_name: self.host.host_name() }),
        }
        self.client = Some(client);
    }

    fn pair(&mut self, code: &str, name: &str) {
        if self.session.is_some() {
            return;
        }
        if self.last_pair.is_some_and(|at| at.elapsed() < PAIR_COOLDOWN) {
            return self.pair_error("Too many attempts, wait a second.");
        }
        self.last_pair = Some(Instant::now());
        let verdict = self.host.pairing.lock().unwrap().verify(code);
        match verdict {
            Verify::Ok => {}
            Verify::Wrong => return self.pair_error("Wrong code. Check the code shown in OmniPanel."),
            Verify::Rotated => {
                self.host.emit_runtime();
                return self.pair_error("Too many wrong codes. A new code is shown in OmniPanel.");
            }
            Verify::Inactive => {
                return self.pair_error("Pairing is off. Click \"Pair a device\" in OmniPanel on your computer.");
            }
        }
        let client = self.client.clone().unwrap_or_default();
        let token = pairing::new_token();
        let device = Device {
            name: crate::clean_name(name).unwrap_or_else(|| pairing::platform_from_user_agent(&client.user_agent)),
            token_hash: pairing::hash_token(&token),
            platform: pairing::platform_from_user_agent(&client.user_agent),
            paired_at: now_ms(),
            last_seen: now_ms(),
            ..Device::default()
        };
        let saved = self.host.mutate(|c| {
            c.devices.push(device.clone());
            Ok(())
        });
        if let Err(e) = saved {
            return self.pair_error(&format!("Couldn't save the device: {e}"));
        }
        self.host.log(LogLevel::Info, format!("Paired {} ({})", device.name, device.platform));
        let _ = self.host.events.send(HostEvent::Paired { device: device.clone() });
        send(&self.tx, &HostToDeck::PairResult { ok: true, token: Some(token), error: None });
        self.session = Some(self.host.register_session(&device.id, self.tx.clone(), self.addr, &client));
    }

    fn pair_error(&self, error: &str) {
        send(&self.tx, &HostToDeck::PairResult { ok: false, token: None, error: Some(error.into()) });
    }
}
