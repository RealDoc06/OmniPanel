//! Audio processes (`kAudioHardwarePropertyProcessObjectList`, macOS 14+) grouped into apps.
//!
//! Many apps play through helper processes (Chromium/Electron renderers, the WebKit
//! GPU process for Safari and WKWebView apps). A bundled helper is attributed to its
//! *responsible* process, so "Discord" covers every Discord helper. Command-line
//! tools (no bundle id) are kept on their own, otherwise `afplay` would show up as
//! the terminal that launched it.

use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::sync::OnceLock;

use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication};
use objc2_core_audio::{
    kAudioHardwarePropertyProcessObjectList, kAudioProcessPropertyBundleID, kAudioProcessPropertyIsRunningOutput,
    kAudioProcessPropertyPID,
};

use super::ca::{self, ObjectId, Status};

/// Processes of one app.
#[derive(Debug, Clone)]
pub struct AppGroup {
    pub id: String,
    pub name: String,
    pub pid: i32,
    /// Sorted HAL process objects.
    pub objects: Vec<ObjectId>,
    /// Producing sound (only filled in by `groups(true)`).
    pub playing: bool,
    /// A Dock app (worth listing even when silent).
    pub regular: bool,
}

#[derive(Debug, Clone)]
struct Identity {
    owner_pid: i32,
    id: String,
    name: String,
    regular: bool,
}

/// Lists HAL processes as apps. Every property read is an IPC round trip to
/// coreaudiod, so what never changes for a process object (its app identity) is
/// cached, and the costly playing state is read only on request.
#[derive(Default)]
pub struct Processes {
    known: HashMap<ObjectId, (i32, Identity)>,
}

impl Processes {
    /// Apps with audio processes, sorted by name; this process excluded.
    pub fn groups(&mut self, with_playing: bool) -> Result<Vec<AppGroup>, Status> {
        let own_pid = std::process::id() as i32;
        let objects = ca::get_list::<u32>(ca::SYSTEM, ca::global(kAudioHardwarePropertyProcessObjectList))?;
        self.known.retain(|object, _| objects.contains(object));
        let mut groups: Vec<AppGroup> = Vec::new();
        for object in objects {
            // Processes can vanish between the list and the reads; skip those. The pid
            // is re-read each time: object ids are reused after a coreaudiod restart.
            let Ok(pid) = ca::get::<i32>(object, ca::global(kAudioProcessPropertyPID)) else { continue };
            if pid <= 0 || pid == own_pid {
                continue;
            }
            if self.known.get(&object).is_none_or(|(known_pid, _)| *known_pid != pid) {
                let bundle_id = ca::get_string(object, ca::global(kAudioProcessPropertyBundleID)).unwrap_or_default();
                self.known.insert(object, (pid, identify(pid, &bundle_id)));
            }
            let identity = &self.known[&object].1;
            let group = match groups.iter_mut().find(|g| g.id == identity.id) {
                Some(group) => group,
                None => {
                    groups.push(AppGroup {
                        id: identity.id.clone(),
                        name: identity.name.clone(),
                        pid: identity.owner_pid,
                        objects: Vec::new(),
                        playing: false,
                        regular: false,
                    });
                    groups.last_mut().expect("just pushed")
                }
            };
            group.objects.push(object);
            group.regular |= identity.regular;
            if with_playing && !group.playing {
                group.playing =
                    ca::get::<u32>(object, ca::global(kAudioProcessPropertyIsRunningOutput)).is_ok_and(|r| r != 0);
            }
        }
        for group in &mut groups {
            group.objects.sort_unstable();
        }
        groups.sort_by_cached_key(|g| g.name.to_lowercase());
        Ok(groups)
    }
}

fn identify(pid: i32, bundle_id: &str) -> Identity {
    autoreleasepool(|_| {
        let own = running_app(pid);
        let is_regular = |app: &NSRunningApplication| app.activationPolicy() == NSApplicationActivationPolicy::Regular;
        let owner = match &own {
            Some(app) if is_regular(app) => None,
            _ if bundle_id.is_empty() => None,
            _ => responsible_pid(pid).filter(|&r| r > 0 && r != pid).and_then(|r| Some((r, running_app(r)?))),
        };
        let (owner_pid, app) = match owner {
            Some((owner_pid, app)) => (owner_pid, Some(app)),
            None => (pid, own),
        };
        let app_id = app.as_ref().and_then(|a| a.bundleIdentifier()).map(|s| s.to_string());
        let app_name = app.as_ref().and_then(|a| a.localizedName()).map(|s| clean_name(&s.to_string()));
        let exe = executable_name(owner_pid);
        Identity {
            owner_pid,
            id: app_id
                .or_else(|| (owner_pid == pid && !bundle_id.is_empty()).then(|| bundle_id.to_string()))
                .unwrap_or_else(|| exe.clone()),
            name: app_name.filter(|n| !n.is_empty()).unwrap_or(exe),
            regular: app.as_ref().is_some_and(|a| is_regular(a)),
        }
    })
}

fn running_app(pid: i32) -> Option<Retained<NSRunningApplication>> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
}

/// The process macOS holds responsible for `pid` (the app behind an XPC service or
/// helper). Private but long-stable libsystem API, looked up at runtime.
fn responsible_pid(pid: i32) -> Option<i32> {
    type Responsible = extern "C" fn(i32) -> i32;
    static FN: OnceLock<Option<Responsible>> = OnceLock::new();
    let f = FN.get_or_init(|| {
        ca::symbol(c"responsibility_get_pid_responsible_for_pid")
            // SAFETY: `pid_t responsibility_get_pid_responsible_for_pid(pid_t)`.
            .map(|sym| unsafe { std::mem::transmute::<*mut std::ffi::c_void, Responsible>(sym.as_ptr()) })
    });
    f.map(|f| f(pid))
}

fn executable_name(pid: i32) -> String {
    let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len > 0 {
        let path = String::from_utf8_lossy(&buf[..len as usize]);
        if let Some(name) = path.rsplit('/').next().filter(|n| !n.is_empty()) {
            return name.to_string();
        }
    }
    let mut buf = [0 as c_char; 64];
    if unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) } > 0 {
        // SAFETY: proc_name NUL-terminates on success.
        return unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
    }
    format!("pid {pid}")
}

/// Strips invisible bidi marks some apps put in their name (WhatsApp: "\u{200e}WhatsApp").
fn clean_name(name: &str) -> String {
    let invisible =
        |c: char| matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}');
    name.trim_matches(|c: char| c.is_whitespace() || invisible(c)).to_string()
}
