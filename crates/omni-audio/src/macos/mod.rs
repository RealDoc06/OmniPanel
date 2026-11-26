//! macOS backend: CoreAudio device volume, process taps for per-app volume, per-app
//! output device and software master volume.
//!
//! macOS has no per-app volume or output device, so both are ours: kept in
//! `State::desired` (keyed by app id, so they survive app restarts) and applied with a
//! [`Tap`], which mutes what the app plays on the default output and plays it back at
//! our level, on the default output or on the device the app is sent to. An app at
//! 100%, unmuted and on the default output has no tap and plays natively. Taps only
//! take what an app plays on the default output: audio it sends to another device by
//! itself keeps its native level and route. An app sent to a device stays on it when
//! the default output changes, and plays on the default output while that device is
//! not connected. Only "system default" (no device) makes it follow the default again.
//!
//! What another app records from cannot be changed: macOS has no public way to
//! redirect a process's input, so `set_app_device` reports that for microphones.
//!
//! Outputs without a volume (or mute) control of their own get a software master:
//! one tap of everything played to the device except OmniPanel and the apps that
//! have their own tap. Those apps are re-rendered by our process, so their taps carry
//! the master gain too, unless they play on another device. At 100% and unmuted the
//! master tap is removed.
//!
//! A worker thread, started on first use, keeps taps in sync with the system:
//! re-attaches restarted apps, follows helper processes and default-output changes.
//! It sleeps on CoreAudio change notifications and only polls while taps exist or
//! wanted ones are missing (a failed build, permission not granted yet).

mod ca;
pub mod capture;
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
    kAudioDeviceTransportTypeAggregate, kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices,
    kAudioHardwarePropertyProcessObjectList, kAudioHardwarePropertyTranslatePIDToProcessObject,
};
use omni_core::AppLevel;

use self::ca::ObjectId;
use self::process::{AppGroup, Processes};
use self::tap::{Source, Tap};
use self::tcc::Permission;
use crate::{
    AppAudio, AppRoute, AudioCapabilities, AudioDevices, AudioError, Backend, Endpoint, Result, VolumeState, find_app,
    find_device,
};

/// Health check / fallback poll while taps exist or wanted ones are missing: dead or
/// stalled devices, sample-rate switches, failed builds, anything notifications miss.
/// Cheap: cached identities; tccd is only asked (per `PERMISSION_TTL`) until granted.
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
const OUTPUT_UNSUPPORTED_HINT: &str = "Choosing a speaker per app needs macOS 14.2 or later.";
const AGGREGATE_HINT: &str =
    "Apps cannot be sent to a speaker while the default output is an aggregate or multi-output device.";
const MICROPHONE_HINT: &str = "macOS has no way to change another app's microphone. Pick it in the app's own settings.";

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
    /// What the user chose per app id, where it is not the default.
    desired: HashMap<String, Wanted>,
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
    /// The last pass left wanted taps missing (a build failed or waits out its backoff,
    /// the system could not be read, permission is not granted): the worker polls.
    retry: bool,
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

/// What the user chose for one app.
#[derive(Debug, Clone, Default, PartialEq)]
struct Wanted {
    level: Level,
    /// UID of the output device the app is sent to; `None` follows the default output.
    output: Option<String>,
}

impl Wanted {
    /// Nothing to apply or remember: the app plays natively.
    fn is_default(&self) -> bool {
        self.level.is_unity() && self.output.is_none()
    }

    /// What to go back to when applying `self` in place of `previous` fails. Letting
    /// go of a device needs no tap, so that part holds: an app let go while its level
    /// cannot be applied (the default output is a multi-output device) must not come
    /// back pinned.
    fn undone(&self, previous: &Wanted) -> Wanted {
        Wanted { output: previous.output.clone().filter(|_| self.output.is_some()), ..previous.clone() }
    }

