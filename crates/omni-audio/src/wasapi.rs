//! Windows backend: WASAPI. `IAudioEndpointVolume` on the default render / capture
//! endpoints; per-app volume through audio sessions (`ISimpleAudioVolume`) on every
//! active render endpoint, grouped by executable like the Windows volume mixer.
//!
//! Apps cannot be sent to a device of their own yet: Windows only offers that through
//! an undocumented interface (`IAudioPolicyConfig`), which is not used until it has
//! been proven on real hardware. `capabilities()` says so.

use std::collections::HashMap;
use std::ptr;
use std::sync::Mutex;

use windows::Win32::Foundation::{CloseHandle, S_OK};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, AudioSessionStateExpired, DEVICE_STATE_ACTIVE, IAudioSessionControl,
    IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
    eCapture, eConsole, eRender,
};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::{Interface, PCWSTR, PWSTR, w};

use crate::{AppAudio, AudioCapabilities, AudioError, Backend, Endpoint, Result, VolumeState};

/// Id of the "System sounds" session (it has no executable of its own).
const SYSTEM_SOUNDS: &str = "system-sounds";
/// `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`: no such endpoint.
const NOT_FOUND: i32 = 0x8007_0490_u32 as i32;

pub(crate) struct Platform {
    /// Display names by executable path (version resources are read from disk).
    names: Mutex<HashMap<String, String>>,
}

impl From<windows::core::Error> for AudioError {
    fn from(e: windows::core::Error) -> Self {
        AudioError::Backend(format!("WASAPI: {} ({})", e.message(), e.code()))
    }
}

/// Initializes COM (MTA) once per calling thread; host threads are reused, so the
/// guard lives until the thread exits.
struct Com(bool);

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

thread_local! {
    // S_FALSE (already initialized) also needs a matching CoUninitialize; RPC_E_CHANGED_MODE
    // (an STA thread) does not, and COM is usable there too.
    static COM: Com = Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok());
}

fn enumerator() -> Result<IMMDeviceEnumerator> {
    COM.with(|_| ());
    Ok(unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?)
}

fn endpoint_volume(endpoint: Endpoint) -> Result<IAudioEndpointVolume> {
    let flow = match endpoint {
        Endpoint::Output => eRender,
        Endpoint::Input => eCapture,
    };
    let device = unsafe { enumerator()?.GetDefaultAudioEndpoint(flow, eConsole) }.map_err(|e| {
        if e.code().0 == NOT_FOUND {
            AudioError::Unsupported(match endpoint {
                Endpoint::Output => "no output device".into(),
                Endpoint::Input => "no input device".into(),
            })
        } else {
            e.into()
        }
    })?;
    Ok(unsafe { device.Activate(CLSCTX_ALL, None) }?)
}

/// One audio session, resolved to the app it belongs to.
struct Session {
    id: String,
    name: String,
    pid: u32,
    playing: bool,
    volume: ISimpleAudioVolume,
}

impl Platform {
    /// Live sessions on every active render endpoint.
    fn sessions(&self) -> Result<Vec<Session>> {
        let devices = unsafe { enumerator()?.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }?;
        let mut sessions = Vec::new();
        for i in 0..unsafe { devices.GetCount() }? {
            // A device can disappear mid-enumeration; skip it rather than fail the listing.
            let Ok(device) = (unsafe { devices.Item(i) }) else { continue };
            let Ok(manager) = (unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) }) else { continue };
            let Ok(list) = (unsafe { manager.GetSessionEnumerator() }) else { continue };
            for j in 0..unsafe { list.GetCount() }.unwrap_or(0) {
                if let Some(session) = unsafe { list.GetSession(j) }.ok().and_then(|c| self.session(&c)) {
                    sessions.push(session);
                }
            }
        }
        Ok(sessions)
    }

    fn session(&self, control: &IAudioSessionControl) -> Option<Session> {
        let state = unsafe { control.GetState() }.ok()?;
        if state == AudioSessionStateExpired {
            return None;
        }
        let control2: IAudioSessionControl2 = control.cast().ok()?;
        let volume: ISimpleAudioVolume = control.cast().ok()?;
        let pid = unsafe { control2.GetProcessId() }.unwrap_or(0);
        let playing = state == AudioSessionStateActive;
        if unsafe { control2.IsSystemSoundsSession() } == S_OK {
            return Some(Session { id: SYSTEM_SOUNDS.into(), name: "System sounds".into(), pid, playing, volume });
        }
        // "@%SystemRoot%\..." display names are resource references, not names.
        let display = unsafe { control.GetDisplayName() }
            .ok()
            .and_then(take_string)
            .filter(|n| !n.trim().is_empty() && !n.starts_with('@'));
        let path = process_path(pid);
        let exe = path.as_deref().and_then(|p| p.rsplit('\\').next()).map(str::to_string);
        let id = match &exe {
            Some(exe) => exe.to_lowercase(),
            None if pid != 0 => format!("pid-{pid}"),
            None => display.clone()?,
        };
        let name = display
            .or_else(|| path.as_deref().and_then(|p| self.file_description(p)))
            .or_else(|| exe.map(|e| e.trim_end_matches(".exe").trim_end_matches(".EXE").to_string()))
            .unwrap_or_else(|| id.clone());
        Some(Session { id, name, pid, playing, volume })
    }

    fn file_description(&self, path: &str) -> Option<String> {
        let mut names = self.names.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(name) = names.get(path) {
            return Some(name.clone());
        }
        let name = file_description(path)?;
        names.insert(path.to_string(), name.clone());
        Some(name)
    }

    /// Sessions grouped into apps, sorted by name.
    fn grouped(&self) -> Result<Vec<(AppAudio, Vec<ISimpleAudioVolume>)>> {
        let mut apps: Vec<(AppAudio, Vec<ISimpleAudioVolume>)> = Vec::new();
        for session in self.sessions()? {
            let volume = unsafe { session.volume.GetMasterVolume() }.unwrap_or(1.0).clamp(0.0, 1.0);
            let muted = unsafe { session.volume.GetMute() }.is_ok_and(|m| m.as_bool());
            match apps.iter_mut().find(|(a, _)| a.id == session.id) {
                Some((app, volumes)) => {
                    app.volume = app.volume.max(volume);
                    app.muted &= muted;
                    app.playing |= session.playing;
                    volumes.push(session.volume);
                }
                None => apps.push((
                    AppAudio {
                        id: session.id,
                        name: session.name,
                        pid: (session.pid != 0).then_some(session.pid),
                        volume,
                        muted,
                        playing: session.playing,
                        output: None,
                        input: None,
                    },
                    vec![session.volume],
                )),
            }
        }
        apps.sort_by_cached_key(|(a, _)| a.name.to_lowercase());
        Ok(apps)
    }

    fn app_sessions(&self, app: &AppAudio) -> Result<Vec<ISimpleAudioVolume>> {
        self.grouped()?
            .into_iter()
            .find(|(a, _)| a.id == app.id)
            .map(|(_, volumes)| volumes)
            .ok_or_else(|| AudioError::AppNotFound(app.id.clone()))
    }
}

