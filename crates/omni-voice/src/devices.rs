//! Device enumeration and lookup, virtual-cable detection, install hints and the
//! Linux virtual microphone.
//!
//! Device ids are cpal's stable [`cpal::DeviceId`] strings (`"coreaudio:<uid>"`,
//! `"wasapi:<endpoint id>"`, `"pulseaudio:<sink name>"`). Lookups also accept a
//! plain device name, so hand-edited configs keep working.

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{DeviceType, InterfaceType};

use crate::{AudioDeviceInfo, AudioDevices, Result, VoiceError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Input,
    Output,
}

impl Direction {
    fn supports(self, device: &cpal::Device) -> bool {
        match self {
            Self::Input => device.supports_input(),
            Self::Output => device.supports_output(),
        }
    }

    /// Retried once: enumerating can fail when the device list changes mid-query.
    pub(crate) fn list(self, host: &cpal::Host) -> Result<Vec<cpal::Device>> {
        let list = || match self {
            Self::Input => host.input_devices().map(|d| d.collect()),
            Self::Output => host.output_devices().map(|d| d.collect()),
        };
        list().or_else(|_| list()).map_err(backend)
    }

    fn default_device(self, host: &cpal::Host) -> Option<cpal::Device> {
        match self {
            Self::Input => host.default_input_device(),
            Self::Output => host.default_output_device(),
        }
    }
}

/// Devices that can't even be described (gone mid-enumeration, or bogus entries
/// from cpal's macOS enumeration racing a device removal) are left out.
pub(crate) fn list() -> Result<AudioDevices> {
    let host = cpal::default_host();
    let infos = |dir: Direction| -> Result<Vec<AudioDeviceInfo>> {
        let default = dir.default_device(&host).map(|d| id(&d));
        Ok(dir
            .list(&host)?
            .iter()
            .filter(|d| dir == Direction::Output || !is_monitor_source(d))
            .filter_map(|d| {
                let name = d.description().ok()?.name().to_owned();
                let id = id(d);
                Some(AudioDeviceInfo { is_default: default.as_ref() == Some(&id), is_virtual: is_virtual(d), name, id })
            })
            .collect())
    };
    Ok(AudioDevices { inputs: infos(Direction::Input)?, outputs: infos(Direction::Output)? })
}

/// Device by id (or plain name), or the system default when `id` is `None`.
pub(crate) fn find(id: Option<&str>, dir: Direction) -> Result<cpal::Device> {
    let Some(id) = id else {
        return dir
            .default_device(&cpal::default_host())
            .ok_or_else(|| VoiceError::DeviceNotFound(format!("default {}", dir.label())));
    };
    if let Some(device) = find_exact(id, dir)? {
        return Ok(device);
    }
    // The OmniPanel virtual mic does not survive a reboot / audio server restart.
    #[cfg(target_os = "linux")]
    if dir == Direction::Output && id.ends_with(linux::SINK) {
        linux::create_virtual_mic()?;
        if let Some(device) = find_exact(id, dir)? {
            return Ok(device);
        }
    }
    Err(VoiceError::DeviceNotFound(id.to_owned()))
}

fn find_exact(id: &str, dir: Direction) -> Result<Option<cpal::Device>> {
    if let Ok(device_id) = id.parse::<cpal::DeviceId>()
        && let Ok(host) = cpal::host_from_id(device_id.host())
        && let Some(device) = host.device_by_id(&device_id)
        && dir.supports(&device)
    {
        return Ok(Some(device));
    }
    Ok(dir.list(&cpal::default_host())?.into_iter().find(|d| name(d) == id))
}

/// The best virtual cable output (see [`cable_rank`]), if one is installed.
pub(crate) fn best_cable_output() -> Result<Option<cpal::Device>> {
    let outputs = Direction::Output.list(&cpal::default_host())?;
    Ok(outputs
        .into_iter()
        .filter_map(|d| cable_rank(&name(&d)).map(|rank| (rank, d)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, d)| d))
}

pub(crate) fn id(device: &cpal::Device) -> String {
    device.id().map(|id| id.to_string()).unwrap_or_else(|_| name(device))
}

/// Display name. Never formats the device itself: cpal's `Display` fails (and
/// `to_string` panics) when the device vanished or can't be queried.
pub(crate) fn name(device: &cpal::Device) -> String {
    device.description().map(|d| d.name().to_owned()).unwrap_or_else(|_| "Unknown audio device".to_owned())
}

pub(crate) fn same_device(a: &cpal::Device, b: &cpal::Device) -> bool {
    id(a) == id(b)
}

/// Any virtual device, including app-private ones (e.g. Microsoft Teams Audio).
pub(crate) fn is_virtual(device: &cpal::Device) -> bool {
    let typed = device
        .description()
        .is_ok_and(|d| d.device_type() == DeviceType::Virtual || d.interface_type() == InterfaceType::Virtual);
    typed || cable_rank(&name(device)).is_some() || cable_rank(&id(device)).is_some()
}

/// Rank of a general-purpose virtual cable by name (lower is preferred), so a
/// default output never lands on an app's private loopback device.
pub(crate) fn cable_rank(name: &str) -> Option<usize> {
    const CABLES: [&[&str]; 7] = [
        &["omnipanel"],
        &["blackhole"],
        &["vb-audio", "vb-cable", "cable input"],
        &["voicemeeter"],
        &["loopback"],
        &["soundflower"],
        &["virtual"],
    ];
    let name = name.to_lowercase();
    CABLES.iter().position(|markers| markers.iter().any(|m| name.contains(m)))
}

