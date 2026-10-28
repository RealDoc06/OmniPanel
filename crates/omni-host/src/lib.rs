//! The OmniPanel engine. Owns the config, runs the deck server, pairs and
//! tracks decks, dispatches actions, pushes live button state and keeps the
//! Android USB bridge alive. Has no UI dependency: the Tauri shell (or a test)
//! drives it through [`Host`] and listens to [`HostEvent`]s.

mod api;
mod dispatch;
mod feedback;
mod icons;
mod images;
mod layout;
mod net;
mod pairing;
mod server;
mod sessions;
mod usb;
mod ws;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use omni_actions::Executor;
use omni_ai::AiClient;
use omni_audio::AudioController;
use omni_core::{
    Action, ActionOutcome, AiContext, AppInfo, AppState, AudioCapabilities, AudioDevices, Config, ControlKind,
    DeckSettings, GenerateRequest, GeneratedControl, HaEntity, HostEvent, InstalledApp, LogEntry, LogLevel, MixerState,
    NavTarget, ObsInventory, PairingInfo, Profile, RuntimeState, ServerStatus, Settings, SettingsPatch, Theme,
    VoiceLevels, VoiceOp, VoicePreset, VolumeOp, VolumeTarget, defaults, new_id, now_ms, store,
};
use omni_voice::VoiceEngine;
use tokio::sync::{Notify, broadcast, mpsc};
use tokio::task::JoinHandle;

pub use dispatch::Origin;

pub type Result<T> = anyhow::Result<T>;

pub struct HostOptions {
    pub data_dir: PathBuf,
    pub version: String,
}

/// Cheap to clone handle to the running engine.
#[derive(Clone)]
pub struct Host(Arc<Inner>);

/// Engine internals behind [`Host`]; fields are private to this crate.
pub struct Inner {
    /// The engine's runtime. Spawn through it: methods may be called from
    /// threads without a runtime (the Tauri main thread).
    rt: tokio::runtime::Handle,
    data_dir: PathBuf,
    version: String,
    machine_name: String,
    config: RwLock<Config>,
    /// Keep synchronous commits and their propagation in the same order.
    config_changes: Mutex<()>,
    save_revisions: Mutex<HashMap<(String, String), u64>>,
    events: broadcast::Sender<HostEvent>,
    logs: Mutex<Vec<LogEntry>>,
    sessions: Mutex<sessions::Sessions>,
    next_session: AtomicU64,
    pairing: Mutex<pairing::Pairing>,
    /// Toggle action state per control id.
    toggles: Mutex<HashMap<String, bool>>,
    /// Runs presses of a control with toggles one at a time, see [`Host::run_control`].
    control_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    server: tokio::sync::Mutex<server::ServerControl>,
    server_status: Mutex<ServerStatus>,
    usb: usb::UsbBridge,
    audio: Arc<AudioController>,
    audio_caps: AudioCapabilities,
    voice: VoiceEngine,
    executor: Executor,
    ai: AiClient,
    /// Wakes the feedback loop (something changed, push fresh states).
    feedback: Notify,
    /// Voice settings changed: the voice worker re-applies the latest ones.
    voice_dirty: Notify,
    audio_snapshot: Mutex<feedback::AudioSnapshot>,
    sliders: mpsc::UnboundedSender<(VolumeTarget, f32)>,
    /// An app volume changed: persist `audio.app_levels()` soon.
    app_levels_dirty: Notify,
    ui_active: AtomicBool,
    last_runtime: Mutex<Option<RuntimeState>>,
    /// Background loops, stopped by [`Host::shutdown`]. They hold `Host`
    /// clones, so the engine is only freed once they are gone.
    workers: Mutex<Vec<JoinHandle<()>>>,
    voice_task: Mutex<Option<JoinHandle<()>>>,
    /// Shutdown started: the voice worker turns the voice changer off and exits.
    closing: AtomicBool,
}

