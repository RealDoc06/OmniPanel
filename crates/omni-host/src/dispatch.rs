//! Runs actions: flow control (multi, toggle, delay, navigate, voice) here,
//! everything else through `omni_actions::Executor`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use omni_core::{Action, ActionOutcome, ControlKind, HostToDeck, LogLevel, ToastLevel, VolumeTarget};
use tokio::sync::mpsc;

use crate::Host;
use crate::sessions::profile_of;

/// Where an action was triggered from; decides which decks `navigate` moves.
#[derive(Debug, Clone, Copy)]
pub enum Origin<'a> {
    /// A press on a deck session.
    Deck(u64),
    /// A global hotkey bound to a control of this profile.
    Hotkey { profile_id: &'a str },
    /// The desktop UI "Test" button.
    Ui,
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Longest single `delay` step.
const MAX_DELAY_MS: u32 = 60_000;

impl Host {
    pub(crate) fn execute<'a>(
        &'a self,
        origin: Origin<'a>,
        control_id: &'a str,
        action: &'a Action,
    ) -> BoxFuture<'a, crate::Result<ActionOutcome>> {
        Box::pin(async move {
            match action {
                Action::Multi { steps, delay_ms } => {
                    let mut message = None;
                    for (i, step) in steps.iter().enumerate() {
                        if i > 0 && *delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis((*delay_ms).min(MAX_DELAY_MS).into())).await;
                        }
                        if let Some(m) = self.execute(origin, control_id, step).await?.message {
                            message = Some(m);
                        }
                    }
                    Ok(ActionOutcome { message })
                }
                Action::Toggle { on, off } => {
                    let active = self.toggles.lock().unwrap().get(control_id).copied().unwrap_or(false);
                    let outcome = self.execute(origin, control_id, if active { off } else { on }).await?;
                    self.toggles.lock().unwrap().insert(control_id.to_string(), !active);
                    Ok(outcome)
                }
                Action::Delay { ms } => {
                    tokio::time::sleep(Duration::from_millis((*ms).min(MAX_DELAY_MS).into())).await;
                    Ok(ActionOutcome { message: None })
                }
                Action::Navigate { to } => {
                    let sessions = match origin {
                        Origin::Deck(id) => vec![id],
                        Origin::Hotkey { profile_id } => self.sessions_on_profile(profile_id),
                        Origin::Ui => {
                            return Ok(ActionOutcome { message: Some("Navigation runs on a deck".into()) });
                        }
                    };
                    self.navigate(&sessions, to);
                    Ok(ActionOutcome { message: None })
                }
                Action::Voice { op } => self.voice_op(op),
                leaf => self.run_leaf(leaf).await,
            }
        })
    }

    /// A deck pressed `control_id` on its current page.
    pub(crate) async fn press(&self, session: u64, control_id: &str) {
        let found = {
            let config = self.config();
            let sessions = self.sessions.lock().unwrap();
            sessions.by_id.get(&session).and_then(|s| {
                let profile = profile_of(&config, &s.device_id)?;
                let control = profile.page(&s.page_id)?.controls.iter().find(|c| c.id == control_id)?;
                match &control.kind {
                    ControlKind::Button { action: Some(action) } => Some((control.style.label.clone(), action.clone())),
                    _ => None,
                }
            })
        };
        let Some((label, action)) = found else { return };
        let result = self.execute(Origin::Deck(session), control_id, &action).await;
        self.report(Some(session), &label, result);
        self.feedback.notify_one();
    }

    /// Slider moved on a deck: queue the value (applied by `slider_worker`).
    pub(crate) fn slide(&self, session: u64, control_id: &str, value: f32) {
        if !value.is_finite() {
            return;
        }
        let target = {
            let config = self.config();
            let sessions = self.sessions.lock().unwrap();
            sessions.by_id.get(&session).and_then(|s| {
                let control = profile_of(&config, &s.device_id)?
                    .page(&s.page_id)?
                    .controls
                    .iter()
                    .find(|c| c.id == control_id)?;
                match &control.kind {
                    ControlKind::Slider { target, .. } => Some(target.clone()),
                    ControlKind::Button { .. } => None,
                }
            })
        };
        if let Some(target) = target {
            let _ = self.sliders.send((target, value.clamp(0.0, 1.0)));
        }
    }

    /// Accelerators bound to controls, deduplicated (case-insensitive).
    pub fn hotkeys(&self) -> Vec<String> {
        let config = self.config();
        let mut keys: Vec<String> = config
            .profiles
            .iter()
            .flat_map(|p| p.controls())
            .filter_map(|c| c.hotkey.as_deref().map(str::trim).filter(|k| !k.is_empty()))
            .map(str::to_string)
            .collect();
        keys.sort_by_key(|k| k.to_lowercase());
        keys.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        keys
    }

    /// A global shortcut fired: run every button bound to it.
    pub async fn trigger_hotkey(&self, accelerator: &str) {
        let bound: Vec<(String, String, String, Action)> = {
            let config = self.config();
            config
                .profiles
                .iter()
                .flat_map(|p| p.controls().map(move |c| (p, c)))
                .filter(|(_, c)| c.hotkey.as_deref().is_some_and(|k| k.trim().eq_ignore_ascii_case(accelerator.trim())))
                .filter_map(|(p, c)| match &c.kind {
                    ControlKind::Button { action: Some(a) } => {
                        Some((p.id.clone(), c.id.clone(), c.style.label.clone(), a.clone()))
                    }
                    _ => None,
                })
                .collect()
        };
        for (profile_id, control_id, label, action) in &bound {
            let result = self.execute(Origin::Hotkey { profile_id }, control_id, action).await;
            self.report(None, label, result);
        }
        self.feedback.notify_one();
    }

    /// Toast the outcome on the deck that triggered it; log failures.
    fn report(&self, session: Option<u64>, label: &str, result: crate::Result<ActionOutcome>) {
        let toast = match result {
            Ok(ActionOutcome { message: Some(text) }) => Some((text, ToastLevel::Info)),
            Ok(ActionOutcome { message: None }) => None,
            Err(e) => {
                let name = if label.is_empty() { "Action" } else { label };
                let text = format!("{name}: {e:#}");
                self.log(LogLevel::Error, text.clone());
                Some((text, ToastLevel::Error))
            }
        };
        if let (Some(session), Some((text, level))) = (session, toast) {
            self.send_to(session, &HostToDeck::Toast { text, level });
        }
    }
}

/// Applies slider values, coalescing bursts so only the latest value per
/// target is applied (decks send ~15 updates per second while dragging).
pub async fn slider_worker(host: Host, mut rx: mpsc::UnboundedReceiver<(VolumeTarget, f32)>) {
    while let Some((target, value)) = rx.recv().await {
        let mut latest = HashMap::from([(target, value)]);
        while let Ok((target, value)) = rx.try_recv() {
            latest.insert(target, value);
        }
        for (target, value) in latest {
            if let Err(e) = host.set_volume(target, value).await {
                host.log(LogLevel::Warn, format!("Slider: {e:#}"));
            }
        }
    }
}
