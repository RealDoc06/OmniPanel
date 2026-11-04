//! Linux backend: PulseAudio or PipeWire (through pipewire-pulse) driven by `pactl`.
//!
//! Volumes are PulseAudio's normalized scale: 65536 (`PA_VOLUME_NORM`) is 100%, and
//! pactl's percentages are already cubic-tapered. Values above 100% are reported as 1.0.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

use crate::{AppAudio, AudioCapabilities, AudioError, Backend, Endpoint, Result, VolumeState};

const NORM: f32 = 65536.0;
const MISSING: &str = "pactl not found: install pulseaudio-utils (works with PipeWire via pipewire-pulse)";
/// pactl waits forever on a stuck sound server.
const PACTL_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct Platform {
    installed: bool,
    /// Sink-input indices per app id from the latest `apps()` listing.
    streams: Mutex<HashMap<String, Vec<u32>>>,
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

#[derive(Debug, Deserialize)]
struct SinkInput {
    index: u32,
    #[serde(default)]
    mute: bool,
    #[serde(default)]
    corked: bool,
    #[serde(default)]
    volume: HashMap<String, ChannelVolume>,
    #[serde(default)]
    properties: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ChannelVolume {
    value: u32,
}

impl SinkInput {
    fn property(&self, key: &str) -> Option<&str> {
        self.properties.get(key)?.as_str().map(str::trim).filter(|s| !s.is_empty())
    }

    fn volume(&self) -> f32 {
        self.volume.values().map(|c| c.value).max().map_or(1.0, |v| (v as f32 / NORM).min(1.0))
    }
}

/// Groups `pactl -f json list sink-inputs` by binary into apps (sorted by name),
/// with the sink-input indices of each.
fn parse_apps(json: &str) -> Result<Vec<(AppAudio, Vec<u32>)>> {
    let inputs: Vec<SinkInput> =
        serde_json::from_str(json).map_err(|e| AudioError::Backend(format!("unexpected pactl output: {e}")))?;
    let mut apps: Vec<(AppAudio, Vec<u32>)> = Vec::new();
    for input in &inputs {
        let id = input
            .property("application.process.binary")
            .or_else(|| input.property("application.name"))
            .map_or_else(|| format!("sink-input-{}", input.index), str::to_string);
        if let Some((app, indices)) = apps.iter_mut().find(|(a, _)| a.id == id) {
            indices.push(input.index);
            app.volume = app.volume.max(input.volume());
            app.muted &= input.mute;
            app.playing |= !input.corked;
            continue;
        }
        let app = AppAudio {
            name: input.property("application.name").unwrap_or(&id).to_string(),
            pid: input.property("application.process.id").and_then(|p| p.parse().ok()),
            id,
            volume: input.volume(),
            muted: input.mute,
            playing: !input.corked,
        };
        apps.push((app, vec![input.index]));
    }
    apps.sort_by_cached_key(|(a, _)| a.name.to_lowercase());
    Ok(apps)
}

impl Platform {
    fn streams_of(&self, app: &AppAudio) -> Result<Vec<u32>> {
        let streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        streams.get(&app.id).cloned().ok_or_else(|| AudioError::AppNotFound(app.id.clone()))
    }
}

fn pactl_installed() -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("pactl").is_file()))
}

impl Backend for Platform {
    fn new() -> Self {
        Platform { installed: pactl_installed(), streams: Mutex::default() }
    }

    fn capabilities(&self) -> AudioCapabilities {
        AudioCapabilities { per_app_volume: self.installed, note: (!self.installed).then(|| MISSING.into()) }
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
        let json = pactl(&["-f", "json", "list", "sink-inputs"]).map_err(|e| match e {
            // JSON output arrived with PulseAudio 15.99 / 16.
            AudioError::Backend(m) if m.contains("unrecognized option") || m.contains("invalid option") => {
                AudioError::Unsupported("per-app volume needs pactl 16 or newer (pulseaudio-utils)".into())
            }
            e => e,
        })?;
        let apps = parse_apps(&json)?;
        let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        *streams = apps.iter().map(|(a, indices)| (a.id.clone(), indices.clone())).collect();
        Ok(apps.into_iter().map(|(app, _)| app).collect())
    }

    fn set_app_volume(&self, app: &AppAudio, volume: f32) -> Result<()> {
        let volume = raw(volume);
        for index in self.streams_of(app)? {
            pactl(&["set-sink-input-volume", &index.to_string(), &volume])?;
        }
        Ok(())
    }