    /// Where the app's tap plays and at which gain, or `None` when it needs no tap.
    /// `route` is the device it is sent to, while connected. On another device than
    /// `default` it plays at its own level; on the default output the software
    /// `master` applies too, and at 100% it plays there natively.
    fn placement(&self, route: Option<ObjectId>, default: ObjectId, master: f32) -> Option<(ObjectId, f32)> {
        match route.filter(|&device| device != default) {
            Some(device) => Some((device, self.level.gain())),
            None => (!self.level.is_unity()).then(|| (default, self.level.gain() * master)),
        }
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
    /// we hold a level or device for. Background daemons that merely opened CoreAudio
    /// are hidden. An app's output is the device chosen for it, connected or not.
    fn apps(&self) -> Vec<AppAudio> {
        self.listing
            .iter()
            .filter(|g| g.playing || g.regular || self.desired.contains_key(&g.id))
            .map(|g| {
                let wanted = self.desired.get(&g.id).cloned().unwrap_or_default();
                AppAudio {
                    id: g.id.clone(),
                    name: g.name.clone(),
                    pid: u32::try_from(g.pid).ok(),
                    volume: wanted.level.volume,
                    muted: wanted.level.muted,
                    playing: g.playing,
                    output: tap::supported()
                        .then(|| wanted.output.map_or(AppRoute::Default, |id| AppRoute::Device { id })),
                    input: None,
                }
            })
            .collect()
    }

    /// What `desired` holds, to persist.
    fn levels(&self) -> Vec<AppLevel> {
        let mut levels: Vec<_> = self
            .desired
            .iter()
            .map(|(app, wanted)| AppLevel {
                app: app.clone(),
                volume: wanted.level.volume,
                muted: wanted.level.muted,
                output: wanted.output.clone(),
            })
            .collect();
        levels.sort_by(|a, b| a.app.cmp(&b.app));
        levels
    }

    fn restore(&mut self, levels: &[AppLevel]) {
        for saved in levels.iter().filter(|l| l.volume.is_finite()) {
            let level = Level { volume: saved.volume.clamp(0.0, 1.0), muted: saved.muted };
            self.set_wanted(&saved.app, Wanted { level, output: saved.output.clone() });
        }
    }

    fn set_wanted(&mut self, app: &str, wanted: Wanted) {
        if wanted.is_default() {
            self.desired.remove(app);
        } else {
            self.desired.insert(app.to_string(), wanted);
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
        let Some(device) = self.master_tap.as_ref().or(self.taps.values().next()).map(|t| t.from) else { return };
        let master = self.master_gain(device);
        for (id, tap) in &self.taps {
            let level = self.desired.get(id).map(|w| w.level).unwrap_or_default();
            // The software master stands in for the default output's controls only: not
            // for an app played on another device than the one it is tapped on.
            tap.set_gain(level.gain() * if tap.device == tap.from { master } else { 1.0 });
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

    /// Creates, retargets, rebuilds and removes taps to match the levels and output
    /// devices, `device` being the default output. `forced` skips the failure backoff
    /// (the user just asked for it).
    fn sync(&mut self, device: ObjectId, groups: &[AppGroup], forced: Option<&Key>) -> Synced {
        let master = self.master_gain(device);
        let mut synced = Synced::default();
        let State { desired, taps, master_tap, backoff, .. } = self;
        let present = |id: &str| groups.iter().any(|g| g.id == id);
        // Where each running app with a tap to keep plays, and how loud.
        let wanted: HashMap<&String, (ObjectId, f32)> = desired
            .iter()
            .filter(|(id, _)| present(id))
            .filter_map(|(id, wanted)| {
                let route = wanted.output.as_deref().and_then(ca::device_with_uid);
                Some((id, wanted.placement(route, device, master)?))
            })
            .collect();
        let released: Vec<(String, Tap)> = taps.extract_if(|id, _| !wanted.contains_key(id)).collect();
        for (&id, &(output, gain)) in &wanted {
            let Some(group) = groups.iter().find(|g| &g.id == id) else { continue };
            let key = Key::App(id.clone());
            let slot = Slot { key: &key, label: &group.name, source: Source::Processes(group.objects.clone()) };
            if let Some(tap) = slot.ensure(taps.remove(id), device, output, gain, forced, backoff, &mut synced) {
                taps.insert(id.clone(), tap);
            }
        }

        // After the app taps, so a newly tapped app leaves the master only once its own
        // tap is running, and a released one rejoins before its tap goes. An app whose
        // tap is still on the previous output (its rebuild failed) stays in the master.
        let rejoined = if master < 1.0 {
            match own_process() {
                Some(own) => {
                    let mut excluded: Vec<ObjectId> =
                        taps.values().filter(|t| t.from == device).flat_map(|t| t.source.objects()).collect();
                    excluded.push(own);
                    excluded.sort_unstable();
                    excluded.dedup();
                    let source = Source::AllExcept(excluded);
                    let slot = Slot { key: &Key::Master, label: "master", source: source.clone() };
                    *master_tap = slot.ensure(master_tap.take(), device, device, master, forced, backoff, &mut synced);
                    master_tap.as_ref().is_some_and(|t| t.device == device && t.source == source)
                }
                None => {
                    let e = AudioError::Backend("OmniPanel's audio process is not registered".into());
                    synced.failures.push((Key::Master, e));
                    false
                }
            }
        } else {
            if let Some(old) = master_tap.take() {
                old.set_gain(1.0); // everything plays natively at full level next
                synced.retired.push(old);
            }
            true
        };

        // Hand-over gains: a released app rejoins the master tap (fade out) or plays
        // natively at full level (ramp to 1 so nothing jumps); one that played on
        // another device fades out there. While the master cannot take it back (failed,
        // or waiting out its backoff), the app's tap stays on: dropping it would play
        // the app at full level. On the default output it carries the master gain
        // meanwhile; on another device it keeps playing there until the master is back.
        for (id, tap) in released {
            let elsewhere = tap.device != device;
            if rejoined || !present(&id) {
                tap.set_gain(if elsewhere || master < 1.0 { 0.0 } else { 1.0 });
                synced.retired.push(tap);
            } else {
                if !elsewhere {
                    tap.set_gain(master);
                }
                taps.insert(id, tap);
            }
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
        if !(permitted && wanted) {
            // Access granted in System Settings sends no notification: keep checking.
            self.retry = wanted;
            if !wanted {
                self.error = None; // nothing left that could still be failing
            }
            return self.take_taps();
        }
        let (device, groups) = match device.and_then(|device| Ok((device, self.processes.groups(false)?))) {
            Ok(read) => read,
            Err(e) => {
                // Running taps keep their levels meanwhile. Logged once, not every poll.
                let e = e.to_string();
                if self.error.as_ref() != Some(&e) {
                    tracing::warn!("audio devices unreadable: {e}");
                }
                self.error = Some(e);
                self.retry = true;
                return Vec::new();
            }
        };
        let synced = self.sync(device, &groups, None);
        for (key, e) in &synced.failures {
            // A lasting failure comes back with every retry: logged when it changes.
            let e = e.to_string();
            if self.error.as_ref() != Some(&e) {
                tracing::warn!("{key:?}: {e}");
            }
        }
        self.settle(synced.failures.last().map(|(_, e)| e), synced.skipped);
        synced.retired
    }

    /// Records how a `sync` went: the failure for the mixer note (kept while a failed
    /// tap waits out its backoff), and whether the worker has to try again.
    fn settle(&mut self, failure: Option<&AudioError>, skipped: bool) {
        match failure {
            Some(e) => self.error = Some(e.to_string()),
            None if !skipped => self.error = None,
            None => {}
        }
        self.retry = failure.is_some() || skipped;
    }

    fn take_taps(&mut self) -> Vec<Tap> {
        let master = self.master_tap.take();
        self.taps.drain().map(|(_, tap)| tap).chain(master).collect()
    }

    fn has_taps(&self) -> bool {
        !self.taps.is_empty() || self.master_tap.is_some()
    }

    /// The worker comes back by itself (`POLL`): running taps need health checks,
    /// missing ones a retry. Otherwise it waits for notifications.
    fn needs_poll(&self) -> bool {
        self.has_taps() || self.retry
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
    /// Returns the tap to keep, listening on `from` (the default output) and playing
    /// on `device`: `current` retargeted, or a new one. A broken tap goes before its
    /// replacement is built: a process tapped twice while it starts up can stay
    /// silent. A working tap whose retarget fails keeps its previous source until a
    /// retry succeeds, as dropping it would play its audio at full level. A working
    /// tap that has to move (the default output changed, or the app is sent to
    /// another device) keeps playing where it was until its replacement runs, then
    /// fades out. For that moment the app is tapped twice, as it is whenever it joins
    /// or leaves the master tap; dropping the old tap first would play the app on the
    /// default output at full level in between.
    #[allow(clippy::too_many_arguments)]
    fn ensure(
        self,
        current: Option<Tap>,
        from: ObjectId,
        device: ObjectId,
        gain: f32,
        forced: Option<&Key>,
        backoff: &mut HashMap<Key, Instant>,
        synced: &mut Synced,
    ) -> Option<Tap> {
        let current = match current {
            Some(mut tap) if (tap.from, tap.device) == (from, device) => {
                if tap.healthy() {
                    tap.set_gain(gain);
                    // A cheap property write, so retried every pass (no backoff): a user
                    // change hands over at once and nothing delays a later rebuild.
                    if tap.source != self.source
                        && let Err(e) = tap.set_source(&self.source)
                    {
                        synced.failures.push((self.key.clone(), e));
                    }
                    return Some(tap);
                }
                drop(tap);
                None
            }
            // Has to move: only a working tap is worth keeping meanwhile.
            Some(mut tap) => tap.healthy().then_some(tap),
            None => None,
        };
        if forced != Some(self.key) && backoff.get(self.key).is_some_and(|until| Instant::now() < *until) {
            synced.skipped = true;
            return current;
        }
        match Tap::new(self.label, self.source, from, device, gain) {
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

/// An aggregate or multi-output device: a tap can neither listen to one nor play on one.
fn aggregate(device: ObjectId) -> bool {
    device::transport(device) == kAudioDeviceTransportTypeAggregate
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
    fn update_app(&self, app: &AppAudio, change: impl FnOnce(&mut Wanted)) -> Result<()> {
        let mut state = self.shared.lock();
        let previous = state.desired.get(&app.id).cloned().unwrap_or_default();
        let mut wanted = previous.clone();
        change(&mut wanted);
        state.set_wanted(&app.id, wanted.clone());
        // A level change on a tap that stays where it is.
        if !wanted.level.is_unity() && wanted.output == previous.output && state.taps.contains_key(&app.id) {
            state.apply_gains();
            return Ok(());
        }
        let undone = wanted.undone(&previous);
        self.commit(state, Key::App(app.id.clone()), |state| state.set_wanted(&app.id, undone))
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
    /// and the worker applies it once the user allows access. Another tap failing on
    /// the way (the master, when an app joins or leaves it) fails the call too but
    /// keeps the level: the taps it could not hand over stay as they were, and the
    /// worker retries.
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
        for (k, e) in &synced.failures {
            tracing::warn!("{k:?}: {e}");
        }
        // The caller's own failure is undone and returned, so it is not kept as the
        // mixer note: only what the worker still has to retry is.
        let own = synced.failures.iter().position(|(k, _)| *k == key).map(|own| synced.failures.swap_remove(own).1);
        if own.is_some() {
            revert(&mut state);
            state.apply_gains();
        }
        let other = synced.failures.pop().map(|(_, e)| e);
        state.settle(other.as_ref(), synced.skipped);
        // A level or device held without a tap (an app pinned to what is the default
        // output right now) still needs the worker: it hears when that changes.
        let (busy, held) = (state.needs_poll(), !state.desired.is_empty());
        drop(state);
        retire(synced.retired);
        if busy || held {
            let wake = self.worker()?;
            if busy {
                wake.send(Msg::Wake).ok();
            }
        }
        own.or(other).map_or(Ok(()), Err)
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
                Err(AudioError::Permission(
                    "approve the System Audio Recording prompt to change app volumes and speakers".into(),
                ))
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
            return AudioCapabilities {
                per_app_volume: false,
                note: Some(UNSUPPORTED_HINT.into()),
                per_app_output: false,
                output_note: Some(OUTPUT_UNSUPPORTED_HINT.into()),
                per_app_input: false,
                input_note: Some(MICROPHONE_HINT.into()),
            };
        }
        let device = device::default_device(Endpoint::Output).ok().map(|d| (d, device::name(d)));
        let mut state = self.shared.lock();
        let soft_master = device
            .as_ref()
            .filter(|(d, _)| !state.controls(*d).volume)
            .map(|(_, name)| format!("Master volume is applied in software ({name} has no volume control)."));
        let (per_app_volume, note) = match state.permission() {
            Some(Permission::Denied) => (false, Some(format!("Per-app volume and speakers: {PERMISSION_HINT}."))),
            Some(Permission::Undetermined) => (
                true,
                Some(
                    "macOS asks for System Audio Recording permission the first time a volume or speaker changes."
                        .into(),
                ),
            ),
            _ => (true, state.error.clone()),
        };
        // Taps listen to the default output only (see the module docs).
        let scope = device
            .as_ref()
            .filter(|_| per_app_volume)
            .map(|(_, name)| format!("App volumes and speakers only affect audio played on {name}."));
        // Both are taps: what stops per-app volume (the permission) stops per-app outputs.
        // So does a default output a tap cannot listen to.
        let aggregate = device.as_ref().is_some_and(|(d, _)| aggregate(*d));
        let output_note =
            note.clone().filter(|_| !per_app_volume).or_else(|| aggregate.then(|| AGGREGATE_HINT.to_string()));
        let note = [note, scope, soft_master].into_iter().flatten().collect::<Vec<_>>().join(" ");
        AudioCapabilities {
            per_app_volume,
            note: (!note.is_empty()).then_some(note),
            per_app_output: per_app_volume && !aggregate,
            output_note,
            per_app_input: false,
            input_note: Some(MICROPHONE_HINT.into()),
        }
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
        self.update_app(app, |wanted| wanted.level.volume = volume)
    }

    fn set_app_muted(&self, app: &AppAudio, muted: bool) -> Result<()> {
        self.update_app(app, |wanted| wanted.level.muted = muted)
    }

    fn app_levels(&self) -> Vec<AppLevel> {
        self.shared.lock().levels()
    }

    /// Levels and devices wait in `desired`; the worker applies them as their apps
    /// appear, once permission is granted. Never prompts.
    fn restore_app_levels(&self, levels: &[AppLevel]) {
        let mut state = self.shared.lock();
        state.restore(levels);
        let pending = !state.desired.is_empty();
        drop(state);
        if pending
            && tap::supported()
            && let Ok(wake) = self.worker()
        {
            let _ = wake.send(Msg::Wake);
        }
    }

    /// Outputs only: see `MICROPHONE_HINT`.
    fn devices(&self) -> Result<AudioDevices> {
        let outputs = if tap::supported() { device::outputs()? } else { Vec::new() };
        Ok(AudioDevices { outputs, inputs: Vec::new() })
    }

    /// A device pins the app to it, the default output included; `None` lets it
    /// follow the default output again.
    fn set_app_device(&self, app: &str, direction: Endpoint, device: Option<&str>) -> Result<()> {
        if direction == Endpoint::Input {
            return Err(AudioError::Unsupported(MICROPHONE_HINT.into()));
        }
        if !tap::supported() {
            return Err(AudioError::Unsupported(OUTPUT_UNSUPPORTED_HINT.into()));
        }
        let output = match device {
            // Going back to the default output stays possible.
            Some(_) if device::default_device(Endpoint::Output).is_ok_and(aggregate) => {
                return Err(AudioError::Unsupported(AGGREGATE_HINT.into()));
            }
            Some(query) => Some(find_device(&device::outputs()?, query)?.id.clone()),
            None => None,
        };
        // From a fresh listing, unlike the levels of a dragged slider: an app started
        // since the last one must not lose to an older, looser match.
        let app = find_app(&self.apps()?, app).cloned().ok_or_else(|| AudioError::AppNotFound(app.into()))?;
        self.update_app(&app, |wanted| wanted.output = output)
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
        let poll = shared.lock().needs_poll();
        let message = if poll { rx.recv_timeout(POLL) } else { rx.recv().map_err(|_| RecvTimeoutError::Disconnected) };
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

    /// A failed or postponed build leaves no tap to health-check, yet the worker must
    /// come back for it instead of sleeping until the next CoreAudio notification.
    #[test]
    fn missing_taps_keep_the_worker_polling() {
        let mut state = State::default();
        state.settle(Some(&AudioError::Backend("tap failed".into())), false);
        assert!(!state.has_taps() && state.needs_poll());
        state.settle(None, true);
        assert!(state.needs_poll(), "waiting out the backoff");
        assert_eq!(state.error.as_deref(), Some("tap failed"), "the note keeps the failure meanwhile");
        state.settle(None, false);
        assert!(!state.needs_poll() && state.error.is_none());
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
    /// driven: nothing polls while the app is not running).
    #[test]
    #[ignore]
    fn restored_level_applies_when_app_starts() {
        require_permission();
        let platform = Platform::new();
        let saved = AppLevel { app: "afplay".into(), volume: 0.5, muted: false, output: None };
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

    /// A tap is needed for a level, or to play on another device than the default
    /// output; the software master only reaches what plays on the default output.
    #[test]
    fn taps_play_where_the_app_is_sent() {
        let (default, headphones, master) = (10, 20, 0.5);
        let wanted = |volume, output: Option<&str>| Wanted {
            level: Level { volume, muted: false },
            output: output.map(str::to_string),
        };
        let quiet = wanted(0.5, None);
        assert_eq!(quiet.placement(None, default, master), Some((default, 0.125 * master)));
        // Sent to the headphones: played there, at its own level only.
        let sent = wanted(1.0, Some("headphones"));
        assert_eq!(sent.placement(Some(headphones), default, master), Some((headphones, 1.0)));
        assert_eq!(
            wanted(0.5, Some("headphones")).placement(Some(headphones), default, master),
            Some((headphones, 0.125))
        );
        // Unplugged, or the headphones became the default output: it plays natively.
        assert_eq!(sent.placement(None, default, master), None);
        assert_eq!(sent.placement(Some(headphones), headphones, master), None);
        assert!(!sent.is_default(), "the choice is kept for when they are back");
        assert!(wanted(1.0, None).is_default());

        // A change that fails is undone, except for letting go of a device.
        let pinned = wanted(0.5, Some("headphones"));
        assert_eq!(wanted(0.2, Some("headphones")).undone(&pinned), pinned);
        assert_eq!(wanted(0.5, Some("speakers")).undone(&pinned), pinned);
        assert_eq!(wanted(0.5, None).undone(&pinned), wanted(0.5, None));
    }

    /// An output device is saved like a level, also for an app at 100%.
    #[test]
    fn saved_levels_keep_the_output() {
        let saved = [
            AppLevel { app: "com.hnc.Discord".into(), volume: 0.5, muted: true, output: None },
            AppLevel {
                app: "com.spotify.client".into(),
                volume: 1.0,
                muted: false,
                output: Some("BuiltInSpeakerDevice".into()),
            },
        ];
        let mut state = State::default();
        state.restore(&saved);
        state.restore(&[AppLevel { app: "afplay".into(), volume: 1.0, muted: false, output: None }]);
        assert_eq!(state.levels(), saved);
    }

    fn route_of(platform: &Platform, id: &str) -> Option<AppRoute> {
        platform.apps().unwrap().into_iter().find(|a| a.id == id).expect("app listed").output
    }

    fn tap_devices(platform: &Platform, id: &str) -> Option<(ObjectId, ObjectId)> {
        platform.shared.lock().taps.get(id).map(|t| (t.from, t.device))
    }

    /// The default output and the other real outputs, as (device, info).
    fn outputs(platform: &Platform) -> (ObjectId, Vec<(ObjectId, crate::AudioDeviceInfo)>) {
        let outputs = platform.devices().unwrap().outputs;
        let object = |d: &crate::AudioDeviceInfo| ca::device_with_uid(&d.id).expect("listed device");
        let default = outputs.iter().find(|d| d.is_default).map(object).expect("a default output");
        let others = outputs.into_iter().filter(|d| !d.is_default && !d.is_virtual).map(|d| (object(&d), d)).collect();
        (default, others)
    }

    /// An app sent to another output is played there by its tap, at its own level and
    /// without dropouts, stays pinned to an output chosen by id, and follows the
    /// default output again once that is chosen. Plays the quiet tone on a second
    /// output device for about ten seconds.
    #[test]
    #[ignore]
    fn app_plays_on_the_output_it_is_sent_to() {
        require_permission();
        let platform = Platform::new();
        let (default, others) = outputs(&platform);
        let Some((other, info)) = others.first() else {
            eprintln!("skipped: needs a second output device");
            return;
        };
        let device = |id: &str| Some(AppRoute::Device { id: id.into() });
        let tone = Tone::play();
        tone.object();
        assert_eq!(route_of(&platform, "afplay"), Some(AppRoute::Default));

        // By name, like a hand-written action.
        platform.set_app_device("afplay", Endpoint::Output, Some(&info.name)).unwrap();
        eprintln!("afplay -> {}", info.name);
        assert_eq!(
            tap_devices(&platform, "afplay"),
            Some((default, *other)),
            "tapped on the default, played on the other"
        );
        assert_eq!(route_of(&platform, "afplay"), device(&info.id));
        assert!((gain_of(|| tap_peaks(&platform, "afplay")) - 1.0).abs() < 0.02);
        assert!(testing::running(*other), "{} is not playing", info.name);

        // Its level is its own there. Four times the largest step of the 440 Hz tone
        // still is far below what a dropout or a drifting clock makes.
        platform.set_app_volume(&platform.find("afplay").unwrap(), 0.5).unwrap();
        assert!((gain_of(|| tap_peaks(&platform, "afplay")) - 0.125).abs() < 0.01);
        let step = || platform.shared.lock().taps.get("afplay").map(|t| t.take_step()).expect("tap running");
        step();
        thread::sleep(Duration::from_secs(8));
        let (step, limit) = (step(), 4.0 * testing::TONE_STEP);
        eprintln!("largest step in 8 s: {step:.5} (limit {limit:.5})");
        assert!(step < limit, "the tone is not continuous on {}", info.name);
        let saved = AppLevel { app: "afplay".into(), volume: 0.5, muted: false, output: Some(info.id.clone()) };
        assert_eq!(platform.app_levels(), [saved]);

        let microphone = platform.set_app_device("afplay", Endpoint::Input, None);
        assert!(matches!(microphone, Err(AudioError::Unsupported(_))), "{microphone:?}");
        let missing = platform.set_app_device("afplay", Endpoint::Output, Some("no such speaker"));
        assert!(matches!(missing, Err(AudioError::DeviceNotFound(_))), "{missing:?}");
        assert_eq!(tap_devices(&platform, "afplay"), Some((default, *other)));

        // Pinned to the default output: nothing to move, only the level keeps its tap.
        let default_uid = device::uid(default).unwrap();
        platform.set_app_device("afplay", Endpoint::Output, Some(&default_uid)).unwrap();
        assert_eq!(tap_devices(&platform, "afplay"), Some((default, default)));
        assert_eq!(route_of(&platform, "afplay"), device(&default_uid));
        assert!((gain_of(|| tap_peaks(&platform, "afplay")) - 0.125).abs() < 0.01);
        platform.set_app_volume(&platform.find("afplay").unwrap(), 1.0).unwrap();
        assert!(platform.shared.lock().taps.is_empty(), "at 100% on the default output it plays natively");
        assert_eq!(platform.app_levels().len(), 1, "the pin is kept");

        platform.set_app_device("afplay", Endpoint::Output, None).unwrap();
        assert_eq!(route_of(&platform, "afplay"), Some(AppRoute::Default));
        assert!(platform.app_levels().is_empty());
    }

    /// Switching a playing app between outputs over and over never leaves it silent:
    /// the old tap hands over to the new one while both listen to the app.
    /// Plays the quiet tone on every real output in turn for a few seconds.
    #[test]
    #[ignore]
    fn switching_outputs_never_silences_the_app() {
        require_permission();
        let platform = Platform::new();
        let (default, others) = outputs(&platform);
        if others.is_empty() {
            eprintln!("skipped: needs a second output device");
            return;
        }
        let tone = Tone::play();
        tone.object();
        // A level first, so every switch replaces a running tap (also back to the default).
        platform.set_app_volume(&platform.find("afplay").unwrap(), 0.5).unwrap();
        let default_uid = device::uid(default).unwrap();
        let stops: Vec<(ObjectId, &str)> = others
            .iter()
            .map(|(object, info)| (*object, info.id.as_str()))
            .chain([(default, default_uid.as_str())])
            .collect();
        for round in 0..4 {
            for (object, uid) in &stops {
                platform.set_app_device("afplay", Endpoint::Output, Some(uid)).unwrap();
                assert_eq!(tap_devices(&platform, "afplay"), Some((default, *object)));
                thread::sleep(Duration::from_millis(150));
                tap_peaks(&platform, "afplay");
                thread::sleep(Duration::from_millis(250));
                let (input, output) = tap_peaks(&platform, "afplay").expect("tap running");
                eprintln!("round {round}: on {uid}: in {input:.4} out {output:.4}");
                assert!(input > 0.005 && output > 0.0005, "silent after switching to {uid}");
            }
        }
        platform.set_app_device("afplay", Endpoint::Output, None).unwrap();
        platform.set_app_volume(&platform.find("afplay").unwrap(), 1.0).unwrap();
        assert!(!platform.shared.lock().has_taps());
    }

    /// Makes a device the system default output until dropped.
    struct DefaultOutput(ObjectId);

    impl DefaultOutput {
        fn switch_to(device: ObjectId) -> Self {
            let previous = DefaultOutput(device::default_device(Endpoint::Output).unwrap());
            ca::set(ca::SYSTEM, ca::global(kAudioHardwarePropertyDefaultOutputDevice), &device).unwrap();
            previous
        }
    }

    impl Drop for DefaultOutput {
        fn drop(&mut self) {
            let _ = ca::set(ca::SYSTEM, ca::global(kAudioHardwarePropertyDefaultOutputDevice), &self.0);
        }
    }

    /// An app pinned to an output plays there after the default output changed, also
    /// when the pin needed no tap until then (it was the default output): the worker
    /// is what notices. `afplay` keeps the device it opened, so a second one is
    /// started after the switch. Makes another device the system default output for
    /// about two seconds.
    #[test]
    #[ignore]
    fn pinned_app_stays_when_the_default_output_changes() {
        require_permission();
        let platform = Platform::new();
        let (default, others) = outputs(&platform);
        let Some((other, info)) = others.first() else {
            eprintln!("skipped: needs a second output device");
            return;
        };
        let first = Tone::play();
        first.object();
        platform.set_app_device("afplay", Endpoint::Output, Some(&device::uid(default).unwrap())).unwrap();
        assert!(!platform.shared.lock().has_taps(), "on the default output it plays natively");

        let switched = DefaultOutput::switch_to(*other);
        eprintln!("default output -> {}", info.name);
        drop(first);
        let second = Tone::play();
        second.object();
        wait_for("the tap that keeps it on its speaker", || {
            tap_devices(&platform, "afplay") == Some((*other, default))
        });
        assert!((gain_of(|| tap_peaks(&platform, "afplay")) - 1.0).abs() < 0.02);
        drop(switched);
        wait_for("native playback again", || !platform.shared.lock().has_taps());
        platform.set_app_device("afplay", Endpoint::Output, None).unwrap();
        assert!(platform.app_levels().is_empty());
    }

    /// A saved output device waits for its app like a saved level does.
    #[test]
    #[ignore]
    fn restored_output_applies_when_app_starts() {
        require_permission();
        let platform = Platform::new();
        let (_, others) = outputs(&platform);
        let Some((other, info)) = others.first() else {
            eprintln!("skipped: needs a second output device");
            return;
        };
        let saved = AppLevel { app: "afplay".into(), volume: 1.0, muted: false, output: Some(info.id.clone()) };
        platform.restore_app_levels(std::slice::from_ref(&saved));
        assert_eq!(platform.app_levels(), [saved]);

        let _tone = Tone::play();
        wait_for("the restored output", || {
            tap_devices(&platform, "afplay").is_some_and(|(_, device)| device == *other)
        });
        assert!((gain_of(|| tap_peaks(&platform, "afplay")) - 1.0).abs() < 0.02);

        platform.set_app_device("afplay", Endpoint::Output, None).unwrap();
        assert!(platform.shared.lock().taps.is_empty() && platform.app_levels().is_empty());
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
