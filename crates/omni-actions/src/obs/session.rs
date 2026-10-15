//! One WebSocket connection to OBS: handshake, state bootstrap, then the
//! event / request loop until the connection ends.

use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use omni_core::{ConnState, ObsSettings};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, interval_at, sleep_until, timeout, timeout_at};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Bytes, Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async_with_config};

use super::Shared;
use super::model::{Model, ObsState, Query};
use super::proto::{self, CLOSE_AUTH_FAILED, Incoming};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Hello, Identified and each bootstrap round must arrive within this time.
const STEP_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest wait for the answer to a user request.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Quiet time after a structural event before the state is reloaded.
const RELOAD_DELAY: Duration = Duration::from_millis(250);
const PING_EVERY: Duration = Duration::from_secs(20);
/// Nothing received (not even a pong) for this long means the link is dead.
const DEAD_AFTER: Duration = Duration::from_secs(50);
const MAX_MESSAGE: usize = 8 << 20;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A request from [`super::ObsClient::request`] to send over the open connection.
pub(super) struct Call {
    pub id: String,
    pub kind: String,
    pub data: Value,
    pub reply: oneshot::Sender<crate::Result<Value>>,
}

/// Why a session ended.
pub(super) struct SessionEnd {
    pub message: String,
    /// Retrying quickly cannot help until the password changes.
    pub auth_failed: bool,
    /// The connection had been fully established before it ended.
    pub was_connected: bool,
}

enum Fail {
    Auth(&'static str),
    Other(String),
}

impl Fail {
    fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }
}

/// Resets the shared connection state when a session ends or is cancelled.
struct Disconnect<'a>(&'a Shared);

impl Drop for Disconnect<'_> {
    fn drop(&mut self) {
        self.0.set_link(None);
        self.0.state.send_if_modified(|state| {
            let changed = *state != ObsState::default();
            *state = ObsState::default();
            changed
        });
    }
}

/// Connect and serve until the connection ends. The shared state is reset
/// afterwards, also when this future is dropped (settings changed).
pub(super) async fn run(shared: &Shared, settings: &ObsSettings) -> SessionEnd {
    let _disconnect = Disconnect(shared);
    let mut was_connected = false;
    let Err(fail) = serve(shared, settings, &mut was_connected).await;
    match fail {
        Fail::Auth(message) => SessionEnd { message: message.into(), auth_failed: true, was_connected },
        Fail::Other(message) => SessionEnd { message, auth_failed: false, was_connected },
    }
}

async fn serve(shared: &Shared, settings: &ObsSettings, was_connected: &mut bool) -> Result<Infallible, Fail> {
    let mut session = Session::new(shared, connect(settings).await?);
    session.handshake(&settings.password).await?;
    session.bootstrap().await?;

    let (calls_tx, mut calls) = mpsc::channel(32);
    session.model.state.connected = true;
    shared.set_link(Some(calls_tx));
    shared.set_status(ConnState::Connected, None);
    session.publish();
    *was_connected = true;
    session.event_loop(&mut calls).await
}

async fn connect(settings: &ObsSettings) -> Result<Socket, Fail> {
    let host = match settings.host.trim() {
        "" => "127.0.0.1",
        host => host,
    };
    let address = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{}", settings.port)
    } else {
        format!("{host}:{}", settings.port)
    };
    let mut request = format!("ws://{address}")
        .into_client_request()
        .map_err(|e| Fail::other(format!("invalid OBS address {address}: {e}")))?;
    request.headers_mut().insert("Sec-WebSocket-Protocol", HeaderValue::from_static(proto::SUBPROTOCOL));
    let config = WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE));

    match timeout(CONNECT_TIMEOUT, connect_async_with_config(request, Some(config), true)).await {
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(WsError::Io(error))) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            Err(Fail::other(format!("OBS is not running or its WebSocket server is off ({address})")))
        }
        Ok(Err(WsError::Io(error))) => Err(Fail::other(format!("can't reach OBS at {address}: {error}"))),
        Ok(Err(WsError::Http(_) | WsError::HttpFormat(_))) => {
            Err(Fail::other(format!("{address} is not an OBS WebSocket v5 server")))
        }
        Ok(Err(error)) => Err(Fail::other(format!("can't connect to OBS: {error}"))),
        Err(_) => Err(Fail::other(format!("timed out connecting to OBS at {address}"))),
    }
}

struct Session<'a> {
    shared: &'a Shared,
    socket: Socket,
    model: Model,
    /// User requests waiting for their answer.
    pending: HashMap<String, oneshot::Sender<crate::Result<Value>>>,
    last_received: Instant,
    /// A structural event arrived and the state needs a reload.
    stale: bool,
}

impl<'a> Session<'a> {
    fn new(shared: &'a Shared, socket: Socket) -> Self {
        Self {
            shared,
            socket,
            model: Model::default(),
            pending: HashMap::new(),
            last_received: Instant::now(),
            stale: false,
        }
    }

    /// Next protocol message. Close frames become errors.
    async fn recv(&mut self) -> Result<Incoming, Fail> {
        loop {
            let message = match self.socket.next().await {
                None => return Err(Fail::other("OBS closed the connection")),
                Some(Err(error)) => return Err(Fail::other(format!("connection to OBS lost: {error}"))),
                Some(Ok(message)) => message,
            };
            self.last_received = Instant::now();
            match message {
                Message::Text(text) => match proto::parse(&text) {
                    Ok(incoming) => return Ok(incoming),
                    Err(error) => tracing::debug!("ignoring unreadable OBS message: {error}"),
                },
                Message::Close(frame) => return Err(close_reason(frame)),
                // Pings are answered by tungstenite; binary frames do not occur with the JSON subprotocol.
                _ => {}
            }
        }
    }