impl std::ops::Deref for Host {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl Host {
    /// Load the config and start every subsystem. Call inside a tokio runtime.
    pub async fn start(options: HostOptions) -> Result<Host> {
        let (config, repair_warning) = store::load_with_repair_report(&options.data_dir).context("loading config")?;
        let http = || {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .user_agent(format!("OmniPanel/{}", options.version))
        };
        let ai = AiClient::new(http())?;
        let audio = Arc::new(AudioController::new());
        let executor = Executor::new(audio.clone(), http().build()?);
        executor.configure(&config.settings);
        let (sliders, slider_rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Handle::current();
        let inner = Inner {
            usb: usb::UsbBridge::new(&rt),
            rt,
            data_dir: options.data_dir,
            version: options.version,
            machine_name: machine_name(),
            events: broadcast::channel(64).0,
            logs: Mutex::default(),
            sessions: Mutex::default(),
            next_session: AtomicU64::new(1),
            pairing: Mutex::default(),
            toggles: Mutex::default(),
            control_locks: Mutex::default(),
            server: tokio::sync::Mutex::default(),
            server_status: Mutex::default(),
            audio_caps: audio.capabilities(),
            audio,
            voice: VoiceEngine::new(),
            executor,
            ai,
            feedback: Notify::new(),
            voice_dirty: Notify::new(),
            audio_snapshot: Mutex::default(),
            sliders,
            app_levels_dirty: Notify::new(),
            ui_active: AtomicBool::new(false),
            last_runtime: Mutex::new(None),
            workers: Mutex::default(),
            voice_task: Mutex::default(),
            closing: AtomicBool::new(false),
            config: RwLock::new(config),
            config_changes: Mutex::default(),
            save_revisions: Mutex::default(),
        };
        let host = Host(Arc::new(inner));
        if let Some(warning) = repair_warning {
            host.log(LogLevel::Warn, warning);
        }
        let settings = host.config().settings.clone();
        host.restart_server().await;
        host.usb.configure(&settings.usb, settings.port);
        host.apply_voice();
        let (audio, levels) = (host.audio.clone(), host.config().app_levels.clone());
        tokio::task::spawn_blocking(move || audio.restore_app_levels(&levels));
        *host.voice_task.lock().unwrap() = Some(tokio::spawn(voice_worker(host.clone())));
        *host.workers.lock().unwrap() = vec![
            tokio::spawn(persist_app_levels(host.clone())),
            tokio::spawn(feedback::run(host.clone())),
            tokio::spawn(dispatch::slider_worker(host.clone(), slider_rx)),
            tokio::spawn(runtime_watcher(host.clone())),
        ];
        Ok(host)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HostEvent> {
        self.events.subscribe()
    }

    pub fn config(&self) -> RwLockReadGuard<'_, Config> {
        self.config.read().unwrap()
    }

    pub fn state(&self) -> AppState {
        // Own statement: the read guard must be gone before `runtime()` reads
        // the config again, or a queued writer deadlocks both.
        let config = self.config().clone();
        AppState {
            config,
            logs: self.logs.lock().unwrap().clone(),
            runtime: self.runtime(),
            builtin_themes: defaults::builtin_themes(),
            builtin_voice_presets: defaults::builtin_voice_presets(),
            info: AppInfo {
                version: self.version.clone(),
                os: os_name().into(),
                arch: std::env::consts::ARCH.into(),
                machine_name: self.machine_name.clone(),
                data_dir: self.data_dir.display().to_string(),
            },
        }
    }

    pub fn runtime(&self) -> RuntimeState {
        let mut server = self.server_status.lock().unwrap().clone();
        if server.running {
            server.urls = net::deck_urls(server.port);
        }
        RuntimeState {
            server,
            sessions: self.session_list(),
            pairing: self.pairing.lock().unwrap().current(),
            usb: self.usb.status(),
            obs: self.executor.obs().status(),
            home_assistant: self.executor.home_assistant().status(),
            voice: self.voice.status(),
            audio: self.audio_caps.clone(),
        }
    }

    /// The desktop window is open: keep runtime status fresh for it.
    pub fn set_ui_active(&self, active: bool) {
        self.ui_active.store(active, Ordering::Relaxed);
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Name shown on decks.
    pub fn host_name(&self) -> String {
        let name = self.config().settings.host_name.trim().to_string();
        if name.is_empty() { self.machine_name.clone() } else { name }
    }

    // ----- config mutations -------------------------------------------------

    pub fn set_settings(&self, settings: Settings) -> Result<()> {
        self.mutate(|c| {
            c.settings = settings;
            Ok(())
        })
    }

    /// Merge fields while holding the same lock used by every config mutation.
    pub fn patch_settings(&self, patch: SettingsPatch) -> Result<()> {
        self.mutate(|c| {
            patch.apply(&mut c.settings);
            Ok(())
        })
    }

    pub fn save_profile(&self, profile: Profile) -> Result<()> {
        self.mutate(|c| {
            match c.profiles.iter_mut().find(|p| p.id == profile.id) {
                Some(existing) => *existing = profile,
                None => c.profiles.push(profile),
            }
            Ok(())
        })
    }

    pub fn delete_profile(&self, id: &str) -> Result<()> {
        self.mutate(|c| {
            if c.profiles.len() <= 1 {
                bail!("can't delete the last profile");
            }
            c.profiles.retain(|p| p.id != id);
            for device in c.devices.iter_mut().filter(|d| d.profile_id.as_deref() == Some(id)) {
                device.profile_id = None;
            }
            Ok(())
        })
    }

    pub fn export_profile(&self, id: &str, path: &Path) -> Result<()> {
        let profile = self.config().profile(id).cloned().ok_or_else(|| anyhow!("profile not found"))?;
        std::fs::write(path, serde_json::to_vec_pretty(&profile)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    /// Import a profile file with fresh ids, so importing twice never collides.
    /// Navigation to its own pages or itself follows the new ids.
    pub fn import_profile(&self, path: &Path) -> Result<Profile> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let mut profile: Profile = serde_json::from_slice(&bytes).context("not an OmniPanel profile")?;
        let new_profile_id = new_id();
        let profiles = HashMap::from([(std::mem::replace(&mut profile.id, new_profile_id.clone()), new_profile_id)]);
        let mut pages = HashMap::new();
        for page in &mut profile.pages {
            let new = new_id();
            pages.insert(std::mem::replace(&mut page.id, new.clone()), new);
        }
        for control in profile.pages.iter_mut().flat_map(|p| &mut p.controls) {
            control.id = new_id();
            if let ControlKind::Button { action: Some(action) } = &mut control.kind {
                follow_renamed(action, &profiles, &pages);
            }
        }
        if self.config().profiles.iter().any(|p| p.name == profile.name) {
            profile.name = format!("{} (imported)", profile.name);
        }
        profile.normalize();
        self.save_profile(profile.clone())?;
        Ok(profile)
    }

    pub fn update_device(&self, id: &str, name: String, profile_id: Option<String>, deck: DeckSettings) -> Result<()> {
        self.mutate(|c| {
            let device = c.devices.iter_mut().find(|d| d.id == id).ok_or_else(|| anyhow!("device not found"))?;
            device.name = clean_name(&name).unwrap_or_else(|| device.name.clone());
            device.profile_id = profile_id;
            device.deck = deck;
            Ok(())
        })
    }

    /// Forget a device. Its open decks are told to drop their token.
    pub fn unpair_device(&self, id: &str) -> Result<()> {
        self.mutate(|c| {
            c.devices.retain(|d| d.id != id);
            Ok(())
        })
    }

    pub fn save_theme(&self, theme: Theme) -> Result<()> {
        if defaults::builtin_themes().iter().any(|t| t.id == theme.id) {
            bail!("built-in themes can't be changed, duplicate it first");
        }
        self.mutate(|c| {
            match c.themes.iter_mut().find(|t| t.id == theme.id) {
                Some(existing) => *existing = theme,
                None => c.themes.push(theme),
            }
            Ok(())
        })
    }

    pub fn delete_theme(&self, id: &str) -> Result<()> {
        self.mutate(|c| {
            c.themes.retain(|t| t.id != id);
            if c.settings.theme_id == id {
                c.settings.theme_id = defaults::DEFAULT_THEME_ID.into();
            }
            for profile in c.profiles.iter_mut().filter(|p| p.theme_id.as_deref() == Some(id)) {
                profile.theme_id = None;
            }
            Ok(())
        })
    }

    pub fn save_voice_preset(&self, preset: VoicePreset) -> Result<()> {
        if defaults::builtin_voice_presets().iter().any(|p| p.id == preset.id) {
            bail!("built-in presets can't be changed, duplicate it first");
        }
        self.mutate(|c| {
            match c.voice_presets.iter_mut().find(|p| p.id == preset.id) {
                Some(existing) => *existing = preset,
                None => c.voice_presets.push(preset),
            }
            Ok(())
        })
    }

    pub fn delete_voice_preset(&self, id: &str) -> Result<()> {
        self.mutate(|c| {
            c.voice_presets.retain(|p| p.id != id);
            if c.settings.voice.preset_id == id {
                c.settings.voice.preset_id = "clean".into();
            }
            Ok(())
        })
    }

    /// Apply `f` to a copy of the config and persist it; only then does the
    /// copy become the live config and the change propagate (UI event, deck
    /// layouts, server port, USB, integrations, voice). On any error the live
    /// config is untouched.
    pub(crate) fn mutate<R>(&self, f: impl FnOnce(&mut Config) -> Result<R>) -> Result<R> {
        let _change = self.config_changes.lock().unwrap();
        let (result, before, after) = {
            let mut config = self.config.write().unwrap();
            let mut next = config.clone();
            let result = f(&mut next)?;
            next.normalize();
            if next == *config {
                return Ok(result);
            }
            store::save(&self.data_dir, &next)?;
            let before = std::mem::replace(&mut *config, next);
            (result, before, config.clone())
        };
        self.after_config_change(&before, &after);
        Ok(result)
    }

    fn after_config_change(&self, before: &Config, after: &Config) {
        let _ = self.events.send(HostEvent::Config { config: after.clone() });
        let (old, new) = (&before.settings, &after.settings);
        if old.port != new.port {
            let host = self.clone();
            self.rt.spawn(async move { host.restart_server().await });
        }
        if old.usb != new.usb || old.port != new.port {
            self.usb.configure(&new.usb, new.port);
        }
        if old.obs != new.obs || old.home_assistant != new.home_assistant {
            self.executor.configure(new);
        }
        let voice_preset_changed = before.voice_presets != after.voice_presets;
        if old.voice != new.voice || voice_preset_changed {
            self.apply_voice();
        }
        self.refresh_sessions();
        self.emit_runtime();
        // A deck may now show a control that needs audio polling or fresh state.
        self.feedback.notify_one();
    }

    // ----- pairing ----------------------------------------------------------

    pub fn start_pairing(&self) -> PairingInfo {
        let info = self.pairing.lock().unwrap().start();
        self.emit_runtime();
        info
    }

    pub fn stop_pairing(&self) {
        self.pairing.lock().unwrap().stop();
        self.emit_runtime();
    }

    // ----- actions ----------------------------------------------------------

    /// Run an action from the desktop UI ("Test" button).
    pub async fn run_action(&self, action: Action) -> Result<ActionOutcome> {
        let outcome = self.execute(Origin::Ui, "", &action).await;
        self.feedback.notify_one();
        outcome
    }

    pub async fn installed_apps(&self) -> Vec<InstalledApp> {
        tokio::task::spawn_blocking(omni_actions::apps::installed).await.unwrap_or_default()
    }

    pub async fn import_image(&self, path: PathBuf) -> Result<String> {
        let dir = self.data_dir.clone();
        tokio::task::spawn_blocking(move || images::import(&dir, &path)).await?
    }

    // ----- audio ------------------------------------------------------------

    pub async fn mixer(&self) -> MixerState {
        let audio = self.audio.clone();
        tokio::task::spawn_blocking(move || audio.mixer()).await.unwrap_or_default()
    }

    pub async fn set_volume(&self, target: VolumeTarget, value: f32) -> Result<()> {
        self.run_leaf(&Action::Volume { target, op: VolumeOp::Set { value } }).await?;
        self.feedback.notify_one();
        Ok(())
    }

    pub async fn set_muted(&self, target: VolumeTarget, muted: bool) -> Result<()> {
        let op = if muted { VolumeOp::Mute } else { VolumeOp::Unmute };
        self.run_leaf(&Action::Volume { target, op }).await?;
        self.feedback.notify_one();
        Ok(())
    }

    /// Run a leaf action; persist app volume changes even if applying them fails.
    pub(crate) async fn run_leaf(&self, action: &Action) -> Result<ActionOutcome> {
        let outcome = self.executor.run(action).await;
        if let Action::Volume { target: VolumeTarget::App { .. }, .. } = action {
            self.app_levels_dirty.notify_one();
        }
        Ok(outcome?)
    }

    // ----- voice ------------------------------------------------------------

    pub async fn voice_devices(&self) -> Result<AudioDevices> {
        let host = self.clone();
        Ok(tokio::task::spawn_blocking(move || host.voice.devices()).await??)
    }

    pub fn voice_levels(&self) -> VoiceLevels {
        self.voice.levels()
    }

    pub async fn create_virtual_mic(&self) -> Result<String> {
        let host = self.clone();
        Ok(tokio::task::spawn_blocking(move || host.voice.create_virtual_mic()).await??)
    }

    /// All voice presets, built-in first.
    pub fn voice_presets(&self) -> Vec<VoicePreset> {
        let mut presets = defaults::builtin_voice_presets();
        presets.extend(self.config().voice_presets.iter().cloned());
        presets
    }

    /// Push the current voice settings to the engine, see [`voice_worker`].
    fn apply_voice(&self) {
        self.voice_dirty.notify_one();
    }

    pub(crate) fn voice_op(&self, op: &VoiceOp) -> Result<ActionOutcome> {
        let presets = self.voice_presets();
        let name_of = |id: &str| presets.iter().find(|p| p.id == id).map(|p| p.name.clone());
        let mut message = None;
        self.mutate(|c| {
            let voice = &mut c.settings.voice;
            match op {
                VoiceOp::Toggle => voice.enabled = !voice.enabled,
                VoiceOp::Enable => voice.enabled = true,
                VoiceOp::Disable => voice.enabled = false,
                VoiceOp::SetPreset { preset_id } => {
                    if name_of(preset_id).is_none() {
                        bail!("voice preset \"{preset_id}\" not found");
                    }
                    voice.preset_id = preset_id.clone();
                    voice.enabled = true;
                }
                VoiceOp::Cycle { preset_ids } => {
                    let ids: Vec<&str> = if preset_ids.is_empty() {
                        presets.iter().map(|p| p.id.as_str()).collect()
                    } else {
                        preset_ids.iter().map(String::as_str).filter(|id| name_of(id).is_some()).collect()
                    };
                    let current = ids.iter().position(|id| *id == voice.preset_id);
                    let next = ids.get(current.map_or(0, |i| (i + 1) % ids.len())).copied();
                    let next = next.ok_or_else(|| anyhow!("no voice presets to cycle"))?;
                    voice.preset_id = next.to_string();
                    voice.enabled = true;
                }
            }
            if matches!(op, VoiceOp::SetPreset { .. } | VoiceOp::Cycle { .. }) {
                message = name_of(&voice.preset_id);
            }
            Ok(())
        })?;
        Ok(ActionOutcome { message })
    }

    // ----- integrations & AI --------------------------------------------------

    pub async fn obs_inventory(&self) -> Result<ObsInventory> {
        Ok(self.executor.obs().inventory().await?)
    }

    pub async fn ha_entities(&self) -> Result<Vec<HaEntity>> {
        Ok(self.executor.home_assistant().entities().await?)
    }

    pub async fn ai_generate(&self, request: GenerateRequest) -> Result<GeneratedControl> {
        let settings = self.config().settings.clone();
        let inventory = self.executor.obs().inventory().await.unwrap_or_default();
        let audio_apps = self.mixer().await.apps.into_iter().map(|a| a.name).collect();
        let voice_presets = self.voice_presets().into_iter().map(|p| format!("{} ({})", p.id, p.name)).collect();
        let context = AiContext {
            os: os_name().into(),
            obs_scenes: inventory.scenes,
            obs_inputs: inventory.inputs,
            audio_apps,
            voice_presets,
            home_assistant: settings.home_assistant.enabled,
        };
        Ok(self.ai.generate_control(&settings.ai, &context, &request).await?)
    }

    pub async fn ai_models(&self) -> Result<Vec<String>> {
        let settings = self.config().settings.ai.clone();
        Ok(self.ai.list_models(&settings).await?)
    }

    pub async fn ai_test(&self) -> Result<String> {
        let settings = self.config().settings.ai.clone();
        Ok(self.ai.test(&settings).await?)
    }

    // ----- events -----------------------------------------------------------

    /// Log to tracing and to the desktop UI log view.
    pub fn log(&self, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        match level {
            LogLevel::Info => tracing::info!("{message}"),
            LogLevel::Warn => tracing::warn!("{message}"),
            LogLevel::Error => tracing::error!("{message}"),
        }
        let entry = LogEntry { at: now_ms(), level, message };
        let mut logs = self.logs.lock().unwrap();
        logs.insert(0, entry.clone());
        logs.truncate(300);
        drop(logs);
        let _ = self.events.send(HostEvent::Log { entry });
    }

    /// Emit a `runtime` event if anything changed since the last one.
    pub(crate) fn emit_runtime(&self) {
        let runtime = self.runtime();
        let mut last = self.last_runtime.lock().unwrap();
        if last.as_ref() != Some(&runtime) {
            *last = Some(runtime.clone());
            let _ = self.events.send(HostEvent::Runtime { runtime });
        }
    }

    /// Close every deck connection, stop the server and the USB bridge for
    /// good, turn the voice changer off and stop the background loops, so
    /// the engine is freed once the last `Host` is dropped.
    pub async fn shutdown(&self) {
        self.close_all_sessions();
        let voice_off = async {
            self.closing.store(true, Ordering::Relaxed);
            self.voice_dirty.notify_one();
            let voice = self.voice_task.lock().unwrap().take();
            if let Some(voice) = voice {
                let _ = voice.await;
            }
        };
        tokio::join!(self.stop_server(), self.usb.stop(), voice_off);
        let workers = std::mem::take(&mut *self.workers.lock().unwrap());
        for worker in workers {
            worker.abort();
            let _ = worker.await;
        }
    }
}

/// Applies voice settings one at a time and always the latest ones, so fast
/// toggles can't finish out of order. Opening devices blocks, hence the
/// blocking pool. Once [`Host::shutdown`] started it turns the voice changer
/// off (after any apply in flight) and exits.
async fn voice_worker(host: Host) {
    loop {
        host.voice_dirty.notified().await;
        let closing = host.closing.load(Ordering::Relaxed);
        let mut settings = host.config().settings.voice.clone();
        settings.enabled &= !closing;
        let presets = host.voice_presets();
        let preset = presets.iter().find(|p| p.id == settings.preset_id).unwrap_or(&presets[0]).clone();
        let engine = host.clone();
        let result = tokio::task::spawn_blocking(move || engine.voice.apply(&settings, &preset)).await;
        match result {
            Ok(Err(e)) => host.log(LogLevel::Error, format!("Voice changer: {e}")),
            Err(e) => host.log(LogLevel::Error, format!("Voice changer stopped: {e}")),
            Ok(Ok(())) => {}
        }
        if closing {
            return;
        }
        host.emit_runtime();
        host.feedback.notify_one();
    }
}

/// Saves per-app levels to the config a moment after they stop changing
/// (sliders send bursts), so they can be restored on the next start.
async fn persist_app_levels(host: Host) {
    loop {
        host.app_levels_dirty.notified().await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let audio = host.audio.clone();
        let Ok(levels) = tokio::task::spawn_blocking(move || audio.app_levels()).await else { continue };
        if host.config().app_levels != levels {
            let _ = host.mutate(|c| {
                c.app_levels = levels;
                Ok(())
            });
        }
    }
}

/// Re-emits runtime status right away when USB, OBS or Home Assistant
/// change, and every 2 s while the window is open (voice engine status has
/// no notifications).
async fn runtime_watcher(host: Host) {
    let mut usb = host.usb.subscribe();
    let mut obs = host.executor.obs().subscribe();
    let mut ha = host.executor.home_assistant().subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            changed = usb.changed() => if changed.is_err() { return },
            changed = obs.changed() => if changed.is_err() { return },
            changed = ha.changed() => if changed.is_err() { return },
            _ = tick.tick() => if !host.ui_active.load(Ordering::Relaxed) { continue },
        }
        host.emit_runtime();
    }
}

/// Remap imported navigation within its own profile or page namespace.
/// Targets outside the maps (other profiles) stay as they are.
fn follow_renamed(action: &mut Action, profiles: &HashMap<String, String>, pages: &HashMap<String, String>) {
    match action {
        Action::Navigate { to } => {
            let (id, renamed) = match to {
                NavTarget::Page { page_id } => (page_id, pages),
                NavTarget::Profile { profile_id } => (profile_id, profiles),
                _ => return,
            };
            if let Some(new) = renamed.get(id) {
                id.clone_from(new);
            }
        }
        Action::Multi { steps, .. } => steps.iter_mut().for_each(|step| follow_renamed(step, profiles, pages)),
        Action::Toggle { on, off } => {
            follow_renamed(on, profiles, pages);
            follow_renamed(off, profiles, pages);
        }
        _ => {}
    }
}

fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(40).collect();
    (!name.is_empty()).then_some(name)
}

/// Friendly computer name: macOS "Computer Name" ("Doc's Mac mini"),
/// elsewhere the hostname without its DNS domain.
fn machine_name() -> String {
    #[cfg(target_os = "macos")]
    if let Some(name) = net::scutil("ComputerName") {
        return name;
    }
    let host = gethostname::gethostname().to_string_lossy().to_string();
    let name = host.split('.').next().unwrap_or_default().trim();
    if name.is_empty() { "OmniPanel".into() } else { name.into() }
}

pub fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A host on a free port with a throwaway data dir, USB off.
    pub(crate) async fn start_host() -> Host {
        let data_dir = std::env::temp_dir().join(format!("omni-host-unit-{}", new_id()));
        let mut config = Config::default();
        config.settings.port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        config.settings.usb.enabled = false;
        store::save(&data_dir, &config).unwrap();
        Host::start(HostOptions { data_dir, version: "test".into() }).await.unwrap()
    }

