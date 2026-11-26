//! Linux backend: PulseAudio or PipeWire (through pipewire-pulse) driven by `pactl`.
//!
//! Volumes are PulseAudio's normalized scale: 65536 (`PA_VOLUME_NORM`) is 100%, and
//! pactl's percentages are already cubic-tapered. Values above 100% are reported as 1.0.
//!
//! An app's device is where its streams are connected: playback streams (sink inputs)
//! are moved between sinks, recording streams (source outputs) between sources. The
//! sound server remembers the choice per app. Both servers take a move to the default
//! device, by name or as `@DEFAULT_SINK@`, as following the default again.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::{
    AppAudio, AppRoute, AudioCapabilities, AudioDeviceInfo, AudioDevices, AudioError, Backend, Endpoint, Result,
    VolumeState, best_match, find_device,
};

const NORM: f32 = 65536.0;
const MISSING: &str = "pactl not found: install pulseaudio-utils (works with PipeWire via pipewire-pulse)";
const TOO_OLD: &str = "per-app volume and devices need pactl 16 or newer (pulseaudio-utils)";
/// pactl waits forever on a stuck sound server.
const PACTL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an app gets to arrive on the device it is moved to, how often that is
/// checked, and after how long the move is first repeated (see `move_app`). Generous:
/// a busy machine stretches all of it, and only a move that goes wrong waits at all.
const MOVE_TIMEOUT: Duration = Duration::from_secs(5);
const MOVE_POLL: Duration = Duration::from_millis(100);
const MOVE_REPEAT: Duration = Duration::from_millis(500);

pub(crate) struct Platform {
    installed: bool,
    /// The installed pactl turned out to have no JSON output (found on first listing).
    too_old: AtomicBool,
    /// The latest `apps()` listing: where the streams and devices of a found app are.
    latest: Mutex<Listing>,
    /// Held while an app is moved, so moves take their turn (see `set_app_device`).
    moving: Mutex<()>,
}

