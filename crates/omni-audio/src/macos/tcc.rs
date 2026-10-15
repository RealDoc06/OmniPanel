//! "System Audio Recording" permission (TCC service `kTCCServiceAudioCapture`).
//!
//! There is no public API to query it; like other tap-based apps we use the private
//! `TCCAccessPreflight` / `TCCAccessRequest` from TCC.framework, loaded at runtime.
//! When they are missing we simply try, and macOS prompts on first tap use.

use std::ffi::c_void;
use std::sync::OnceLock;

use block2::RcBlock;
use objc2_core_foundation::CFString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Granted,
    Denied,
    /// Never asked (the next request shows the system prompt).
    Undetermined,
}

type Preflight = unsafe extern "C" fn(service: *const CFString, options: *const c_void) -> i64;
type Request = unsafe extern "C" fn(service: *const CFString, options: *const c_void, done: *const c_void);

struct Tcc {
    preflight: Preflight,
    request: Request,
}

fn tcc() -> Option<&'static Tcc> {
    static TCC: OnceLock<Option<Tcc>> = OnceLock::new();
    TCC.get_or_init(|| unsafe {
        let path = c"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC";
        let handle = libc::dlopen(path.as_ptr(), libc::RTLD_LAZY);
        if handle.is_null() {
            return None;
        }
        let preflight = libc::dlsym(handle, c"TCCAccessPreflight".as_ptr());
        let request = libc::dlsym(handle, c"TCCAccessRequest".as_ptr());
        (!preflight.is_null() && !request.is_null()).then(|| Tcc {
            preflight: std::mem::transmute::<*mut c_void, Preflight>(preflight),
            request: std::mem::transmute::<*mut c_void, Request>(request),
        })
    })
    .as_ref()
}

fn service() -> objc2_core_foundation::CFRetained<CFString> {
    CFString::from_static_str("kTCCServiceAudioCapture")
}

/// `None` when the private API is unavailable.
pub fn status() -> Option<Permission> {
    let tcc = tcc()?;
    Some(match unsafe { (tcc.preflight)(&*service(), std::ptr::null()) } {
        0 => Permission::Granted,
        1 => Permission::Denied,
        _ => Permission::Undetermined,
    })
}

/// Shows the system prompt (once per app); `done(granted)` runs on a TCC queue.
/// Returns false when the API is unavailable.
pub fn request(done: impl Fn(bool) + Send + Sync + 'static) -> bool {
    let Some(tcc) = tcc() else { return false };
    let block = RcBlock::new(move |granted: u8| done(granted != 0));
    // TCC copies the block, so ours can be released when this returns.
    unsafe { (tcc.request)(&*service(), std::ptr::null(), RcBlock::as_ptr(&block).cast()) };
    true
}
