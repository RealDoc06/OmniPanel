//! Executes leaf actions (keys, apps, scripts, media, volume, OBS, Home
//! Assistant, HTTP, system) and keeps the integration connections alive.
//!
//! Flow-control actions (`multi`, `toggle`, `delay`, `navigate`, `voice`)
//! belong to `omni-host`; [`Executor::run`] rejects them with
//! [`ActionError::NotLeaf`].

pub mod apps;
pub mod ha;
pub mod obs;

mod desktop;
mod http;
mod keys;
mod launch;
mod media;
mod proc;
mod rt;
mod script;
mod system;
#[cfg(test)]
mod testutil;
mod volume;

use std::sync::Arc;

use omni_audio::{AudioController, AudioError};
use omni_core::{Action, ActionOutcome, Settings};

/// Why an action failed. The `Display` text is meant for the user (toast, log).
#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error("{0} is handled by the host")]
    NotLeaf(&'static str),
    #[error("not supported on this platform: {0}")]
    Unsupported(String),
    #[error("invalid action: {0}")]
    Invalid(String),
    #[error("{0} is not connected")]
    NotConnected(&'static str),
    #[error("timed out after {0}s")]
    Timeout(u32),
    #[error("{0}")]
    Failed(String),
}

pub type Result<T> = std::result::Result<T, ActionError>;

impl From<AudioError> for ActionError {
    fn from(error: AudioError) -> Self {
        match error {
            AudioError::Unsupported(what) => Self::Unsupported(what),
            other => Self::Failed(other.to_string()),
        }
    }
}

/// Outcome of an action that has nothing to show.
fn done() -> ActionOutcome {
    ActionOutcome { message: None }
}

/// Run blocking work (OS input, audio, system calls) off the async runtime.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| ActionError::Failed(format!("action task failed: {e}")))?
}

/// Runs leaf actions and owns the OBS / Home Assistant connections. Cheap to share behind an `Arc`.
pub struct Executor {
    audio: Arc<AudioController>,
    http: reqwest::Client,
    obs: obs::ObsClient,
    ha: ha::HaClient,
}

impl Executor {
    pub fn new(audio: Arc<AudioController>, http: reqwest::Client) -> Self {
        Self { audio, obs: obs::ObsClient::new(), ha: ha::HaClient::new(http.clone()), http }
    }

    /// (Re)connect integrations after settings changed. Cheap when nothing changed.
    pub fn configure(&self, settings: &Settings) {
        self.obs.configure(&settings.obs);
        self.ha.configure(&settings.home_assistant);
    }

    pub fn obs(&self) -> &obs::ObsClient {
        &self.obs
    }

    pub fn home_assistant(&self) -> &ha::HaClient {
        &self.ha
    }

    pub fn audio(&self) -> &Arc<AudioController> {
        &self.audio
    }

    /// Run one leaf action. Must be called inside a tokio runtime.
    pub async fn run(&self, action: &Action) -> Result<ActionOutcome> {
        match action {
            Action::Hotkey { keys } => keys::press(keys).await.map(|()| done()),
            Action::TypeText { text } => keys::type_text(text).await.map(|()| done()),
            Action::OpenApp { app, args } => launch::open_app(app, args).await.map(|()| done()),
            Action::Open { target } => launch::open(target).await.map(|()| done()),
            Action::RunScript { interpreter, script, timeout_secs, show_output } => {
                let outcome = script::run(*interpreter, script, *timeout_secs).await?;
                Ok(if *show_output { outcome } else { done() })
            }
            Action::Media { command } => media::run(&self.audio, *command).await.map(|()| done()),
            Action::Volume { target, op } => volume::run(&self.audio, &self.obs, target, op).await.map(|()| done()),
            Action::Obs { op } => self.obs.run(op).await,
            Action::HomeAssistant { op } => self.ha.run(op).await,
            Action::Http { method, url, headers, body } => {
                http::run(&self.http, *method, url, headers, body.as_deref()).await
            }
            Action::System { command } => system::run(*command).await.map(|()| done()),
            Action::Voice { .. } => Err(ActionError::NotLeaf("voice")),
            Action::Navigate { .. } => Err(ActionError::NotLeaf("navigate")),
            Action::Multi { .. } => Err(ActionError::NotLeaf("multi")),
            Action::Toggle { .. } => Err(ActionError::NotLeaf("toggle")),
            Action::Delay { .. } => Err(ActionError::NotLeaf("delay")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_core::NavTarget;

    /// The host shares one executor between tasks.
    #[tokio::test]
    async fn executor_is_shareable_across_tasks() {
        fn assert_send_sync<T: Send + Sync>() {}
        fn assert_send<T: Send>(_: T) {}
        assert_send_sync::<Executor>();
        let executor = Executor::new(Arc::new(AudioController::new()), reqwest::Client::new());
        assert_send(executor.run(&Action::Delay { ms: 0 }));
    }

    #[tokio::test]
    async fn flow_control_actions_are_rejected() {
        let executor = Executor::new(Arc::new(AudioController::new()), reqwest::Client::new());
        for action in [
            Action::Delay { ms: 1 },
            Action::Navigate { to: NavTarget::NextPage },
            Action::Multi { steps: vec![], delay_ms: 0 },
        ] {
            assert!(matches!(executor.run(&action).await, Err(ActionError::NotLeaf(_))), "{action:?}");
        }
    }
}