    /// Like [`Self::recv`] with a deadline.
    async fn recv_by(&mut self, deadline: Instant, what: &str) -> Result<Incoming, Fail> {
        timeout_at(deadline, self.recv()).await.map_err(|_| Fail::other(format!("OBS did not send {what} in time")))?
    }

    async fn send(&mut self, text: String) -> Result<(), Fail> {
        self.socket.send(Message::text(text)).await.map_err(|e| Fail::other(format!("connection to OBS lost: {e}")))
    }

    /// Hello, then Identify (answering the password challenge), then Identified.
    async fn handshake(&mut self, password: &str) -> Result<(), Fail> {
        let deadline = Instant::now() + STEP_TIMEOUT;
        let Incoming::Hello(challenge) =
            self.recv_by(deadline, "Hello (is this OBS 28+ with the WebSocket server enabled?)").await?
        else {
            return Err(Fail::other("unexpected first message from OBS"));
        };
        let authentication = match challenge {
            Some(_) if password.is_empty() => return Err(Fail::Auth("authentication failed (OBS needs a password)")),
            Some(challenge) => Some(proto::auth_response(password, &challenge.salt, &challenge.challenge)),
            None => None,
        };
        self.send(proto::identify(authentication)).await?;
        loop {
            if let Incoming::Identified = self.recv_by(deadline, "Identified").await? {
                return Ok(());
            }
        }
    }

    /// Reload the whole state: two rounds, since inputs and scenes must be known first.
    async fn bootstrap(&mut self) -> Result<(), Fail> {
        self.model.reset();
        self.query(Query::base()).await?;
        let follow_up = self.model.follow_up();
        self.query(follow_up).await
    }

    /// Send all `queries` at once and fold the answers into the model as they
    /// arrive, in order with the events, so a newer event is never overwritten
    /// by an older answer. Failed queries (feature off) keep the defaults.
    async fn query(&mut self, queries: Vec<Query>) -> Result<(), Fail> {
        let mut waiting = HashMap::new();
        for (index, query) in queries.iter().enumerate() {
            let id = self.shared.next_request_id();
            let (kind, data) = query.request();
            self.send(proto::request(&id, kind, &data)).await?;
            waiting.insert(id, index);
        }
        let deadline = Instant::now() + STEP_TIMEOUT;
        while !waiting.is_empty() {
            match self.recv_by(deadline, "the state").await? {
                Incoming::Response(response) if waiting.contains_key(&response.id) => {
                    let query = &queries[waiting.remove(&response.id).unwrap_or_default()];
                    if response.ok {
                        self.model.apply_answer(query, &response.data);
                    }
                }
                other => self.handle(other),
            }
        }
        Ok(())
    }

    /// Events update the model, responses go to whoever asked.
    fn handle(&mut self, incoming: Incoming) {
        match incoming {
            Incoming::Event { kind, data } => self.stale |= self.model.apply_event(&kind, &data),
            Incoming::Response(response) => {
                if let Some(reply) = self.pending.remove(&response.id) {
                    let _ = reply.send(response.into_result());
                }
            }
            Incoming::Hello(_) | Incoming::Identified | Incoming::Other => {}
        }
    }

    fn publish(&self) {
        self.shared.state.send_if_modified(|state| {
            let changed = *state != self.model.state;
            if changed {
                state.clone_from(&self.model.state);
            }
            changed
        });
    }

    async fn event_loop(&mut self, calls: &mut mpsc::Receiver<Call>) -> Result<Infallible, Fail> {
        let mut ping = interval_at(Instant::now() + PING_EVERY, PING_EVERY);
        let mut reload_at: Option<Instant> = None;
        loop {
            tokio::select! {
                incoming = self.recv() => {
                    self.handle(incoming?);
                    self.publish();
                }
                Some(call) = calls.recv() => {
                    // Answers nobody waits for anymore (timed out) would pile up otherwise.
                    self.pending.retain(|_, reply| !reply.is_closed());
                    self.send(proto::request(&call.id, &call.kind, &call.data)).await?;
                    self.pending.insert(call.id, call.reply);
                }
                () = sleep_until(reload_at.unwrap_or_else(Instant::now)), if reload_at.is_some() => {
                    reload_at = None;
                    self.bootstrap().await?;
                    self.publish();
                }
                _ = ping.tick() => {
                    if self.last_received.elapsed() > DEAD_AFTER {
                        return Err(Fail::other("connection to OBS timed out"));
                    }
                    self.socket.send(Message::Ping(Bytes::new())).await.map_err(|e| Fail::other(format!("connection to OBS lost: {e}")))?;
                }
            }
            if self.stale && reload_at.is_none() {
                reload_at = Some(Instant::now() + RELOAD_DELAY);
            }
            self.stale = false;
        }
    }
}

fn close_reason(frame: Option<CloseFrame>) -> Fail {
    match frame {
        Some(frame) if u16::from(frame.code) == CLOSE_AUTH_FAILED => Fail::Auth("authentication failed"),
        Some(frame) if !frame.reason.is_empty() => Fail::other(format!("OBS closed the connection: {}", frame.reason)),
        _ => Fail::other("OBS closed the connection"),
    }
}