fn pactl(args: &[&str]) -> Result<String> {
    // C locale: pactl translates its text output.
    let mut command = Command::new("pactl");
    command.args(args).env("LC_ALL", "C");
    let output = omni_core::proc::output_within(&mut command, PACTL_TIMEOUT).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => AudioError::Unsupported(MISSING.into()),
        std::io::ErrorKind::TimedOut => {
            AudioError::Backend("the sound server isn't responding (pactl timed out)".into())
        }
        _ => AudioError::Backend(format!("pactl: {e}")),
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AudioError::Backend(format!("pactl {}: {}", args.join(" "), stderr.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `(kind, name)` for pactl commands like `get-sink-volume @DEFAULT_SINK@`.
fn target(endpoint: Endpoint) -> (&'static str, &'static str) {
    match endpoint {
        Endpoint::Output => ("sink", "@DEFAULT_SINK@"),
        Endpoint::Input => ("source", "@DEFAULT_SOURCE@"),
    }
}

fn raw(volume: f32) -> String {
    ((volume * NORM).round() as u32).to_string()
}

/// Parses `get-sink-volume` output: "Volume: front-left: 32768 /  50% / -18.06 dB,   front-right: ...".
/// Returns the loudest channel.
fn parse_volume(text: &str) -> Option<f32> {
    text.split(',')
        .filter_map(|channel| {
            let (_, value) = channel.rsplit_once(':')?;
            value.split('/').next()?.trim().parse::<u32>().ok()
        })
        .max()
        .map(|v| (v as f32 / NORM).min(1.0))
}

/// Parses `get-sink-mute` output: "Mute: yes".
fn parse_mute(text: &str) -> Option<bool> {
    match text.trim().strip_prefix("Mute:")?.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

type Properties = HashMap<String, serde_json::Value>;

fn property<'a>(properties: &'a Properties, key: &str) -> Option<&'a str> {
    properties.get(key)?.as_str().map(str::trim).filter(|s| !s.is_empty())
}

/// What `pactl -f json list` prints, as far as we read it. One call covers streams and
/// devices; listing each kind on its own would spawn pactl four times.
#[derive(Debug, Default, Deserialize)]
struct Raw {
    #[serde(default)]
    sinks: Vec<Device>,
    #[serde(default)]
    sources: Vec<Device>,
    #[serde(default)]
    sink_inputs: Vec<Stream>,
    #[serde(default)]
    source_outputs: Vec<Stream>,
}

/// A sink or a source.
#[derive(Debug, Deserialize)]
struct Device {
    index: u32,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    properties: Properties,
}

impl Device {
    /// Every sink has a monitor source; they are never a microphone.
    fn is_monitor(&self) -> bool {
        property(&self.properties, "device.class") == Some("monitor") || self.name.ends_with(".monitor")
    }

    fn info(&self) -> AudioDeviceInfo {
        AudioDeviceInfo {
            id: self.name.clone(),
            name: if self.description.trim().is_empty() { self.name.clone() } else { self.description.clone() },
            // Filled in by `Platform::devices`: the listing does not say.
            is_default: false,
            // Hardware comes from a device API (alsa, bluez5); null sinks and remaps do not.
            is_virtual: property(&self.properties, "device.api").is_none(),
        }
    }
}

/// A sink input (playback) or source output (recording).
#[derive(Debug, Deserialize)]
struct Stream {
    index: u32,
    /// Index of the sink (sink inputs) or source (source outputs) it is connected to.
    #[serde(alias = "sink", alias = "source")]
    device: Option<u32>,
    #[serde(default)]
    mute: bool,
    #[serde(default)]
    corked: bool,
    #[serde(default)]
    volume: HashMap<String, ChannelVolume>,
    #[serde(default)]
    properties: Properties,
}

#[derive(Debug, Deserialize)]
struct ChannelVolume {
    value: u32,
}

impl Stream {
    fn property(&self, key: &str) -> Option<&str> {
        property(&self.properties, key)
    }

    /// The app the stream belongs to; none for streams the sound server runs itself
    /// (loopbacks, remapped sources).
    fn app_id(&self) -> Option<&str> {
        self.property("application.process.binary").or_else(|| self.property("application.name"))
    }

    fn volume(&self) -> f32 {
        self.volume.values().map(|c| c.value).max().map_or(1.0, |v| (v as f32 / NORM).min(1.0))
    }
}

/// One app and the indices of its streams.
#[derive(Debug)]
struct App {
    audio: AppAudio,
    playback: Vec<u32>,
    recording: Vec<u32>,
}

impl App {
    /// Where the app plays (records) and the streams doing it.
    fn side(&self, direction: Endpoint) -> (&Option<AppRoute>, &[u32]) {
        match direction {
            Endpoint::Output => (&self.audio.output, &self.playback),
            Endpoint::Input => (&self.audio.input, &self.recording),
        }
    }
}

/// Apps (sorted by name) and the devices they can be sent to.
#[derive(Debug, Default)]
struct Listing {
    apps: Vec<App>,
    devices: AudioDevices,
}

/// The route of an app with one more stream on `device`: that device while all its
/// streams share it. pactl does not say which streams follow the default, so an app
/// is never reported as `AppRoute::Default` here.
fn joined(route: Option<AppRoute>, device: Option<&str>) -> AppRoute {
    match (route, device) {
        (None, Some(device)) => AppRoute::Device { id: device.into() },
        (Some(AppRoute::Device { id }), Some(device)) if id == device => AppRoute::Device { id },
        _ => AppRoute::Several,
    }
}

/// Groups the streams of `pactl -f json list` by binary into apps. Recording from a
/// monitor (desktop capture) is not a microphone choice and is left alone.
fn parse_listing(json: &str) -> Result<Listing> {
    let raw: Raw =
        serde_json::from_str(json).map_err(|e| AudioError::Backend(format!("unexpected pactl output: {e}")))?;
    let mut apps: Vec<App> = Vec::new();
    for stream in &raw.sink_inputs {
        let id = stream.app_id().map_or_else(|| format!("sink-input-{}", stream.index), str::to_string);
        let sink = raw.sinks.iter().find(|d| Some(d.index) == stream.device).map(|d| d.name.as_str());
        if let Some(app) = apps.iter_mut().find(|a| a.audio.id == id) {
            app.playback.push(stream.index);
            app.audio.volume = app.audio.volume.max(stream.volume());
            app.audio.muted &= stream.mute;
            app.audio.playing |= !stream.corked;
            app.audio.output = Some(joined(app.audio.output.take(), sink));
            continue;
        }
        let audio = AppAudio {
            name: stream.property("application.name").unwrap_or(&id).to_string(),
            pid: stream.property("application.process.id").and_then(|p| p.parse().ok()),
            id,
            volume: stream.volume(),
            muted: stream.mute,
            playing: !stream.corked,
            output: Some(joined(None, sink)),
            input: None,
        };
        apps.push(App { audio, playback: vec![stream.index], recording: Vec::new() });
    }
    let microphones: Vec<&Device> = raw.sources.iter().filter(|d| !d.is_monitor()).collect();
    for stream in &raw.source_outputs {
        let Some(id) = stream.app_id() else { continue };
        // A stream between sources (PipeWire, mid-move) has none for a moment: it
        // stays with its app, on no device, so a move is not taken for done.
        let source = raw.sources.iter().find(|d| Some(d.index) == stream.device);
        if source.is_some_and(Device::is_monitor) {
            continue;
        }
        let app = match apps.iter_mut().position(|a| a.audio.id == id) {
            Some(known) => &mut apps[known],
            None => {
                // Only records: listed for its microphone, with no volume to change.
                let audio = AppAudio {
                    name: stream.property("application.name").unwrap_or(id).to_string(),
                    pid: stream.property("application.process.id").and_then(|p| p.parse().ok()),
                    id: id.to_string(),
                    volume: 1.0,
                    muted: false,
                    playing: false,
                    output: None,
                    input: None,
                };
                apps.push(App { audio, playback: Vec::new(), recording: Vec::new() });
                apps.last_mut().expect("just pushed")
            }
        };
        app.recording.push(stream.index);
        app.audio.input = Some(joined(app.audio.input.take(), source.map(|d| d.name.as_str())));
    }
    apps.sort_by_cached_key(|a| a.audio.name.to_lowercase());
    let devices = AudioDevices {
        outputs: raw.sinks.iter().map(Device::info).collect(),
        inputs: microphones.into_iter().map(Device::info).collect(),
    };
    Ok(Listing { apps, devices })
}

/// `pactl -f json info`, as far as we read it.
#[derive(Debug, Default, Deserialize)]
struct ServerInfo {
    #[serde(default)]
    default_sink_name: String,
    #[serde(default)]
    default_source_name: String,
}

impl Platform {
    fn list(&self) -> Result<Listing> {
        let json = pactl(&["-f", "json", "list"]).map_err(|e| match e {
            // JSON output arrived with PulseAudio 15.99 / 16.
            AudioError::Backend(m) if m.contains("unrecognized option") || m.contains("invalid option") => {
                self.too_old.store(true, Relaxed);
                AudioError::Unsupported(TOO_OLD.into())
            }
            e => e,
        })?;
        parse_listing(&json)
    }

    /// Moves the app's playback (recording) streams to `device` and watches them arrive.
    ///
    /// pactl reports a move as done when the sound server took the request. PipeWire
    /// now and then applies it a moment later, or not at all (it drops about one move
    /// in three that follows another of the same stream at once, none 0.3 s later).
    /// So the move is asked again while the app is not there, each time after twice
    /// as long, and fails when it never gets there.
    fn move_app(&self, app: &App, direction: Endpoint, device: &AudioDeviceInfo) -> Result<()> {
        let verb = if direction == Endpoint::Output { "move-sink-input" } else { "move-source-output" };
        let mut streams = app.side(direction).1.to_vec();
        let (deadline, mut patience) = (Instant::now() + MOVE_TIMEOUT, MOVE_REPEAT);
        loop {
            // A stream that just ended fails its move: where the others went is what counts.
            let mut failed = None;
            for index in &streams {
                failed = pactl(&[verb, &index.to_string(), &device.id]).err().or(failed);
            }
            let asked = Instant::now();
            while asked.elapsed() < patience {
                let now = self.list()?.apps;
                let Some(now) = now.into_iter().find(|a| a.audio.id == app.audio.id) else { return Ok(()) };
                let (route, left) = now.side(direction);
                if left.is_empty() || matches!(route, Some(AppRoute::Device { id }) if *id == device.id) {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    let (app, device) = (&app.audio.name, &device.name);
                    let stuck = || AudioError::Backend(format!("the sound server did not move {app} to {device}"));
                    return Err(failed.unwrap_or_else(stuck));
                }
                streams = left.to_vec();
                std::thread::sleep(MOVE_POLL);
            }
            patience *= 2;
        }
    }

    /// Marks the default sink and source, which the listing does not name.
    fn with_defaults(&self, mut devices: AudioDevices) -> Result<AudioDevices> {
        let info = pactl(&["-f", "json", "info"])?;
        let info: ServerInfo = serde_json::from_str(&info).unwrap_or_default();
        for device in &mut devices.outputs {
            device.is_default = device.id == info.default_sink_name;
        }
        for device in &mut devices.inputs {
            device.is_default = device.id == info.default_source_name;
        }
        Ok(devices)
    }

    /// Indices of the app's playback streams, from the listing that found it.
    fn playback_of(&self, app: &AppAudio) -> Result<Vec<u32>> {
        let latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        let found = latest.apps.iter().find(|a| a.audio.id == app.id);
        let streams = found.ok_or_else(|| AudioError::AppNotFound(app.id.clone()))?.playback.clone();
        if streams.is_empty() {
            return Err(AudioError::Backend(format!("{} is not playing anything", app.name)));
        }
        Ok(streams)
    }
}

fn pactl_installed() -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("pactl").is_file()))
}

