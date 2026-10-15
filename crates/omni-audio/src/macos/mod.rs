//! macOS backend: CoreAudio device volume, process taps for per-app and software
//! master volume.
//!
//! macOS has no per-app volume, so an app's level is ours: kept in `State::desired`
//! (keyed by app id, so it survives app restarts) and applied with a [`Tap`] while it
//! is not 100%. At 100% and unmuted the tap is removed and the app plays natively.
//!
//! Outputs without a volume (or mute) control of their own get a software master:
//! one tap of everything played to the device except OmniPanel and the apps that
//! have their own tap. Those apps are re-rendered by our process, so their taps carry
//! the master gain too. At 100% and unmuted the master tap is removed.
//!
//! A worker thread, started on first use, keeps taps in sync with the system:
//! re-attaches restarted apps, follows helper processes and default-output changes.
//! It sleeps on CoreAudio change notifications and only polls while taps exist.

mod ca;
mod device;
mod process;
mod tap;
mod tcc;
#[cfg(test)]
mod testing;

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use objc2_core_audio::{
    AudioObjectAddPropertyListener, AudioObjectID, AudioObjectPropertyAddress, AudioObjectRemovePropertyListener,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices, kAudioHardwarePropertyProcessObjectList,
    kAudioHardwarePropertyTranslatePIDToProcessObject,
};
use omni_core::AppLevel;

use self::ca::ObjectId;
use self::process::{AppGroup, Processes};
use self::tap::{Source, Tap};
use self::tcc::Permission;
use crate::{AppAudio, AudioCapabilities, AudioError, Backend, Endpoint, Result, VolumeState, find_app};

/// Health check / fallback poll while taps exist: dead or stalled devices, sample-rate
/// switches, anything notifications miss. Cheap: cached identities, no tccd queries.
const POLL: Duration = Duration::from_secs(1);
/// Coalesces bursts of notifications (a device switch fires several).
const DEBOUNCE: Duration = Duration::from_millis(80);
/// How long a replaced tap keeps playing while its successor fades in.
const CROSSFADE: Duration = Duration::from_millis(60);
/// Wait before rebuilding a tap that failed to build.
const RETRY: Duration = Duration::from_secs(5);
/// How long an unanswered / denied permission status is reused (each check is an IPC to tccd).
const PERMISSION_TTL: Duration = Duration::from_secs(2);

const PERMISSION_HINT: &str = "allow OmniPanel under System Settings > Privacy & Security > \
     Screen & System Audio Recording (System Audio Recording Only)";
const UNSUPPORTED_HINT: &str = "per-app volume needs macOS 14.2 or later";

pub(crate) struct Platform {
    shared: Arc<Shared>,
    worker: Mutex<Option<Worker>>,
}

struct Shared {
    state: Mutex<State>,
}

/// A tap we keep: one app's, or the software master.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    App(String),
    Master,
}

/// Which controls the default output has itself; the software master covers the rest.
#[derive(Debug, Clone, Copy)]
struct Controls {
    volume: bool,
    mute: bool,
}

#[derive(Default)]
struct State {
    processes: Processes,
    /// Latest full listing (with playing state); `find` answers from it.
    listing: Vec<AppGroup>,
    /// Non-default app levels by app id.
    desired: HashMap<String, Level>,
    taps: HashMap<String, Tap>,
    /// Software master level, applied for the controls the default output lacks.
    master: Level,
    master_tap: Option<Tap>,
    /// Cached `Controls` of the default output (checking costs several IPCs).
    controls: Option<(ObjectId, Controls)>,
    /// Last tap failure, surfaced in `capabilities().note`.
    error: Option<String>,
    /// Taps that failed to build, and when to try again.
    backoff: HashMap<Key, Instant>,
    /// The permission prompt was shown (once per run), and its answer.
    prompted: bool,
    prompt_granted: Arc<AtomicBool>,
    /// Granted is final for the run; other answers are re-checked after `PERMISSION_TTL`.
    granted: bool,
    checked: Option<(Instant, Option<Permission>)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Level {
    volume: f32,
    muted: bool,
}

impl Default for Level {
    fn default() -> Self {
        Level { volume: 1.0, muted: false }
    }
}

impl Level {
    fn is_unity(self) -> bool {
        !self.muted && self.volume >= 0.999
    }