/// Name of the microphone other apps should pick when we play into `output`:
/// BlackHole is the same device; VB-CABLE "CABLE Input" pairs with "CABLE Output".
pub(crate) fn mic_name_for_output(output: &str) -> String {
    if output.contains("OmniPanel_Virtual_Sink") {
        "OmniPanel_Virtual_Mic".into()
    } else if output.contains("Input") {
        output.replacen("Input", "Output", 1)
    } else {
        output.into()
    }
}

/// How to get a virtual cable on this OS.
pub(crate) fn install_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "Install BlackHole 2ch (brew install blackhole-2ch), choose it as the voice output, \
         then pick \"BlackHole 2ch\" as the microphone in Discord/OBS."
    } else if cfg!(target_os = "windows") {
        "Install VB-Audio Virtual Cable (vb-audio.com/Cable), choose \"CABLE Input\" as the voice output, \
         then pick \"CABLE Output\" as the microphone in Discord/OBS."
    } else {
        "Click \"Create virtual mic\", then pick \"OmniPanel_Virtual_Mic\" as the microphone in Discord/OBS."
    }
}

/// PulseAudio source/sink name when `device` belongs to cpal's PulseAudio host.
#[cfg(target_os = "linux")]
pub(crate) fn pulse_name(device: &cpal::Device) -> Option<String> {
    let id = device.id().ok()?;
    (id.host() == cpal::HostId::PulseAudio).then(|| id.id().to_owned())
}

/// PulseAudio lists every sink's monitor as an input; they are never a microphone.
fn is_monitor_source(device: &cpal::Device) -> bool {
    cfg!(target_os = "linux") && id(device).ends_with(".monitor")
}

impl Direction {
    fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
}

pub(crate) fn backend(err: cpal::Error) -> VoiceError {
    match err.kind() {
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::DeviceBusy => {
            VoiceError::DeviceNotFound(err.to_string())
        }
        cpal::ErrorKind::PermissionDenied => VoiceError::Backend(format!(
            "microphone access denied ({err}); allow OmniPanel in the system privacy settings"
        )),
        _ => VoiceError::Backend(err.to_string()),
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod linux {
    //! OmniPanel virtual microphone: a null sink we play into, plus a remapped
    //! source of its monitor that shows up as a regular microphone. Works with
    //! PulseAudio and PipeWire (pipewire-pulse); cpal's PulseAudio host then
    //! opens the sink by name, no ALSA routing tricks or helper processes.

    use std::process::Command;
    use std::time::Duration;

    use crate::{Result, VoiceError};

    pub const SINK: &str = "omnipanel_vmic";
    const SOURCE: &str = "omnipanel_mic";

    /// Idempotent: only loads the modules that are missing. Returns the output device id.
    pub fn create_virtual_mic() -> Result<String> {
        if !has_entry("sinks", SINK)? {
            pactl(&[
                "load-module",
                "module-null-sink",
                "sink_name=omnipanel_vmic",
                "sink_properties=device.description=OmniPanel_Virtual_Sink",
            ])?;
        }
        if !has_entry("sources", SOURCE)? {
            pactl(&[
                "load-module",
                "module-remap-source",
                "master=omnipanel_vmic.monitor",
                "source_name=omnipanel_mic",
                "source_properties=device.description=OmniPanel_Virtual_Mic",
            ])?;
        }
        Ok(format!("pulseaudio:{SINK}"))
    }

    fn has_entry(kind: &str, name: &str) -> Result<bool> {
        let list = pactl(&["list", "short", kind])?;
        Ok(list.lines().any(|line| line.split_whitespace().nth(1) == Some(name)))
    }

    fn pactl(args: &[&str]) -> Result<String> {
        let mut command = Command::new("pactl");
        command.args(args);
        // pactl waits forever on a stuck sound server.
        let out = omni_core::proc::output_within(&mut command, Duration::from_secs(10)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                return VoiceError::Backend("the sound server isn't responding (pactl timed out)".into());
            }
            VoiceError::Unsupported(format!("pactl not available ({e}); install pulseaudio-utils (or pipewire-pulse)"))
        })?;
        if !out.status.success() {
            return Err(VoiceError::Backend(format!(
                "pactl {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_virtual_cables() {
        let rank = |n| cable_rank(n).unwrap_or_else(|| panic!("{n}"));
        assert!(rank("OmniPanel_Virtual_Sink") < rank("BlackHole 2ch"));
        assert!(rank("BlackHole 16ch") < rank("CABLE Input (VB-Audio Virtual Cable)"));
        assert!(rank("Loopback Audio") < rank("Virtual Audio Device"));
        for name in ["MacBook Pro Speakers", "Speakers (Realtek(R) Audio)", "AirPods Pro", "Microsoft Teams Audio"] {
            assert_eq!(cable_rank(name), None, "{name}");
        }
    }

    #[test]
    fn maps_cable_output_to_its_microphone() {
        assert_eq!(mic_name_for_output("BlackHole 2ch"), "BlackHole 2ch");
        assert_eq!(
            mic_name_for_output("CABLE Input (VB-Audio Virtual Cable)"),
            "CABLE Output (VB-Audio Virtual Cable)"
        );
        assert_eq!(mic_name_for_output("OmniPanel_Virtual_Sink"), "OmniPanel_Virtual_Mic");
    }
}