impl Backend for Platform {
    fn new() -> Self {
        Platform {
            installed: pactl_installed(),
            too_old: AtomicBool::new(false),
            latest: Mutex::default(),
            moving: Mutex::default(),
        }
    }

    fn capabilities(&self) -> AudioCapabilities {
        let note = match (self.installed, self.too_old.load(Relaxed)) {
            (false, _) => Some(MISSING.to_string()),
            (true, true) => Some(TOO_OLD.to_string()),
            (true, false) => None,
        };
        let works = note.is_none();
        AudioCapabilities {
            per_app_volume: works,
            note: note.clone(),
            per_app_output: works,
            output_note: note.clone(),
            per_app_input: works,
            input_note: note,
        }
    }

    fn endpoint(&self, endpoint: Endpoint) -> Result<VolumeState> {
        let (kind, name) = target(endpoint);
        let volume = pactl(&[&format!("get-{kind}-volume"), name])?;
        let mute = pactl(&[&format!("get-{kind}-mute"), name])?;
        let unexpected = |out: &str| AudioError::Backend(format!("unexpected pactl output: {}", out.trim()));
        Ok(VolumeState {
            volume: parse_volume(&volume).ok_or_else(|| unexpected(&volume))?,
            muted: parse_mute(&mute).ok_or_else(|| unexpected(&mute))?,
        })
    }

