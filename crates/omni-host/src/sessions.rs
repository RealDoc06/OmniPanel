//! Connected decks: registration, per-device page, layout pushes, navigation.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use omni_core::{
    ClientInfo, Config, ControlState, DeckLayout, DeckSession, Device, HostToDeck, NavTarget, Profile, Transport,
    now_ms,
};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{Notify, mpsc};

use crate::{Host, layout};

/// Frames a deck may fall behind by before it is disconnected.
const OUTBOUND_QUEUE: usize = 256;

/// Frames queued for a deck's socket writer.
#[derive(Debug, Clone)]
pub enum Outbound {
    Text(Arc<str>),
    Close,
}

/// A deck's outbound queue. Bounded: a deck that stops reading fills it and
/// is disconnected (it reconnects and resyncs) instead of buffering forever.
#[derive(Clone)]
pub struct DeckTx {
    frames: mpsc::Sender<Outbound>,
    overflow: Arc<Notify>,
}

impl DeckTx {
    pub fn new() -> (Self, mpsc::Receiver<Outbound>) {
        let (frames, rx) = mpsc::channel(OUTBOUND_QUEUE);
        (Self { frames, overflow: Arc::default() }, rx)
    }

    fn push(&self, frame: Outbound) {
        if let Err(TrySendError::Full(_)) = self.frames.try_send(frame) {
            self.overflow.notify_one();
        }
    }

    /// Ask the writer to close the socket after the queued frames.
    pub fn close(&self) {
        self.push(Outbound::Close);
    }

    /// Resolves once a frame was dropped because the queue was full.
    pub async fn overflowed(&self) {
        self.overflow.notified().await;
    }
}

pub struct Session {
    pub device_id: String,
    pub names: (String, String),
    pub tx: DeckTx,
    pub transport: Transport,
    pub address: String,
    pub connected_at: u64,
    pub page_id: String,
    /// Screen orientation reported by the deck.
    pub portrait: Option<bool>,
    /// Last layout and states sent, to only push differences.
    pub layout: Option<DeckLayout>,
    pub states: HashMap<String, ControlState>,
}

#[derive(Default)]
pub struct Sessions {
    pub by_id: HashMap<u64, Session>,
    /// Last page per device, restored on reconnect.
    pub last_page: HashMap<String, String>,
}

pub fn send(tx: &DeckTx, message: &HostToDeck) {
    if let Ok(json) = serde_json::to_string(message) {
        tx.push(Outbound::Text(json.into()));
    }
}

fn orientation(width: u32, height: u32) -> Option<bool> {
    (width > 0 && height > 0).then_some(height > width)
}

/// Page of `profile` with `page_id`, or the first page.
fn page_index(profile: &Profile, page_id: &str) -> usize {
    profile.pages.iter().position(|p| p.id == page_id).unwrap_or(0)
}

impl Host {
    /// Called once a deck authenticated: remembers it, updates the device
    /// record and sends `welcome` + `layout`.
    pub(crate) fn register_session(&self, device_id: &str, tx: DeckTx, addr: SocketAddr, client: &ClientInfo) -> u64 {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        let android = client.user_agent.to_ascii_lowercase().contains("android");
        let transport = if addr.ip().is_loopback() && android { Transport::Usb } else { Transport::Wifi };
        let platform = crate::pairing::platform_from_user_agent(&client.user_agent);
        let _ = self.mutate(|c| {
            if let Some(device) = c.devices.iter_mut().find(|d| d.id == device_id) {
                device.last_seen = now_ms();
                device.platform = platform;
            }
            Ok(())
        });
        let host_name = self.host_name();
        let device_name = {
            let config = self.config();
            let device = config.device(device_id);
            let name = device.map(|d| d.name.clone()).unwrap_or_default();
            let mut sessions = self.sessions.lock().unwrap();
            let page_id = sessions.last_page.get(device_id).cloned().unwrap_or_default();
            sessions.by_id.insert(
                id,
                Session {
                    device_id: device_id.to_string(),
                    names: (name.clone(), host_name.clone()),
                    tx: tx.clone(),
                    transport,
                    address: addr.ip().to_string(),
                    connected_at: now_ms(),
                    page_id,
                    portrait: orientation(client.width, client.height),
                    layout: None,
                    states: HashMap::new(),
                },
            );
            name
        };
        send(
            &tx,
            &HostToDeck::Welcome {
                device_id: device_id.to_string(),
                device_name,
                host_name,
                host_version: self.version.clone(),
            },
        );
        self.refresh_sessions();
        self.emit_runtime();
        self.feedback.notify_one();
        id
    }

    /// The deck rotated: re-send the layout if the grid should flip.
    pub(crate) fn resize_session(&self, id: u64, width: u32, height: u32) {
        let changed = {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.by_id.get_mut(&id).is_some_and(|s| {
                let portrait = orientation(width, height);
                let changed = s.portrait != portrait;
                s.portrait = portrait;
                changed
            })
        };
        if changed {
            self.refresh_sessions();
        }
    }

    pub(crate) fn unregister_session(&self, id: u64) {
        let removed = self.sessions.lock().unwrap().by_id.remove(&id);
        if let Some(session) = removed {
            let _ = self.mutate(|c| {
                if let Some(device) = c.devices.iter_mut().find(|d| d.id == session.device_id) {
                    device.last_seen = now_ms();
                }
                Ok(())
            });
            self.emit_runtime();
        }
    }