    /// Amplitude gain. Cubic taper, like PulseAudio's volume scale, so 50% sounds
    /// about half as loud rather than barely quieter.
    fn gain(self) -> f32 {
        if self.muted { 0.0 } else { self.volume.clamp(0.0, 1.0).powi(3) }
    }
}

/// Result of `State::sync`.
#[derive(Default)]
struct Synced {
    /// Replaced or removed taps, already ramping to their hand-over gain: drop after
    /// `CROSSFADE`, unlocked.
    retired: Vec<Tap>,
    failures: Vec<(Key, AudioError)>,
    /// A failed tap was not retried yet (backoff).
    skipped: bool,
}

impl State {
    /// The latest listing as the API shows it: Dock apps, anything playing, and apps
    /// we hold a level for. Background daemons that merely opened CoreAudio are hidden.
    fn apps(&self) -> Vec<AppAudio> {
        self.listing
            .iter()
            .filter(|g| g.playing || g.regular || self.desired.contains_key(&g.id))
            .map(|g| {
                let level = self.desired.get(&g.id).copied().unwrap_or_default();
                AppAudio {
                    id: g.id.clone(),
                    name: g.name.clone(),
                    pid: u32::try_from(g.pid).ok(),
                    volume: level.volume,
                    muted: level.muted,
                    playing: g.playing,
                }
            })
            .collect()
    }

    fn set_level(&mut self, app: &str, level: Level) {
        if level.is_unity() {
            self.desired.remove(app);
        } else {
            self.desired.insert(app.to_string(), level);
        }
    }

    fn controls(&mut self, device: ObjectId) -> Controls {
        match self.controls {
            Some((cached, controls)) if cached == device => controls,
            _ => {
                let controls = Controls {
                    volume: device::has_volume(device, Endpoint::Output),
                    mute: device::has_mute(device, Endpoint::Output),
                };
                self.controls = Some((device, controls));
                controls
            }
        }
    }

    /// Gain of the software master on `device`: it only stands in for missing controls.
    fn master_gain(&mut self, device: ObjectId) -> f32 {
        let controls = self.controls(device);
        let volume = if controls.volume { 1.0 } else { Level { muted: false, ..self.master }.gain() };
        if self.master.muted && !controls.mute { 0.0 } else { volume }
    }

    /// Whether `key` needs a tap now.
    fn wants(&mut self, key: &Key, device: ObjectId) -> bool {
        match key {
            Key::App(id) => self.desired.contains_key(id),
            Key::Master => self.master_gain(device) < 1.0,
        }
    }

    /// Pushes the current levels into running taps: the hot path of a dragged slider.
    fn apply_gains(&mut self) {
        let Some(device) = self.master_tap.as_ref().or(self.taps.values().next()).map(|t| t.device) else { return };
        let master = self.master_gain(device);
        for (id, tap) in &self.taps {
            tap.set_gain(self.desired.get(id).copied().unwrap_or_default().gain() * master);
        }
        if let Some(tap) = &self.master_tap {
            tap.set_gain(master);
        }
    }

    /// Current permission; `None` when it cannot be queried.
    fn permission(&mut self) -> Option<Permission> {
        if self.granted || self.prompt_granted.load(Relaxed) {
            self.granted = true;
            return Some(Permission::Granted);
        }
        if let Some((at, status)) = self.checked
            && at.elapsed() < PERMISSION_TTL
        {
            return status;
        }
        let status = tcc::status();
        self.granted = status == Some(Permission::Granted);
        self.checked = Some((Instant::now(), status));
        status
    }