    fn set_endpoint_volume(&self, endpoint: Endpoint, volume: f32) -> Result<()> {
        let (kind, name) = target(endpoint);
        pactl(&[&format!("set-{kind}-volume"), name, &raw(volume)]).map(drop)
    }

    fn set_endpoint_muted(&self, endpoint: Endpoint, muted: bool) -> Result<()> {
        let (kind, name) = target(endpoint);
        pactl(&[&format!("set-{kind}-mute"), name, if muted { "1" } else { "0" }]).map(drop)
    }

    fn apps(&self) -> Result<Vec<AppAudio>> {
        let listing = self.list()?;
        let apps = listing.apps.iter().map(|a| a.audio.clone()).collect();
        *self.latest.lock().unwrap_or_else(|e| e.into_inner()) = listing;
        Ok(apps)
    }

    fn set_app_volume(&self, app: &AppAudio, volume: f32) -> Result<()> {
        let volume = raw(volume);
        for index in self.playback_of(app)? {
            pactl(&["set-sink-input-volume", &index.to_string(), &volume])?;
        }
        Ok(())
    }

    fn set_app_muted(&self, app: &AppAudio, muted: bool) -> Result<()> {
        for index in self.playback_of(app)? {
            pactl(&["set-sink-input-mute", &index.to_string(), if muted { "1" } else { "0" }])?;
        }
        Ok(())
    }

    fn devices(&self) -> Result<AudioDevices> {
        self.with_defaults(self.list()?.devices)
    }