    #[tokio::test]
    async fn config_commit_waits_for_previous_propagation() {
        let host = start_host().await;
        let engine = host.clone();
        tokio::task::spawn_blocking(move || {
            // Stall A while it propagates, after committing and emitting its config.
            let runtime = engine.last_runtime.lock().unwrap();
            let mut events = engine.subscribe();
            std::thread::scope(|scope| {
                let first = scope.spawn(|| {
                    engine.mutate(|c| {
                        c.settings.host_name = "A".into();
                        Ok(())
                    })
                });
                while !matches!(events.blocking_recv().unwrap(), HostEvent::Config { .. }) {}
                let (started, starting) = std::sync::mpsc::channel();
                let (committing, commit) = std::sync::mpsc::channel();
                let second_engine = &engine;
                let second = scope.spawn(move || {
                    started.send(()).unwrap();
                    second_engine.mutate(|c| {
                        committing.send(()).unwrap();
                        c.settings.host_name = "B".into();
                        Ok(())
                    })
                });
                starting.recv_timeout(Duration::from_secs(1)).unwrap();
                let blocked = commit.recv_timeout(Duration::from_millis(100)).is_err();
                drop(runtime);
                first.join().unwrap().unwrap();
                second.join().unwrap().unwrap();
                assert!(blocked, "B committed while A was still propagating");
            });
            assert_eq!(engine.config().settings.host_name, "B");
        })
        .await
        .unwrap();
        host.shutdown().await;
        std::fs::remove_dir_all(host.data_dir()).unwrap();
    }