    /// Creates, retargets, rebuilds and removes taps on `device` to match the levels.
    /// `forced` skips the failure backoff (the user just asked for it).
    fn sync(&mut self, device: ObjectId, groups: &[AppGroup], forced: Option<&Key>) -> Synced {
        let master = self.master_gain(device);
        let mut synced = Synced::default();
        // Hand-over gains: a released app rejoins the master tap (fade out) or plays
        // natively at full level (ramp to 1 so nothing jumps).
        let released = if master < 1.0 { 0.0 } else { 1.0 };
        let State { desired, taps, master_tap, backoff, .. } = self;
        for (_, tap) in taps.extract_if(|id, _| !desired.contains_key(id) || !groups.iter().any(|g| &g.id == id)) {
            tap.set_gain(released);
            synced.retired.push(tap);
        }
        for (id, level) in desired.iter() {
            let Some(group) = groups.iter().find(|g| &g.id == id) else { continue };
            let key = Key::App(id.clone());
            let slot = Slot { key: &key, label: &group.name, source: Source::Processes(group.objects.clone()) };
            let gain = level.gain() * master;
            if let Some(tap) = slot.ensure(taps.remove(id), device, gain, forced, backoff, &mut synced) {
                taps.insert(id.clone(), tap);
            }
        }

        // After the app taps, so a newly tapped app leaves the master only once its own
        // tap is running, and a released one rejoins before its tap goes.
        if master < 1.0 {
            match own_process() {
                Some(own) => {
                    let mut excluded: Vec<ObjectId> = taps.values().flat_map(|t| t.source.objects()).collect();
                    excluded.push(own);
                    excluded.sort_unstable();
                    excluded.dedup();
                    let slot = Slot { key: &Key::Master, label: "master", source: Source::AllExcept(excluded) };
                    *master_tap = slot.ensure(master_tap.take(), device, master, forced, backoff, &mut synced);
                }
                None => synced
                    .failures
                    .push((Key::Master, AudioError::Backend("OmniPanel's audio process is not registered".into()))),
            }
        } else if let Some(old) = master_tap.take() {
            old.set_gain(1.0); // everything plays natively at full level next
            synced.retired.push(old);
        }
        synced
    }

    /// Worker pass: syncs everything when permitted. Never prompts.
    fn reconcile(&mut self) -> Vec<Tap> {
        let permitted = matches!(self.permission(), Some(Permission::Granted) | None);
        let wanted = !self.desired.is_empty() || !self.master.is_unity();
        let device = device::default_device(Endpoint::Output);
        if let Ok(device) = device {
            self.controls = None; // re-check now and then; cheap at worker pace
            self.controls(device);
        }
        let groups = if permitted && wanted { self.processes.groups(false).ok() } else { None };
        let (Ok(device), Some(groups)) = (device, groups) else {
            return self.take_taps();
        };
        let synced = self.sync(device, &groups, None);
        for (key, e) in &synced.failures {
            tracing::warn!("{key:?}: {e}");
        }
        match synced.failures.last() {
            Some((_, e)) => self.error = Some(e.to_string()),
            None if !synced.skipped => self.error = None,
            None => {}
        }
        synced.retired
    }

    fn take_taps(&mut self) -> Vec<Tap> {
        let master = self.master_tap.take();
        self.taps.drain().map(|(_, tap)| tap).chain(master).collect()
    }

    fn has_taps(&self) -> bool {
        !self.taps.is_empty() || self.master_tap.is_some()
    }
}

impl Source {
    fn objects(&self) -> impl Iterator<Item = ObjectId> + '_ {
        match self {
            Source::Processes(objects) | Source::AllExcept(objects) => objects.iter().copied(),
        }
    }
}

/// One tap to keep in line with its wanted source and gain.
struct Slot<'a> {
    key: &'a Key,
    label: &'a str,
    source: Source,
}

