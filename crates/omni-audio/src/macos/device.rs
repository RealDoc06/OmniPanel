//! Volume and mute of the default output / input device.
//!
//! Devices expose volume in different ways: a main element (most devices), only
//! per-channel controls (some USB interfaces), or nothing at all (HDMI, many audio
//! interfaces with a hardware knob). We try the main element, then the HAL's
//! virtual main volume, then the preferred stereo channels. Without any, the
//! backend falls back to a software master (see `super::State::master`).

use objc2_core_audio::{
    AudioObjectPropertyAddress, kAudioDevicePropertyDeviceUID, kAudioDevicePropertyMute,
    kAudioDevicePropertyPreferredChannelsForStereo, kAudioDevicePropertyVolumeScalar,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyName,
    kAudioObjectPropertyScopeInput, kAudioObjectPropertyScopeOutput,
};

use super::ca::{self, MAIN, ObjectId};
use crate::{AudioError, Endpoint, Result, VolumeState};

/// `kAudioHardwareServiceDeviceProperty_VirtualMainVolume` ('vmvc'): balance-preserving
/// volume across channels; served by the HAL for devices without a main element.
const VIRTUAL_MAIN_VOLUME: u32 = u32::from_be_bytes(*b"vmvc");

pub fn scope(endpoint: Endpoint) -> u32 {
    match endpoint {
        Endpoint::Output => kAudioObjectPropertyScopeOutput,
        Endpoint::Input => kAudioObjectPropertyScopeInput,
    }
}

pub fn default_device(endpoint: Endpoint) -> Result<ObjectId> {
    let selector = match endpoint {
        Endpoint::Output => kAudioHardwarePropertyDefaultOutputDevice,
        Endpoint::Input => kAudioHardwarePropertyDefaultInputDevice,
    };
    match ca::get::<u32>(ca::SYSTEM, ca::global(selector))? {
        0 => Err(AudioError::Unsupported(match endpoint {
            Endpoint::Output => "no output device".into(),
            Endpoint::Input => "no input device".into(),
        })),
        device => Ok(device),
    }
}

pub fn name(device: ObjectId) -> String {
    ca::get_string(device, ca::global(kAudioObjectPropertyName)).unwrap_or_else(|_| "audio device".into())
}

pub fn uid(device: ObjectId) -> Result<String> {
    Ok(ca::get_string(device, ca::global(kAudioDevicePropertyDeviceUID))?)
}

/// Hardware volume; `None` when the device has no settable volume control.
pub fn volume(device: ObjectId, endpoint: Endpoint) -> Result<Option<f32>> {
    let Some(control) = Control::find(device, endpoint, Kind::Volume) else { return Ok(None) };
    Ok(Some(control.read::<f32>(device)?.into_iter().fold(0.0, f32::max).clamp(0.0, 1.0)))
}

/// Hardware mute; `None` when the device has no settable mute control.
pub fn muted(device: ObjectId, endpoint: Endpoint) -> Result<Option<bool>> {
    let Some(control) = Control::find(device, endpoint, Kind::Mute) else { return Ok(None) };
    Ok(Some(control.read::<u32>(device)?.iter().all(|&m| m != 0)))
}

pub fn has_volume(device: ObjectId, endpoint: Endpoint) -> bool {
    Control::find(device, endpoint, Kind::Volume).is_some()
}

pub fn has_mute(device: ObjectId, endpoint: Endpoint) -> bool {
    Control::find(device, endpoint, Kind::Mute).is_some()
}

pub fn state(device: ObjectId, endpoint: Endpoint) -> Result<VolumeState> {
    let volume = volume(device, endpoint)?.ok_or_else(|| no_volume(device))?;
    Ok(VolumeState { volume, muted: muted(device, endpoint)?.unwrap_or(false) })
}

pub fn set_volume(device: ObjectId, endpoint: Endpoint, volume: f32) -> Result<()> {
    Control::find(device, endpoint, Kind::Volume).ok_or_else(|| no_volume(device))?.write(device, volume)
}

pub fn set_muted(device: ObjectId, endpoint: Endpoint, muted: bool) -> Result<()> {
    Control::find(device, endpoint, Kind::Mute)
        .ok_or_else(|| AudioError::Unsupported(format!("{} cannot be muted", name(device))))?
        .write(device, u32::from(muted))
}