impl Backend for Platform {
    fn new() -> Self {
        Platform { names: Mutex::default() }
    }

    fn capabilities(&self) -> AudioCapabilities {
        AudioCapabilities {
            per_app_volume: true,
            note: None,
            per_app_output: false,
            per_app_input: false,
            device_note: Some("Choosing a device per app is not available on Windows yet.".into()),
        }
    }

    fn endpoint(&self, endpoint: Endpoint) -> Result<VolumeState> {
        let control = endpoint_volume(endpoint)?;
        Ok(VolumeState {
            volume: unsafe { control.GetMasterVolumeLevelScalar() }?,
            muted: unsafe { control.GetMute() }?.as_bool(),
        })
    }

    fn set_endpoint_volume(&self, endpoint: Endpoint, volume: f32) -> Result<()> {
        Ok(unsafe { endpoint_volume(endpoint)?.SetMasterVolumeLevelScalar(volume, ptr::null()) }?)
    }

    fn set_endpoint_muted(&self, endpoint: Endpoint, muted: bool) -> Result<()> {
        Ok(unsafe { endpoint_volume(endpoint)?.SetMute(muted, ptr::null()) }?)
    }

    fn apps(&self) -> Result<Vec<AppAudio>> {
        Ok(self.grouped()?.into_iter().map(|(app, _)| app).collect())
    }

    fn set_app_volume(&self, app: &AppAudio, volume: f32) -> Result<()> {
        for session in self.app_sessions(app)? {
            unsafe { session.SetMasterVolume(volume, ptr::null()) }?;
        }
        Ok(())
    }

    fn set_app_muted(&self, app: &AppAudio, muted: bool) -> Result<()> {
        for session in self.app_sessions(app)? {
            unsafe { session.SetMute(muted, ptr::null()) }?;
        }
        Ok(())
    }
}

/// Copies a COM-allocated string and frees it.
fn take_string(value: PWSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let text = unsafe { value.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
    text
}

fn process_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let result = unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) };
    let _ = unsafe { CloseHandle(handle) };
    result.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// `FileDescription` from the executable's version resource ("Google Chrome" for chrome.exe).
fn file_description(path: &str) -> Option<String> {
    let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let path = PCWSTR(path.as_ptr());
    let size = unsafe { GetFileVersionInfoSizeW(path, None) };
    if size == 0 {
        return None;
    }
    // u32 storage keeps the block DWORD-aligned, as the resource layout expects.
    let mut block = vec![0u32; (size as usize).div_ceil(4)];
    unsafe { GetFileVersionInfoW(path, None, size, block.as_mut_ptr().cast()) }.ok()?;
    let query = |key: PCWSTR| {
        let mut value = ptr::null_mut();
        let mut len = 0u32;
        let found = unsafe { VerQueryValueW(block.as_ptr().cast(), key, &mut value, &mut len) }.as_bool();
        (found && !value.is_null() && len > 0).then_some((value, len))
    };
    let (translation, _) = query(w!("\\VarFileInfo\\Translation"))?;
    let [lang, codepage] = unsafe { translation.cast::<[u16; 2]>().read_unaligned() };
    let key: Vec<u16> =
        format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription").encode_utf16().chain(Some(0)).collect();
    let (text, len) = query(PCWSTR(key.as_ptr()))?;
    let text = unsafe { std::slice::from_raw_parts(text.cast::<u16>(), len as usize) };
    let text = String::from_utf16_lossy(text);
    let text = text.trim_end_matches('\0').trim();
    (!text.is_empty()).then(|| text.to_string())
}