impl Slot<'_> {
    /// Returns the tap to keep: `current` retargeted, or a new one. Taps are never
    /// stacked on the same device: a process tapped twice while it starts up can stay
    /// silent, so a broken tap goes before its replacement is built. Only a move to
    /// another output crossfades (the old tap keeps playing there meanwhile).
    fn ensure(
        self,
        current: Option<Tap>,
        device: ObjectId,
        gain: f32,
        forced: Option<&Key>,
        backoff: &mut HashMap<Key, Instant>,
        synced: &mut Synced,
    ) -> Option<Tap> {
        let current = match current {
            Some(mut tap) if tap.device == device => {
                if tap.healthy() && (tap.source == self.source || tap.set_source(&self.source).is_ok()) {
                    tap.set_gain(gain);
                    return Some(tap);
                }
                drop(tap);
                None
            }
            other => other,
        };
        if forced != Some(self.key) && backoff.get(self.key).is_some_and(|until| Instant::now() < *until) {
            synced.skipped = true;
            return current;
        }
        match Tap::new(self.label, self.source, device, gain) {
            Ok(tap) => {
                backoff.remove(self.key);
                if let Some(old) = current {
                    old.set_gain(0.0);
                    synced.retired.push(old);
                }
                Some(tap)
            }
            Err(e) => {
                backoff.insert(self.key.clone(), Instant::now() + RETRY);
                synced.failures.push((self.key.clone(), e));
                current
            }
        }
    }
}

/// This process's HAL object (excluded from the master tap: our own output is the
/// re-rendered audio, tapping it would feed back).
fn own_process() -> Option<ObjectId> {
    let pid = std::process::id() as i32;
    ca::get_qualified::<u32, i32>(ca::SYSTEM, ca::global(kAudioHardwarePropertyTranslatePIDToProcessObject), &pid)
        .ok()
        .filter(|&object| object != 0)
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves consistent-enough state; keep going.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reconcile(&self) {
        let retired = self.lock().reconcile();
        retire(retired);
    }
}

/// Lets replaced taps fade out, then tears them down (outside the lock: teardown
/// waits for their IO thread).
fn retire(taps: Vec<Tap>) {
    if !taps.is_empty() {
        thread::sleep(CROSSFADE);
    }
    drop(taps);
}

impl Platform {
    fn update_app(&self, app: &AppAudio, change: impl FnOnce(&mut Level)) -> Result<()> {
        let mut state = self.shared.lock();
        let previous = state.desired.get(&app.id).copied().unwrap_or_default();
        let mut level = previous;
        change(&mut level);
        state.set_level(&app.id, level);
        if !level.is_unity() && state.taps.contains_key(&app.id) {
            state.apply_gains();
            return Ok(());
        }
        self.commit(state, Key::App(app.id.clone()), |state| state.set_level(&app.id, previous))
    }

    fn update_master(&self, device: ObjectId, change: impl FnOnce(&mut Level)) -> Result<()> {
        let mut state = self.shared.lock();
        let previous = state.master;
        change(&mut state.master);
        if state.master_tap.is_some() && state.master_gain(device) < 1.0 {
            state.apply_gains();
            return Ok(());
        }
        self.commit(state, Key::Master, |state| state.master = previous)
    }