    pub(crate) fn session_list(&self) -> Vec<DeckSession> {
        let config = self.config();
        let sessions = self.sessions.lock().unwrap();
        let mut list: Vec<DeckSession> = sessions
            .by_id
            .values()
            .map(|s| DeckSession {
                device_id: s.device_id.clone(),
                transport: s.transport,
                address: s.address.clone(),
                profile_id: config
                    .device(&s.device_id)
                    .and_then(|d| config.profile_for_device(d))
                    .map(|p| p.id.clone())
                    .unwrap_or_default(),
                page_id: s.layout.as_ref().map(|l| l.page_id.clone()).unwrap_or_default(),
                connected_at: s.connected_at,
            })
            .collect();
        list.sort_by_key(|s| s.connected_at);
        list
    }

    /// Rebuild every deck's layout and push the ones that changed. Decks
    /// whose device was unpaired are told so and disconnected.
    pub(crate) fn refresh_sessions(&self) {
        let config = self.config();
        let states = self.state_context(&config);
        let host_name = if config.settings.host_name.trim().is_empty() {
            self.machine_name.clone()
        } else {
            config.settings.host_name.trim().to_string()
        };
        let mut sessions = self.sessions.lock().unwrap();
        let mut gone = vec![];
        for (id, session) in sessions.by_id.iter_mut() {
            let Some(device) = config.device(&session.device_id) else {
                send(&session.tx, &HostToDeck::Unpaired);
                session.tx.close();
                gone.push(*id);
                continue;
            };
            let names = (device.name.clone(), host_name.clone());
            if session.names != names {
                send(
                    &session.tx,
                    &HostToDeck::Welcome {
                        device_id: device.id.clone(),
                        device_name: device.name.clone(),
                        host_name: host_name.clone(),
                        host_version: self.version.clone(),
                    },
                );
                session.names = names;
            }
            let Some(profile) = config.profile_for_device(device) else { continue };
            let index = page_index(profile, &session.page_id);
            session.page_id = profile.pages[index].id.clone();
            let layout = layout::build(&config, profile, index, &device.deck, session.portrait, |control_id| {
                profile.pages[index]
                    .controls
                    .iter()
                    .find(|c| c.id == control_id)
                    .map(|c| states.state_for(c, &profile.id, &session.page_id))
                    .unwrap_or_default()
            });
            if session.layout.as_ref() != Some(&layout) {
                session.states = layout.controls.iter().map(|c| (c.id.clone(), c.state.clone())).collect();
                send(&session.tx, &HostToDeck::Layout { layout: Box::new(layout.clone()) });
                session.layout = Some(layout);
            }
        }
        for id in gone {
            sessions.by_id.remove(&id);
        }
        let last: Vec<(String, String)> =
            sessions.by_id.values().map(|s| (s.device_id.clone(), s.page_id.clone())).collect();
        sessions.last_page.extend(last);
    }

    /// Move one deck (or every deck showing a profile) to another page/profile.
    pub(crate) fn navigate(&self, sessions_to_move: &[u64], to: &NavTarget) {
        if let NavTarget::Profile { profile_id } = to {
            let devices: Vec<String> = {
                let sessions = self.sessions.lock().unwrap();
                sessions_to_move.iter().filter_map(|id| sessions.by_id.get(id)).map(|s| s.device_id.clone()).collect()
            };
            let _ = self.mutate(|c| {
                if c.profile(profile_id).is_none() {
                    anyhow::bail!("profile not found");
                }
                for device in c.devices.iter_mut().filter(|d| devices.contains(&d.id)) {
                    device.profile_id = Some(profile_id.clone());
                }
                Ok(())
            });
            return;
        }
        {
            let config = self.config();
            let mut sessions = self.sessions.lock().unwrap();
            for id in sessions_to_move {
                let Some(session) = sessions.by_id.get_mut(id) else { continue };
                let Some(profile) = profile_of(&config, &session.device_id) else { continue };
                let current = page_index(profile, &session.page_id);
                let last = profile.pages.len() - 1;
                let target = match to {
                    NavTarget::NextPage => (current + 1).min(last),
                    NavTarget::PrevPage => current.saturating_sub(1),
                    NavTarget::Page { page_id } => page_index(profile, page_id),
                    NavTarget::Profile { .. } => current,
                };
                session.page_id = profile.pages[target].id.clone();
            }
        }
        self.refresh_sessions();
        self.emit_runtime();
        self.feedback.notify_one();
    }

    /// Sessions currently showing `profile_id`.
    pub(crate) fn sessions_on_profile(&self, profile_id: &str) -> Vec<u64> {
        let config = self.config();
        let sessions = self.sessions.lock().unwrap();
        sessions
            .by_id
            .iter()
            .filter(|(_, s)| profile_of(&config, &s.device_id).is_some_and(|p| p.id == profile_id))
            .map(|(id, _)| *id)
            .collect()
    }

    pub(crate) fn send_to(&self, session: u64, message: &HostToDeck) {
        if let Some(s) = self.sessions.lock().unwrap().by_id.get(&session) {
            send(&s.tx, message);
        }
    }

    pub(crate) fn close_all_sessions(&self) {
        let mut sessions = self.sessions.lock().unwrap();
        for session in sessions.by_id.values() {
            session.tx.close();
        }
        sessions.by_id.clear();
    }
}

pub(crate) fn profile_of<'a>(config: &'a Config, device_id: &str) -> Option<&'a Profile> {
    config.device(device_id).and_then(|d: &Device| config.profile_for_device(d))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_deck_that_stops_reading_overflows_instead_of_buffering() {
        let (tx, _rx) = DeckTx::new();
        for _ in 0..OUTBOUND_QUEUE {
            send(&tx, &HostToDeck::Pong);
        }
        assert!(tokio::time::timeout(Duration::from_millis(20), tx.overflowed()).await.is_err());
        send(&tx, &HostToDeck::Pong);
        tokio::time::timeout(Duration::from_secs(1), tx.overflowed()).await.expect("overflow signalled");
    }
}
