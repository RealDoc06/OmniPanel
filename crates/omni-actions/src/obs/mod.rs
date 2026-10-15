//! OBS Studio integration over obs-websocket v5 (OBS 28+).
//!
//! [`ObsClient::configure`] starts a supervisor task that keeps one WebSocket
//! session alive while the integration is enabled (reconnecting with backoff,
//! restarting on settings changes). The session mirrors OBS into
//! [`ObsState`] and executes requests for the actions.

mod model;
mod ops;
mod proto;
mod session;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use omni_core::{ConnState, ConnectionStatus, ObsSettings};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{ActionError, Result};
pub use model::ObsState;
use session::{Call, REQUEST_TIMEOUT};

const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Connection to OBS Studio: live [`ObsState`] plus request execution.
pub struct ObsClient {
    shared: std::sync::Arc<Shared>,
    /// Desired connection; the supervisor follows it. Dropping the client ends the supervisor.
    settings: watch::Sender<ObsSettings>,
    supervisor_started: AtomicBool,
    backoff: (Duration, Duration),
}

/// State shared between the client and its connection tasks.
struct Shared {
    state: watch::Sender<ObsState>,
    status: Mutex<ConnectionStatus>,
    /// Sends requests to the open session, `None` while disconnected.
    link: Mutex<Option<mpsc::Sender<Call>>>,
    next_id: AtomicU64,
}

impl Shared {
    fn set_status(&self, state: ConnState, message: Option<String>) {
        let next = ConnectionStatus { state, message };
        let mut status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
        if *status != next {
            *status = next;
            drop(status);
            // Subscribers learn about connection changes through the state channel too.
            self.state.send_modify(|_| {});
        }
    }

    fn set_link(&self, link: Option<mpsc::Sender<Call>>) {
        *self.link.lock().unwrap_or_else(PoisonError::into_inner) = link;
    }

    fn next_request_id(&self) -> String {
        self.next_id.fetch_add(1, Ordering::Relaxed).to_string()
    }
}

impl ObsClient {
    pub fn new() -> Self {
        Self::with_backoff(BACKOFF_MIN, BACKOFF_MAX)
    }

    fn with_backoff(min: Duration, max: Duration) -> Self {
        Self {
            shared: std::sync::Arc::new(Shared {
                state: watch::Sender::new(ObsState::default()),
                status: Mutex::default(),
                link: Mutex::default(),
                next_id: AtomicU64::new(1),
            }),
            settings: watch::Sender::new(ObsSettings::default()),
            supervisor_started: AtomicBool::new(false),
            backoff: (min, max),
        }
    }

    /// Connect / disconnect / reconnect according to `settings.enabled`.
    pub fn configure(&self, settings: &ObsSettings) {
        let changed = self.settings.send_if_modified(|current| {
            let changed = current != settings;
            if changed {
                current.clone_from(settings);
            }
            changed
        });
        // Nothing runs until OBS is enabled for the first time.
        if changed && settings.enabled && !self.supervisor_started.swap(true, Ordering::Relaxed) {
            crate::rt::spawn(supervise(self.shared.clone(), self.settings.subscribe(), self.backoff));
        }
    }

    pub fn status(&self) -> ConnectionStatus {
        self.shared.status.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn state(&self) -> ObsState {
        self.shared.state.borrow().clone()
    }

    /// Notified on every state or connection change.
    pub fn subscribe(&self) -> watch::Receiver<ObsState> {
        self.shared.state.subscribe()
    }

    /// Send one obs-websocket request and wait for its answer.
    pub(crate) async fn request(&self, kind: &str, data: Value) -> Result<Value> {
        let link = self.shared.link.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let link = link.ok_or(ActionError::NotConnected("OBS"))?;
        let (reply, answer) = oneshot::channel();
        let call = Call { id: self.shared.next_request_id(), kind: kind.to_owned(), data, reply };
        link.send(call).await.map_err(|_| ActionError::NotConnected("OBS"))?;
        match tokio::time::timeout(REQUEST_TIMEOUT, answer).await {
            Ok(Ok(result)) => result,
            // The session ended before OBS answered.
            Ok(Err(_)) => Err(ActionError::NotConnected("OBS")),
            Err(_) => Err(ActionError::Timeout(REQUEST_TIMEOUT.as_secs() as u32)),
        }
    }
}

impl Default for ObsClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Keep a session running while enabled; retry with backoff; follow settings changes.
async fn supervise(
    shared: std::sync::Arc<Shared>,
    mut settings: watch::Receiver<ObsSettings>,
    (min, max): (Duration, Duration),
) {
    let mut delay = min;
    // Show "connecting" for the first attempt after a settings change, then keep the last error visible.
    let mut fresh = true;
    let mut last_error = String::new();
    loop {
        let current = settings.borrow_and_update().clone();
        if !current.enabled {
            shared.set_status(ConnState::Disabled, None);
            if settings.changed().await.is_err() {
                return;
            }
            (delay, fresh) = (min, true);
            continue;
        }
        if fresh {
            shared.set_status(ConnState::Connecting, None);
        }

        let end = tokio::select! {
            end = session::run(&shared, &current) => end,
            changed = settings.changed() => {
                if changed.is_err() {
                    return;
                }
                (delay, fresh) = (min, true);
                continue;
            }
        };
        // Retries repeat the same failure; log it once.
        if end.message != last_error {
            tracing::info!("OBS: {}", end.message);
            last_error.clone_from(&end.message);
        }
        shared.set_status(ConnState::Error, Some(end.message));
        if end.was_connected {
            delay = min;
        }
        let wait = if end.auth_failed { max } else { delay };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            changed = settings.changed() => {
                if changed.is_err() {
                    return;
                }
                (delay, fresh) = (min, true);
                continue;
            }
        }
        delay = (delay * 2).min(max);
        fresh = false;
    }
}