    /// Applies a level change that needs taps created or removed. On failure the
    /// level is reverted, except while the permission prompt is pending: then it stays
    /// and the worker applies it once the user allows access.
    fn commit(&self, mut state: MutexGuard<'_, State>, key: Key, revert: impl FnOnce(&mut State)) -> Result<()> {
        let device = match device::default_device(Endpoint::Output) {
            Ok(device) => device,
            Err(e) => {
                revert(&mut state);
                return Err(e);
            }
        };
        if state.wants(&key, device) {
            let allowed = if tap::supported() {
                self.permit(&mut state)
            } else {
                Err(AudioError::Unsupported(UNSUPPORTED_HINT.into()))
            };
            if let Err(e) = allowed {
                if !(state.prompted && state.permission() == Some(Permission::Undetermined)) {
                    revert(&mut state);
                }
                return Err(e);
            }
        }
        let groups = match state.processes.groups(false) {
            Ok(groups) => groups,
            Err(e) => {
                revert(&mut state);
                return Err(e.into());
            }
        };
        if let Key::App(id) = &key
            && state.desired.contains_key(id)
            && !groups.iter().any(|g| &g.id == id)
        {
            revert(&mut state);
            return Err(AudioError::AppNotFound(id.clone()));
        }
        let mut synced = state.sync(device, &groups, Some(&key));
        let failure = synced.failures.iter().position(|(k, _)| *k == key).map(|i| synced.failures.swap_remove(i).1);
        if failure.is_some() {
            revert(&mut state);
            state.apply_gains();
        }
        state.error = failure.as_ref().map(|e| e.to_string());
        let busy = state.has_taps();
        drop(state);
        retire(synced.retired);
        if busy {
            self.worker()?.send(Msg::Wake).ok();
        }
        failure.map_or(Ok(()), Err)
    }

    /// Ok when taps may be created now. Otherwise asks once (the system prompt) and
    /// explains; the prompt's answer wakes the worker.
    fn permit(&self, state: &mut State) -> Result<()> {
        match state.permission() {
            Some(Permission::Granted) | None => Ok(()),
            Some(Permission::Denied) => Err(AudioError::Permission(PERMISSION_HINT.into())),
            Some(Permission::Undetermined) => {
                if !std::mem::replace(&mut state.prompted, true) {
                    let wake = self.worker()?;
                    let granted = state.prompt_granted.clone();
                    // The callback must not take the state lock: it may run before `request` returns.
                    tcc::request(move |ok| {
                        granted.fetch_or(ok, Relaxed);
                        let _ = wake.send(Msg::Wake);
                    });
                }
                Err(AudioError::Permission("approve the System Audio Recording prompt to change volumes".into()))
            }
        }
    }

    /// Starts the worker on first use; returns a handle to wake it.
    fn worker(&self) -> Result<Sender<Msg>> {
        let mut worker = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(worker) = &*worker {
            return Ok(worker.tx.clone());
        }
        let started = Worker::start(self.shared.clone())?;
        let tx = started.tx.clone();
        *worker = Some(started);
        Ok(tx)
    }

    /// Default output plus the controls it lacks, when a software master can stand in.
    fn soft_master(&self) -> Result<(ObjectId, Controls)> {
        let device = device::default_device(Endpoint::Output)?;
        let controls = if tap::supported() {
            self.shared.lock().controls(device)
        } else {
            Controls { volume: true, mute: true } // no taps: the device's controls or nothing
        };
        Ok((device, controls))
    }
}

impl Backend for Platform {
    fn new() -> Self {
        if tap::supported() {
            tap::remove_orphans();
        }
        Platform { shared: Arc::new(Shared { state: Mutex::new(State::default()) }), worker: Mutex::new(None) }
    }

    fn capabilities(&self) -> AudioCapabilities {
        if !tap::supported() {
            return AudioCapabilities { per_app_volume: false, note: Some(UNSUPPORTED_HINT.into()) };
        }
        let device = device::default_device(Endpoint::Output).ok();
        let mut state = self.shared.lock();
        let soft_master = device
            .filter(|&d| !state.controls(d).volume)
            .map(|d| format!("Master volume is applied in software ({} has no volume control).", device::name(d)));
        let (per_app_volume, note) = match state.permission() {
            Some(Permission::Denied) => (false, Some(format!("Per-app volume: {PERMISSION_HINT}."))),
            Some(Permission::Undetermined) => {
                (true, Some("macOS asks for System Audio Recording permission the first time a volume changes.".into()))
            }
            _ => (true, state.error.clone()),
        };
        let note = [note, soft_master].into_iter().flatten().collect::<Vec<_>>().join(" ");
        AudioCapabilities { per_app_volume, note: (!note.is_empty()).then_some(note) }
    }

