//! One deck WebSocket connection: authentication, pairing, then input.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use omni_core::{ClientInfo, DeckToHost, Device, HostEvent, HostToDeck, LogLevel, now_ms};
use tokio::sync::{mpsc, watch};

use crate::Host;
use crate::pairing::{self, Verify};
use crate::sessions::{DeckTx, Outbound, send};

/// Decks ping every 20 s; silence longer than this means the socket is dead
/// (phone went to sleep, Wi-Fi dropped).
const IDLE_TIMEOUT: Duration = Duration::from_secs(75);
/// A deck that doesn't take a frame for this long stopped reading.
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);
/// Minimum time between pairing attempts on one connection.
const PAIR_COOLDOWN: Duration = Duration::from_secs(1);

/// Serve one deck until it disconnects, stops reading (write timeout or a
/// full outbound queue) or its server stops (`stop`'s sender is dropped).
pub async fn run(host: Host, socket: WebSocket, addr: SocketAddr, mut stop: watch::Receiver<()>) {
    let (sink, mut stream) = socket.split();
    let (tx, rx) = DeckTx::new();
    let mut writer = tokio::spawn(write(sink, rx));

    let mut conn = Connection { host: host.clone(), tx, addr, session: None, client: None, last_pair: None };
    loop {
        let message = tokio::select! {
            message = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => message,
            _ = &mut writer => break,
            () = conn.tx.overflowed() => break,
            _ = stop.changed() => break,
        };
        match message {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(message) = serde_json::from_str::<DeckToHost>(&text) {
                    conn.handle(message);
                }
            }
            Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) | Err(_) => break,
            Ok(Some(Ok(_))) => {}
        }
    }
    if let Some(id) = conn.session {
        host.unregister_session(id);
    }
    drop(conn);
    writer.abort();
}

/// Sends queued frames until `Close`, a failed write or a deck that stops reading.
async fn write(mut sink: SplitSink<WebSocket, Message>, mut rx: mpsc::Receiver<Outbound>) {
    while let Some(frame) = rx.recv().await {
        let (message, last) = match frame {
            Outbound::Text(text) => (Message::Text(text.as_ref().into()), false),
            Outbound::Close => (Message::Close(None), true),
        };
        let sent = tokio::time::timeout(WRITE_TIMEOUT, sink.send(message)).await;
        if last || !matches!(sent, Ok(Ok(()))) {
            break;
        }
    }
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
                    self.host.log(LogLevel::Info, "A device disconnected and forgot this computer");
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
            Verify::Wrong => return self.pair_error("Wrong code. Check the code on your computer."),
            Verify::Rotated => {
                // The desktop shows the new code from the runtime state.
                self.host.emit_runtime();
                return self.pair_error(&format!(
                    "Too many wrong codes. Wait {} seconds, then enter the new code shown on your computer.",
                    pairing::LOCKOUT_MS / 1000
                ));
            }
            Verify::Locked => return self.pair_error("Too many wrong codes. Wait a moment and try again."),
            Verify::Expired => return self.pair_error("Code expired. Enter the new code shown on your computer."),
            Verify::Inactive => {
                return self.pair_error("Pairing is off. Click \"Connect a device\" in OmniPanel on your computer.");
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
        self.host.log(LogLevel::Info, format!("Connected {} ({})", device.name, device.platform));
        let _ = self.host.events.send(HostEvent::Paired { device: device.clone() });
        send(&self.tx, &HostToDeck::PairResult { ok: true, token: Some(token), error: None });
        self.session = Some(self.host.register_session(&device.id, self.tx.clone(), self.addr, &client));
    }

    fn pair_error(&self, error: &str) {
        send(&self.tx, &HostToDeck::PairResult { ok: false, token: None, error: Some(error.into()) });
    }
}