    fn set_app_device(&self, app: &str, direction: Endpoint, device: Option<&str>) -> Result<()> {
        // One at a time: a move still being watched would undo a newer one of the same app.
        let _turn = self.moving.lock().unwrap_or_else(|e| e.into_inner());
        let Listing { apps, devices } = self.list()?;
        let app = best_match(&apps, app, |a| &a.audio.id, |a| &a.audio.name)
            .ok_or_else(|| AudioError::AppNotFound(app.into()))?;
        let (route, streams) = app.side(direction);
        if streams.is_empty() {
            // Nothing to move, and nowhere to keep the choice for later.
            let idle = if direction == Endpoint::Output { "playing" } else { "recording" };
            return Err(AudioError::Backend(format!("{} is not {idle} anything", app.audio.name)));
        }
        let devices = self.with_defaults(devices)?;
        let devices = match direction {
            Endpoint::Output => &devices.outputs,
            Endpoint::Input => &devices.inputs,
        };
        let Some(query) = device else {
            // The default by name: where the default input is a monitor (no microphone
            // connected), `@DEFAULT_SOURCE@` would put the app on a device it is not
            // listed with, out of reach of the next choice.
            let default = devices.iter().find(|d| d.is_default).ok_or_else(|| {
                AudioError::Backend(format!("the system default is not a device {} can use", app.audio.name))
            })?;
            // Only a move onto the default device makes a stream follow the default
            // again: both sound servers ignore one that changes nothing. So an app
            // that is already there goes by another device, for a moment, to forget
            // an earlier choice of this one. (One spread over several devices is
            // just gathered: what was there already keeps its choice.)
            if matches!(route, Some(AppRoute::Device { id }) if *id == default.id) {
                let other = devices.iter().find(|d| d.id != default.id).ok_or_else(|| {
                    let app = &app.audio.name;
                    AudioError::Backend(format!(
                        "{app} is on the default device already, and making it follow the default again takes a second device"
                    ))
                })?;
                self.move_app(app, direction, other)?;
            }
            return self.move_app(app, direction, default);
        };
        self.move_app(app, direction, find_device(devices, query)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `pactl -f json list sink-inputs` from pipewire-pulse (PipeWire 1.0, Fedora),
    /// trimmed to three streams: two Firefox tabs (one paused) and Spotify.
    const SINK_INPUTS: &str = r#"[{"index":75,"driver":"PipeWire","owner_module":"","client":"74","sink":56,"sample_specification":"float32le 2ch 48000Hz","channel_map":"front-left,front-right","format":"pcm, format.sample_format = \"\\\"float32le\\\"\"  format.rate = \"48000\"  format.channels = \"2\"  format.channel_map = \"\\\"front-left,front-right\\\"\"","corked":false,"mute":false,"volume":{"front-left":{"value":32768,"value_percent":"50%","db":"-18.06 dB"},"front-right":{"value":32768,"value_percent":"50%","db":"-18.06 dB"}},"balance":0.000000,"buffer_latency_usec":0,"sink_latency_usec":0,"resample_method":"PipeWire","properties":{"client.api":"pipewire-pulse","pulse.server.type":"unix","application.name":"Firefox","application.process.id":"4321","application.process.user":"doc","application.process.host":"fedora","application.process.binary":"firefox","application.language":"en_US.UTF-8","window.x11.display":":0","application.process.machine_id":"0f2c5b1e9a8d4c7b","application.icon_name":"firefox","media.name":"AudioStream","node.rate":"1/48000","node.latency":"3600/48000","stream.is-live":"true","node.name":"Firefox","node.want-driver":"true","node.autoconnect":"true","media.class":"Stream/Output/Audio","adapt.follower.spa-node":"","object.register":"false","factory.id":"7","clock.quantum-limit":"8192","factory.mode":"split","audio.adapt.follower":"","library.name":"audioconvert/libspa-audioconvert","client.id":"74","object.id":"77","object.serial":"1203","pulse.attr.maxlength":"4194304","pulse.attr.tlength":"14400","pulse.attr.prebuf":"14404","pulse.attr.minreq":"3600","media.role":"Music","module-stream-restore.id":"sink-input-by-application-name:Firefox"}},{"index":81,"driver":"PipeWire","owner_module":"","client":"74","sink":56,"sample_specification":"float32le 2ch 44100Hz","channel_map":"front-left,front-right","format":"pcm, format.sample_format = \"\\\"float32le\\\"\"  format.rate = \"44100\"  format.channels = \"2\"  format.channel_map = \"\\\"front-left,front-right\\\"\"","corked":true,"mute":true,"volume":{"front-left":{"value":65536,"value_percent":"100%","db":"0.00 dB"},"front-right":{"value":65536,"value_percent":"100%","db":"0.00 dB"}},"balance":0.000000,"buffer_latency_usec":0,"sink_latency_usec":0,"resample_method":"PipeWire","properties":{"client.api":"pipewire-pulse","application.name":"Firefox","application.process.id":"4321","application.process.binary":"firefox","media.name":"AudioStream","media.class":"Stream/Output/Audio","object.id":"91","object.serial":"1270"}},{"index":88,"driver":"PipeWire","owner_module":"","client":"85","sink":56,"sample_specification":"s16le 2ch 44100Hz","channel_map":"front-left,front-right","format":"pcm, format.sample_format = \"\\\"s16le\\\"\"  format.rate = \"44100\"  format.channels = \"2\"  format.channel_map = \"\\\"front-left,front-right\\\"\"","corked":false,"mute":false,"volume":{"front-left":{"value":98304,"value_percent":"150%","db":"10.57 dB"},"front-right":{"value":98304,"value_percent":"150%","db":"10.57 dB"}},"balance":0.000000,"buffer_latency_usec":0,"sink_latency_usec":0,"resample_method":"PipeWire","properties":{"client.api":"pipewire-pulse","application.name":"Spotify","application.process.id":"5555","application.process.binary":"spotify","application.icon_name":"spotify-client","media.name":"Spotify","media.role":"music","media.class":"Stream/Output/Audio","object.id":"95","object.serial":"1301"}}]"#;

    /// The rest of a `pactl -f json list` from the same kind of system, trimmed to what
    /// is read: two sinks, their monitors and two microphones, and four recording
    /// streams (Discord on the webcam, Firefox on the default microphone, OBS capturing
    /// the desktop from a monitor, and the loopback behind a remapped source).
    const DEVICES_AND_RECORDERS: &str = r#""sinks":[{"index":56,"state":"RUNNING","name":"alsa_output.pci-0000_00_1f.3.analog-stereo","description":"Built-in Audio Analog Stereo","monitor_source":"alsa_output.pci-0000_00_1f.3.analog-stereo.monitor","flags":["HARDWARE","HW_MUTE_CTRL","HW_VOLUME_CTRL","DECIBEL_VOLUME","LATENCY"],"properties":{"device.api":"alsa","device.class":"sound","node.name":"alsa_output.pci-0000_00_1f.3.analog-stereo","media.class":"Audio/Sink"}},{"index":59,"state":"SUSPENDED","name":"headphones","description":"","monitor_source":"headphones.monitor","flags":["HARDWARE","DECIBEL_VOLUME","LATENCY"],"properties":{"node.name":"headphones","media.class":"Audio/Sink","factory.name":"support.null-audio-sink"}}],"sources":[{"index":56,"state":"SUSPENDED","name":"alsa_output.pci-0000_00_1f.3.analog-stereo.monitor","description":"Monitor of Built-in Audio Analog Stereo","monitor_source":"alsa_output.pci-0000_00_1f.3.analog-stereo","properties":{"device.api":"alsa","device.class":"monitor"}},{"index":59,"state":"SUSPENDED","name":"headphones.monitor","description":"Monitor of headphones","monitor_source":"headphones","properties":{"device.class":"monitor"}},{"index":57,"state":"RUNNING","name":"alsa_input.pci-0000_00_1f.3.analog-stereo","description":"Built-in Audio Analog Stereo","monitor_source":"","properties":{"device.api":"alsa","device.class":"sound"}},{"index":73,"state":"RUNNING","name":"alsa_input.usb-046d_C922-02.analog-stereo","description":"C922 Pro Stream Webcam Analog Stereo","monitor_source":"","properties":{"device.api":"alsa","device.class":"sound"}}],"source_outputs":[{"index":100,"driver":"PipeWire","owner_module":null,"client":"98","source":73,"corked":false,"mute":false,"properties":{"client.api":"pipewire-pulse","application.name":"WEBRTC VoiceEngine","application.process.id":"7001","application.process.binary":"Discord","media.name":"recStream","media.class":"Stream/Input/Audio"}},{"index":104,"driver":"PipeWire","owner_module":null,"client":"74","source":57,"corked":false,"mute":false,"properties":{"application.name":"Firefox","application.process.id":"4321","application.process.binary":"firefox","media.name":"AudioCallbackDriver","media.class":"Stream/Input/Audio"}},{"index":110,"driver":"PipeWire","owner_module":null,"client":"120","source":56,"corked":false,"mute":false,"properties":{"application.name":"OBS","application.process.id":"8100","application.process.binary":"obs","media.name":"Desktop Audio","media.class":"Stream/Input/Audio"}},{"index":74,"driver":"PipeWire","owner_module":"536870919","client":null,"source":57,"corked":false,"mute":false,"properties":{"node.name":"input.mic_a","media.name":"MicA input","node.virtual":"true","media.class":"Stream/Input/Audio"}}]"#;

    fn listing() -> Listing {
        parse_listing(&format!(r#"{{"modules":[],"sink_inputs":{SINK_INPUTS},{DEVICES_AND_RECORDERS},"clients":[]}}"#))
            .unwrap()
    }

    #[test]
    fn sink_inputs_group_by_binary() {
        let apps = listing().apps;
        let names: Vec<&str> = apps.iter().map(|a| a.audio.name.as_str()).collect();
        assert_eq!(names, ["Firefox", "Spotify", "WEBRTC VoiceEngine"]);

        let firefox = &apps[0];
        let audio = &firefox.audio;
        assert_eq!((audio.id.as_str(), audio.name.as_str(), audio.pid), ("firefox", "Firefox", Some(4321)));
        assert_eq!(firefox.playback, [75, 81]);
        assert_eq!(audio.volume, 1.0); // loudest stream
        assert!(!audio.muted); // only one of two streams is muted
        assert!(audio.playing); // one stream is not corked

        let spotify = &apps[1];
        assert_eq!((spotify.audio.id.as_str(), spotify.audio.volume, spotify.audio.playing), ("spotify", 1.0, true));
        assert_eq!(spotify.playback, [88]);

        assert!(parse_listing("{}").unwrap().apps.is_empty());
        // A recording stream between sources still belongs to its app, on no device.
        let unlinked =
            r#"{"source_outputs":[{"index":7,"source":4294967295,"properties":{"application.name":"Recorder"}}]}"#;
        let apps = parse_listing(unlinked).unwrap().apps;
        assert_eq!((apps[0].recording.as_slice(), &apps[0].audio.input), ([7].as_slice(), &Some(AppRoute::Several)));
        assert!(parse_listing("Connection failure: Connection refused").is_err());
    }

    #[test]
    fn apps_know_their_devices() {
        let Listing { apps, devices } = listing();
        let device = |d: &AudioDeviceInfo| (d.id.clone(), d.name.clone(), d.is_virtual);
        assert_eq!(
            devices.outputs.iter().map(device).collect::<Vec<_>>(),
            [
                ("alsa_output.pci-0000_00_1f.3.analog-stereo".into(), "Built-in Audio Analog Stereo".into(), false),
                ("headphones".into(), "headphones".into(), true), // null sink without a description
            ]
        );
        // Monitors are not microphones.
        assert_eq!(
            devices.inputs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            ["alsa_input.pci-0000_00_1f.3.analog-stereo", "alsa_input.usb-046d_C922-02.analog-stereo"]
        );

        let on = |id: &str| Some(AppRoute::Device { id: id.into() });
        let firefox = &apps[0];
        assert_eq!(firefox.audio.output, on("alsa_output.pci-0000_00_1f.3.analog-stereo")); // both tabs
        assert_eq!(firefox.audio.input, on("alsa_input.pci-0000_00_1f.3.analog-stereo"));
        assert_eq!(firefox.recording, [104]);
        // A third tab on the headphones: the app is on no single device any more.
        assert_eq!(joined(firefox.audio.output.clone(), Some("headphones")), AppRoute::Several);
        assert_eq!(joined(Some(AppRoute::Several), Some("headphones")), AppRoute::Several);

        let spotify = &apps[1];
        assert_eq!((&spotify.audio.input, spotify.recording.len()), (&None, 0));

        // Only records: no playback to move or turn down. OBS (desktop capture from a
        // monitor) and the sound server's own loopback are not listed at all.
        let discord = &apps[2];
        assert_eq!((discord.audio.id.as_str(), discord.audio.pid), ("Discord", Some(7001)));
        assert_eq!((&discord.audio.output, discord.audio.playing), (&None, false));
        assert_eq!(discord.audio.input, on("alsa_input.usb-046d_C922-02.analog-stereo"));
        assert_eq!((discord.playback.len(), discord.recording.as_slice()), (0, [100].as_slice()));
    }

    /// Two null sinks and two microphones (remapped monitors) loaded into the running
    /// sound server, the first of each made the default, and a `pacat` playing and
    /// recording on those. All of it is undone on drop.
    struct Lab {
        modules: Vec<String>,
        streams: Vec<std::process::Child>,
        /// The defaults to put back: (`set-default-sink` / `set-default-source`, name).
        defaults: Vec<(&'static str, String)>,
    }

    impl Lab {
        fn start() -> Self {
            let mut lab = Lab { modules: Vec::new(), streams: Vec::new(), defaults: Vec::new() };
            for name in ["omni_test_a", "omni_test_b"] {
                let sink = format!("sink_name={name}");
                let description = format!("sink_properties=device.description={name}_speaker");
                lab.load(&["module-null-sink", &sink, &description]);
            }
            for name in ["omni_test_mic_a", "omni_test_mic_b"] {
                let source = format!("source_name={name}");
                let description = format!("source_properties=device.description={name}_device");
                lab.load(&["module-remap-source", "master=omni_test_a.monitor", &source, &description]);
            }
            for (get, set, ours) in [
                ("get-default-sink", "set-default-sink", "omni_test_a"),
                ("get-default-source", "set-default-source", "omni_test_mic_a"),
            ] {
                lab.defaults.push((set, pactl(&[get]).unwrap_or_default().trim().to_string()));
                pactl(&[set, ours]).unwrap();
            }
            for args in [["--playback", "--device=omni_test_a"], ["--record", "--device=omni_test_mic_a"]] {
                let (zero, null) =
                    (std::fs::File::open("/dev/zero").unwrap(), std::fs::File::create("/dev/null").unwrap());
                lab.streams
                    .push(Command::new("pacat").args(args).arg("--raw").stdin(zero).stdout(null).spawn().unwrap());
            }
            lab
        }

        fn load(&mut self, args: &[&str]) {
            let module = pactl(&[&["load-module"], args].concat()).unwrap();
            self.modules.push(module.trim().to_string());
        }
    }

    impl Drop for Lab {
        fn drop(&mut self) {
            for stream in &mut self.streams {
                let _ = stream.kill();
                let _ = stream.wait();
            }
            for (set, previous) in &self.defaults {
                let _ = pactl(&[set, previous]);
            }
            for module in self.modules.iter().rev() {
                let _ = pactl(&["unload-module", module]);
            }
        }
    }

    /// Moves an app's playback and recording between devices of a live PulseAudio or
    /// PipeWire server, as that server then reports them. Loads its own devices:
    /// `cargo test -p omni-audio -- --ignored live_server`.
    #[test]
    #[ignore]
    fn live_server_moves_app_streams() {
        let _lab = Lab::start();
        let platform = Platform::new();
        let pacat = || platform.apps().unwrap().into_iter().find(|a| a.id == "pacat");
        let on = |output: &str, input: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let device = |id: &str| Some(AppRoute::Device { id: id.into() });
                let devices = pacat().map(|a| (a.output, a.input));
                if devices == Some((device(output), device(input))) {
                    return;
                }
                assert!(Instant::now() < deadline, "pacat is on {devices:?}, not on {output} / {input}");
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        on("omni_test_a", "omni_test_mic_a");

        platform.set_app_device("pacat", Endpoint::Output, Some("omni_test_b")).unwrap();
        on("omni_test_b", "omni_test_mic_a");
        // By display name, like a hand-written action.
        platform.set_app_device("pacat", Endpoint::Input, Some("omni_test_mic_b_device")).unwrap();
        on("omni_test_b", "omni_test_mic_b");

        let missing = platform.set_app_device("pacat", Endpoint::Output, Some("no such speaker"));
        assert!(matches!(missing, Err(AudioError::DeviceNotFound(_))), "{missing:?}");
        on("omni_test_b", "omni_test_mic_b");

        let devices = platform.devices().unwrap();
        let default = |devices: &[AudioDeviceInfo]| devices.iter().find(|d| d.is_default).map(|d| d.id.clone());
        assert_eq!(default(&devices.outputs).as_deref(), Some("omni_test_a"));
        assert_eq!(default(&devices.inputs).as_deref(), Some("omni_test_mic_a"));
        platform.set_app_device("pacat", Endpoint::Output, None).unwrap();
        platform.set_app_device("pacat", Endpoint::Input, None).unwrap();
        on("omni_test_a", "omni_test_mic_a");

        // An app that is on the default device by choice (it was sent to b, which then
        // became the default) follows the default again after "system default".
        platform.set_app_device("pacat", Endpoint::Output, Some("omni_test_b")).unwrap();
        pactl(&["set-default-sink", "omni_test_b"]).unwrap();
        platform.set_app_device("pacat", Endpoint::Output, None).unwrap();
        pactl(&["set-default-sink", "omni_test_a"]).unwrap();
        on("omni_test_a", "omni_test_mic_a");

        // PipeWire drops the second of two moves of a stream in quick succession about
        // one time in ten (pactl reports both as done): the app must end up on the
        // device all the same.
        let recording = platform.list().unwrap().apps.into_iter().find(|a| a.audio.id == "pacat").unwrap().recording;
        for round in 0..40 {
            for index in &recording {
                pactl(&["move-source-output", &index.to_string(), "omni_test_mic_a"]).unwrap();
            }
            platform.set_app_device("pacat", Endpoint::Input, Some("omni_test_mic_b")).unwrap();
            let moved = Some(AppRoute::Device { id: "omni_test_mic_b".into() });
            assert_eq!(pacat().and_then(|a| a.input), moved, "round {round}");
        }
    }

    #[test]
    fn endpoint_text_output() {
        let stereo = "Volume: front-left: 32768 /  50% / -18.06 dB,   front-right: 26214 /  40% / -23.88 dB\n        balance -0.20\n";
        assert_eq!(parse_volume(stereo), Some(0.5));
        assert_eq!(parse_volume("Volume: mono: 65536 / 100% / 0.00 dB\n"), Some(1.0));
        assert_eq!(parse_volume("Volume: front-left: 98304 / 150% / 10.57 dB"), Some(1.0));
        assert_eq!(parse_volume("Failure: No such entity"), None);
        assert_eq!(parse_mute("Mute: yes\n"), Some(true));
        assert_eq!(parse_mute("Mute: no"), Some(false));
        assert_eq!(raw(0.5), "32768");
    }
}