    fn endpoint(&self, endpoint: Endpoint) -> Result<VolumeState> {
        if endpoint == Endpoint::Input {
            return device::state(device::default_device(endpoint)?, endpoint);
        }
        let (device, controls) = self.soft_master()?;
        if controls.volume && controls.mute {
            return device::state(device, endpoint);
        }
        let soft = self.shared.lock().master;
        Ok(VolumeState {
            volume: if controls.volume { device::volume(device, endpoint)?.unwrap_or(1.0) } else { soft.volume },
            muted: if controls.mute { device::muted(device, endpoint)?.unwrap_or(false) } else { soft.muted },
        })
    }

    fn set_endpoint_volume(&self, endpoint: Endpoint, volume: f32) -> Result<()> {
        if endpoint == Endpoint::Output {
            let (device, controls) = self.soft_master()?;
            if !controls.volume {
                return self.update_master(device, |level| level.volume = volume);
            }
        }
        device::set_volume(device::default_device(endpoint)?, endpoint, volume)
    }

    fn set_endpoint_muted(&self, endpoint: Endpoint, muted: bool) -> Result<()> {
        if endpoint == Endpoint::Output {
            let (device, controls) = self.soft_master()?;
            if !controls.mute {
                return self.update_master(device, |level| level.muted = muted);
            }
        }
        device::set_muted(device::default_device(endpoint)?, endpoint, muted)
    }

    fn apps(&self) -> Result<Vec<AppAudio>> {
        let mut state = self.shared.lock();
        state.listing = state.processes.groups(true)?;
        Ok(state.apps())
    }

    /// Answers from the latest listing when it has the app: listing reads the playing
    /// state of every process (an IPC each), far too slow for a dragged slider.
    fn find(&self, query: &str) -> Result<AppAudio> {
        if let Some(app) = find_app(&self.shared.lock().apps(), query) {
            return Ok(app.clone());
        }
        find_app(&self.apps()?, query).cloned().ok_or_else(|| AudioError::AppNotFound(query.into()))
    }

    fn set_app_volume(&self, app: &AppAudio, volume: f32) -> Result<()> {
        self.update_app(app, |level| level.volume = volume)
    }

    fn set_app_muted(&self, app: &AppAudio, muted: bool) -> Result<()> {
        self.update_app(app, |level| level.muted = muted)
    }

    fn app_levels(&self) -> Vec<AppLevel> {
        let state = self.shared.lock();
        let mut levels: Vec<_> = state
            .desired
            .iter()
            .map(|(app, level)| AppLevel { app: app.clone(), volume: level.volume, muted: level.muted })
            .collect();
        levels.sort_by(|a, b| a.app.cmp(&b.app));
        levels
    }

    /// Levels wait in `desired`; the worker applies them as their apps appear, once
    /// permission is granted. Never prompts.
    fn restore_app_levels(&self, levels: &[AppLevel]) {
        let mut state = self.shared.lock();
        for level in levels.iter().filter(|l| l.volume.is_finite()) {
            state.set_level(&level.app, Level { volume: level.volume.clamp(0.0, 1.0), muted: level.muted });
        }
        let pending = !state.desired.is_empty();
        drop(state);
        if pending
            && tap::supported()
            && let Ok(wake) = self.worker()
        {
            let _ = wake.send(Msg::Wake);
        }
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.get_mut().unwrap_or_else(|e| e.into_inner()).take() {
            worker.stop();
        }
        let taps = self.shared.lock().take_taps();
        drop(taps);
    }
}

enum Msg {
    Wake,
    Stop,
}

/// The sync thread plus its CoreAudio listeners.
struct Worker {
    tx: Sender<Msg>,
    thread: JoinHandle<()>,
    /// Boxed sender handed to CoreAudio as listener client data.
    listener_tx: *mut Sender<Msg>,
}

// SAFETY: `listener_tx` is only dereferenced by CoreAudio listeners (Sender is Sync)
// and freed in `stop` after the listeners are removed.
unsafe impl Send for Worker {}