fn no_volume(device: ObjectId) -> AudioError {
    AudioError::Unsupported(format!("{} has no software volume control", name(device)))
}

#[derive(Clone, Copy)]
enum Kind {
    Volume,
    Mute,
}

/// The property addresses that make up one logical control.
enum Control {
    Main(AudioObjectPropertyAddress),
    Channels(Vec<AudioObjectPropertyAddress>),
}

impl Control {
    fn find(device: ObjectId, endpoint: Endpoint, kind: Kind) -> Option<Self> {
        let scope = scope(endpoint);
        let selector = match kind {
            Kind::Volume => kAudioDevicePropertyVolumeScalar,
            Kind::Mute => kAudioDevicePropertyMute,
        };
        // Read-only controls (some HDMI sinks report a fixed level) count as absent.
        let usable = |address| ca::has(device, address) && ca::settable(device, address);
        let main = ca::prop(selector, scope, MAIN);
        if usable(main) {
            return Some(Control::Main(main));
        }
        if let Kind::Volume = kind {
            let virtual_main = ca::prop(VIRTUAL_MAIN_VOLUME, scope, MAIN);
            if usable(virtual_main) {
                return Some(Control::Main(virtual_main));
            }
        }
        let stereo = ca::get::<[u32; 2]>(device, ca::prop(kAudioDevicePropertyPreferredChannelsForStereo, scope, MAIN))
            .unwrap_or([1, 2]);
        let mut channels: Vec<_> = stereo.iter().map(|&ch| ca::prop(selector, scope, ch)).collect();
        channels.dedup_by_key(|a| a.mElement);
        channels.retain(|&a| usable(a));
        (!channels.is_empty()).then_some(Control::Channels(channels))
    }

    fn addresses(&self) -> &[AudioObjectPropertyAddress] {
        match self {
            Control::Main(address) => std::slice::from_ref(address),
            Control::Channels(addresses) => addresses,
        }
    }

    fn read<T: ca::Pod>(&self, device: ObjectId) -> Result<Vec<T>> {
        Ok(self.addresses().iter().map(|&a| ca::get(device, a)).collect::<std::result::Result<_, _>>()?)
    }

    fn write<T: ca::Pod>(&self, device: ObjectId, value: T) -> Result<()> {
        for &address in self.addresses() {
            ca::set(device, address, &value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_audio::{kAudioDevicePropertyStreams, kAudioHardwarePropertyDevices};

    use super::*;

    /// Round-trips volume and mute on a device that is not the default (so nothing is
    /// audible) and restores the original values. Needs real hardware:
    /// `cargo test -p omni-audio -- --ignored`.
    #[test]
    #[ignore]
    fn set_and_restore_on_idle_device() {
        let default_out = default_device(Endpoint::Output).ok();
        let devices = ca::get_list::<u32>(ca::SYSTEM, ca::global(kAudioHardwarePropertyDevices)).unwrap();
        let mut tested = 0;
        for device in devices.into_iter().filter(|&d| Some(d) != default_out) {
            let streams = ca::prop(kAudioDevicePropertyStreams, scope(Endpoint::Output), MAIN);
            if ca::get_list::<u32>(device, streams).map_or(true, |s| s.is_empty()) {
                continue;
            }
            let Ok(original) = state(device, Endpoint::Output) else { continue };
            let target = if original.volume > 0.5 { 0.25 } else { 0.75 };
            set_volume(device, Endpoint::Output, target).unwrap();
            let changed = state(device, Endpoint::Output).unwrap();
            set_volume(device, Endpoint::Output, original.volume).unwrap();
            assert!((changed.volume - target).abs() < 0.02, "{}: {changed:?}", name(device));
            if set_muted(device, Endpoint::Output, !original.muted).is_ok() {
                let toggled = state(device, Endpoint::Output).unwrap().muted;
                set_muted(device, Endpoint::Output, original.muted).unwrap();
                assert_eq!(toggled, !original.muted);
            }
            assert_eq!(state(device, Endpoint::Output).unwrap(), original);
            tested += 1;
        }
        assert!(tested > 0, "no idle output device with a volume control");
    }
}