    fn set_app_muted(&self, app: &AppAudio, muted: bool) -> Result<()> {
        for index in self.streams_of(app)? {
            pactl(&["set-sink-input-mute", &index.to_string(), if muted { "1" } else { "0" }])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `pactl -f json list sink-inputs` from pipewire-pulse (PipeWire 1.0, Fedora),
    /// trimmed to three streams: two Firefox tabs (one paused) and Spotify.
    const SINK_INPUTS: &str = r#"[{"index":75,"driver":"PipeWire","owner_module":"","client":"74","sink":56,"sample_specification":"float32le 2ch 48000Hz","channel_map":"front-left,front-right","format":"pcm, format.sample_format = \"\\\"float32le\\\"\"  format.rate = \"48000\"  format.channels = \"2\"  format.channel_map = \"\\\"front-left,front-right\\\"\"","corked":false,"mute":false,"volume":{"front-left":{"value":32768,"value_percent":"50%","db":"-18.06 dB"},"front-right":{"value":32768,"value_percent":"50%","db":"-18.06 dB"}},"balance":0.000000,"buffer_latency_usec":0,"sink_latency_usec":0,"resample_method":"PipeWire","properties":{"client.api":"pipewire-pulse","pulse.server.type":"unix","application.name":"Firefox","application.process.id":"4321","application.process.user":"doc","application.process.host":"fedora","application.process.binary":"firefox","application.language":"en_US.UTF-8","window.x11.display":":0","application.process.machine_id":"0f2c5b1e9a8d4c7b","application.icon_name":"firefox","media.name":"AudioStream","node.rate":"1/48000","node.latency":"3600/48000","stream.is-live":"true","node.name":"Firefox","node.want-driver":"true","node.autoconnect":"true","media.class":"Stream/Output/Audio","adapt.follower.spa-node":"","object.register":"false","factory.id":"7","clock.quantum-limit":"8192","factory.mode":"split","audio.adapt.follower":"","library.name":"audioconvert/libspa-audioconvert","client.id":"74","object.id":"77","object.serial":"1203","pulse.attr.maxlength":"4194304","pulse.attr.tlength":"14400","pulse.attr.prebuf":"14404","pulse.attr.minreq":"3600","media.role":"Music","module-stream-restore.id":"sink-input-by-application-name:Firefox"}},{"index":81,"driver":"PipeWire","owner_module":"","client":"74","sink":56,"sample_specification":"float32le 2ch 44100Hz","channel_map":"front-left,front-right","format":"pcm, format.sample_format = \"\\\"float32le\\\"\"  format.rate = \"44100\"  format.channels = \"2\"  format.channel_map = \"\\\"front-left,front-right\\\"\"","corked":true,"mute":true,"volume":{"front-left":{"value":65536,"value_percent":"100%","db":"0.00 dB"},"front-right":{"value":65536,"value_percent":"100%","db":"0.00 dB"}},"balance":0.000000,"buffer_latency_usec":0,"sink_latency_usec":0,"resample_method":"PipeWire","properties":{"client.api":"pipewire-pulse","application.name":"Firefox","application.process.id":"4321","application.process.binary":"firefox","media.name":"AudioStream","media.class":"Stream/Output/Audio","object.id":"91","object.serial":"1270"}},{"index":88,"driver":"PipeWire","owner_module":"","client":"85","sink":56,"sample_specification":"s16le 2ch 44100Hz","channel_map":"front-left,front-right","format":"pcm, format.sample_format = \"\\\"s16le\\\"\"  format.rate = \"44100\"  format.channels = \"2\"  format.channel_map = \"\\\"front-left,front-right\\\"\"","corked":false,"mute":false,"volume":{"front-left":{"value":98304,"value_percent":"150%","db":"10.57 dB"},"front-right":{"value":98304,"value_percent":"150%","db":"10.57 dB"}},"balance":0.000000,"buffer_latency_usec":0,"sink_latency_usec":0,"resample_method":"PipeWire","properties":{"client.api":"pipewire-pulse","application.name":"Spotify","application.process.id":"5555","application.process.binary":"spotify","application.icon_name":"spotify-client","media.name":"Spotify","media.role":"music","media.class":"Stream/Output/Audio","object.id":"95","object.serial":"1301"}}]"#;

    #[test]
    fn sink_inputs_group_by_binary() {
        let apps = parse_apps(SINK_INPUTS).unwrap();
        assert_eq!(apps.len(), 2);

        let (firefox, streams) = &apps[0];
        assert_eq!((firefox.id.as_str(), firefox.name.as_str(), firefox.pid), ("firefox", "Firefox", Some(4321)));
        assert_eq!(streams, &[75, 81]);
        assert_eq!(firefox.volume, 1.0); // loudest stream
        assert!(!firefox.muted); // only one of two streams is muted
        assert!(firefox.playing); // one stream is not corked

        let (spotify, streams) = &apps[1];
        assert_eq!((spotify.id.as_str(), spotify.volume, spotify.playing), ("spotify", 1.0, true));
        assert_eq!(streams, &[88]);

        assert!(parse_apps("[]").unwrap().is_empty());
        assert!(parse_apps("Connection failure: Connection refused").is_err());
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