const WATCHED: [u32; 3] =
    [kAudioHardwarePropertyProcessObjectList, kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices];

impl Worker {
    fn start(shared: Arc<Shared>) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("omni-audio-taps".into())
            .spawn(move || run(&shared, &rx))
            .map_err(|e| AudioError::Backend(format!("could not start audio worker: {e}")))?;
        let listener_tx = Box::into_raw(Box::new(tx.clone()));
        for selector in WATCHED {
            let address = ca::global(selector);
            let status = unsafe {
                AudioObjectAddPropertyListener(ca::SYSTEM, NonNull::from(&address), Some(on_change), listener_tx.cast())
            };
            if status != 0 {
                tracing::warn!("CoreAudio listener not installed: {}", ca::Status(status));
            }
        }
        Ok(Worker { tx, thread, listener_tx })
    }

    fn stop(self) {
        for selector in WATCHED {
            let address = ca::global(selector);
            unsafe {
                AudioObjectRemovePropertyListener(
                    ca::SYSTEM,
                    NonNull::from(&address),
                    Some(on_change),
                    self.listener_tx.cast(),
                );
            }
        }
        // SAFETY: created by `Box::into_raw` in `start`; listeners are gone.
        drop(unsafe { Box::from_raw(self.listener_tx) });
        let _ = self.tx.send(Msg::Stop);
        let _ = self.thread.join();
    }
}