    #[tokio::test]
    async fn import_keeps_profile_and_page_navigation_separate() {
        let host = start_host().await;
        let profile_nav = |id: &str| Action::Navigate { to: NavTarget::Profile { profile_id: id.into() } };
        let page_nav = |id: &str| Action::Navigate { to: NavTarget::Page { page_id: id.into() } };
        let mut profile = defaults::default_profile();
        profile.id = "main".into();
        profile.pages.truncate(1);
        let page = &mut profile.pages[0];
        page.id = "main".into();
        page.controls.truncate(1);
        page.controls[0].kind = ControlKind::Button {
            action: Some(Action::Toggle {
                on: Box::new(profile_nav("main")),
                off: Box::new(Action::Multi {
                    steps: vec![page_nav("main"), profile_nav("external"), profile_nav("page-only")],
                    delay_ms: 0,
                }),
            }),
        };
        profile.pages.push(omni_core::Page { id: "page-only".into(), name: "Other".into(), controls: vec![] });
        let path = host.data_dir().join("import.json");
        std::fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();

        let imported = host.import_profile(&path).unwrap();
        assert_ne!(imported.id, imported.pages[0].id);
        assert_ne!(imported.id, "main");
        assert_ne!(imported.pages[0].id, "main");
        assert_eq!(
            imported.pages[0].controls[0].kind,
            ControlKind::Button {
                action: Some(Action::Toggle {
                    on: Box::new(profile_nav(&imported.id)),
                    off: Box::new(Action::Multi {
                        steps: vec![page_nav(&imported.pages[0].id), profile_nav("external"), profile_nav("page-only")],
                        delay_ms: 0,
                    }),
                })
            }
        );
        host.shutdown().await;
        std::fs::remove_dir_all(host.data_dir()).unwrap();
    }

    #[tokio::test]
    async fn shutdown_stops_for_good_and_frees_the_engine() {
        let host = start_host().await;
        let mut settings = host.config().settings.clone();
        settings.port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        // Queues a server restart that only runs once shutdown yields.
        host.set_settings(settings).unwrap();
        host.shutdown().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!host.runtime().server.running);
        // Every background loop is gone: this handle is the last one.
        assert_eq!(Arc::strong_count(&host.0), 1);
    }
}