fn run(shared: &Shared, rx: &Receiver<Msg>) {
    loop {
        // Health checks only matter for running taps; pending levels wait for notifications.
        let active = shared.lock().has_taps();
        let message =
            if active { rx.recv_timeout(POLL) } else { rx.recv().map_err(|_| RecvTimeoutError::Disconnected) };
        match message {
            Ok(Msg::Wake) => {
                thread::sleep(DEBOUNCE);
                if rx.try_iter().any(|m| matches!(m, Msg::Stop)) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
        }
        shared.reconcile();
    }
}

unsafe extern "C-unwind" fn on_change(
    _object: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    client: *mut c_void,
) -> i32 {
    if let Some(tx) = unsafe { client.cast::<Sender<Msg>>().as_ref() } {
        let _ = tx.send(Msg::Wake);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::testing::{self, Tone, require_permission};
    use super::*;

    fn tap_peaks(platform: &Platform, id: &str) -> Option<(f32, f32)> {
        platform.shared.lock().taps.get(id).map(|tap| tap.take_peaks())
    }

    /// Output / input peak ratio of a tap over half a second, once its gain has settled.
    fn gain_of(peaks: impl Fn() -> Option<(f32, f32)>) -> f32 {
        thread::sleep(Duration::from_millis(250));
        peaks();
        thread::sleep(Duration::from_millis(500));
        let (input, output) = peaks().expect("tap running");
        assert!(input > 0.005, "tap delivers silence");
        output / input
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Software master on an output without its own volume, alone and stacked with an
    /// app tap (the app leaves the master tap, its own tap carries the master gain).
    /// Briefly lowers (and for 0.75 s mutes) everything you hear.
    #[test]
    #[ignore]
    fn software_master_composes_with_app_taps() {
        require_permission();
        let platform = Platform::new();
        let (_, controls) = platform.soft_master().unwrap();
        if controls.volume {
            eprintln!("skipped: the default output has its own volume control");
            return;
        }
        let master_peaks = || platform.shared.lock().master_tap.as_ref().map(|t| t.take_peaks());
        let master_excludes = |object| {
            platform.shared.lock().master_tap.as_ref().is_some_and(|t| t.source.objects().any(|o| o == object))
        };
        let tone = Tone::play();
        let object = tone.object();

        platform.set_endpoint_volume(Endpoint::Output, 0.8).unwrap();
        assert_eq!(platform.endpoint(Endpoint::Output).unwrap(), VolumeState { volume: 0.8, muted: false });
        assert!(platform.capabilities().note.unwrap().contains("software"));
        let master = 0.8f32.powi(3);
        let cpu = testing::cpu_time();
        assert!((gain_of(master_peaks) - master).abs() < 0.02);
        eprintln!("master tap: process cpu {:.2}%", (testing::cpu_time() - cpu).as_secs_f64() / 0.75 * 100.0);
        assert!(master_excludes(own_process().unwrap()), "our own output must never be tapped");
        assert!(!master_excludes(object));

        let app = platform.find("afplay").unwrap();
        platform.set_app_volume(&app, 0.5).unwrap();
        assert!(master_excludes(object), "a tapped app must leave the master tap");
        let stacked = gain_of(|| tap_peaks(&platform, "afplay"));
        eprintln!("app 50% under master 80%: gain {stacked:.4}");
        assert!((stacked - 0.125 * master).abs() < 0.01);

        platform.set_endpoint_muted(Endpoint::Output, true).unwrap();
        thread::sleep(Duration::from_millis(250));
        tap_peaks(&platform, "afplay");
        master_peaks();
        thread::sleep(Duration::from_millis(500));
        assert_eq!(tap_peaks(&platform, "afplay").unwrap().1, 0.0);
        assert_eq!(master_peaks().unwrap().1, 0.0);
        platform.set_endpoint_muted(Endpoint::Output, false).unwrap();

        platform.set_app_volume(&app, 1.0).unwrap();
        assert!(platform.shared.lock().taps.is_empty());
        assert!(!master_excludes(object), "a released app rejoins the master tap");
        platform.set_endpoint_volume(Endpoint::Output, 1.0).unwrap();
        assert!(!platform.shared.lock().has_taps());
        assert_eq!(platform.endpoint(Endpoint::Output).unwrap(), VolumeState { volume: 1.0, muted: false });
    }

    /// Persisted levels wait for their app and apply when it starts (notification
    /// driven: nothing polls while no tap runs).
    #[test]
    #[ignore]
    fn restored_level_applies_when_app_starts() {
        require_permission();
        let platform = Platform::new();
        let saved = AppLevel { app: "afplay".into(), volume: 0.5, muted: false };
        platform.restore_app_levels(std::slice::from_ref(&saved));
        assert_eq!(platform.app_levels(), [saved]);
        assert!(!platform.shared.lock().has_taps());

        let tone = Tone::play();
        let object = tone.object();
        wait_for("the restored level", || {
            platform.shared.lock().taps.get("afplay").is_some_and(|t| t.source.objects().any(|o| o == object))
        });
        assert!((gain_of(|| tap_peaks(&platform, "afplay")) - 0.125).abs() < 0.01);

        let app = platform.find("afplay").unwrap();
        assert_eq!(app.volume, 0.5);
        platform.set_app_volume(&app, 1.0).unwrap();
        assert!(platform.app_levels().is_empty());
    }

    /// The level outlives the process: a restarted app is tapped again by the worker.
    /// Real hardware: `cargo test -p omni-audio -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn app_level_follows_restarts() {
        require_permission();
        let platform = Platform::new();
        let first = Tone::play();
        first.object();
        let app = platform.apps().unwrap().into_iter().find(|a| a.id == "afplay").expect("afplay listed");
        assert!(app.playing);

        platform.set_app_volume(&app, 0.5).unwrap();
        assert_eq!(platform.apps().unwrap().iter().find(|a| a.id == "afplay").map(|a| a.volume), Some(0.5));

        drop(first);
        let second = Tone::play();
        let object = second.object();
        wait_for("the restarted app's tap", || {
            platform.shared.lock().taps.get("afplay").is_some_and(|t| t.source.objects().any(|o| o == object))
        });
        assert!((gain_of(|| tap_peaks(&platform, "afplay")) - 0.125).abs() < 0.01);

        platform.set_app_volume(&app, 1.0).unwrap();
        assert!(platform.shared.lock().taps.is_empty());
    }
}
