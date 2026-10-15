//! Typed wrappers over the CoreAudio `AudioObject` property API.

use std::fmt;
use std::mem::{MaybeUninit, size_of};
use std::ptr::{self, NonNull};

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectHasProperty, AudioObjectID,
    AudioObjectIsPropertySettable, AudioObjectPropertyAddress, AudioObjectSetPropertyData,
    kAudioHardwareUnspecifiedError, kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
    kAudioObjectSystemObject,
};
use objc2_core_audio_types::AudioStreamBasicDescription;
use objc2_core_foundation::{CFRetained, CFString};

use crate::AudioError;

pub type ObjectId = AudioObjectID;
pub const SYSTEM: ObjectId = kAudioObjectSystemObject as ObjectId;
pub const GLOBAL: u32 = kAudioObjectPropertyScopeGlobal;
pub const MAIN: u32 = kAudioObjectPropertyElementMain;

/// A failed CoreAudio call (`OSStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub i32);

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Most HAL errors are four-char codes like 'who?' or '!obj'.
        let bytes = self.0.to_be_bytes();
        match std::str::from_utf8(&bytes) {
            Ok(code) if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') => write!(f, "'{code}'"),
            _ => write!(f, "{}", self.0),
        }
    }
}

impl From<Status> for AudioError {
    fn from(status: Status) -> Self {
        AudioError::Backend(format!("CoreAudio error {status}"))
    }
}

pub fn check(status: i32) -> Result<(), Status> {
    if status == 0 { Ok(()) } else { Err(Status(status)) }
}

pub const fn prop(selector: u32, scope: u32, element: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: element }
}

pub const fn global(selector: u32) -> AudioObjectPropertyAddress {
    prop(selector, GLOBAL, MAIN)
}

/// Property values that are plain bytes: valid when zeroed, copied by value.
///
/// # Safety
/// Implementors must be `repr(C)`-compatible POD types without pointers that need ownership.
pub unsafe trait Pod: Copy {}
unsafe impl Pod for u32 {}
unsafe impl Pod for i32 {}
unsafe impl Pod for f32 {}
unsafe impl Pod for f64 {}
unsafe impl Pod for [u32; 2] {}
unsafe impl Pod for AudioStreamBasicDescription {}

pub fn has(id: ObjectId, address: AudioObjectPropertyAddress) -> bool {
    unsafe { AudioObjectHasProperty(id, NonNull::from(&address)) }
}

pub fn settable(id: ObjectId, address: AudioObjectPropertyAddress) -> bool {
    let mut settable = 0u8;
    let status = unsafe { AudioObjectIsPropertySettable(id, NonNull::from(&address), NonNull::from(&mut settable)) };
    status == 0 && settable != 0
}

pub fn get<T: Pod>(id: ObjectId, address: AudioObjectPropertyAddress) -> Result<T, Status> {
    let mut value = MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>() as u32;
    check(unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    })?;
    // SAFETY: `T: Pod` is valid when zeroed, and the HAL wrote at most `size_of::<T>()` bytes.
    Ok(unsafe { value.assume_init() })
}

/// Reads a property that takes a qualifier (e.g. a pid to translate).
pub fn get_qualified<T: Pod, Q: Pod>(
    id: ObjectId,
    address: AudioObjectPropertyAddress,
    qualifier: &Q,
) -> Result<T, Status> {
    let mut value = MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>() as u32;
    check(unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&address),
            size_of::<Q>() as u32,
            ptr::from_ref(qualifier).cast(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    })?;
    // SAFETY: as in `get`.
    Ok(unsafe { value.assume_init() })
}

pub fn get_list<T: Pod>(id: ObjectId, address: AudioObjectPropertyAddress) -> Result<Vec<T>, Status> {
    let mut size = 0u32;
    check(unsafe {
        AudioObjectGetPropertyDataSize(id, NonNull::from(&address), 0, ptr::null(), NonNull::from(&mut size))
    })?;
    let len = size as usize / size_of::<T>();
    if len == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: zeroed `Pod` values are valid.
    let mut items = vec![unsafe { MaybeUninit::<T>::zeroed().assume_init() }; len];
    check(unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(items.as_mut_ptr()).cast(),
        )
    })?;
    items.truncate(size as usize / size_of::<T>());
    Ok(items)
}

/// Reads a `CFString` property (the HAL hands out a +1 reference).
pub fn get_string(id: ObjectId, address: AudioObjectPropertyAddress) -> Result<String, Status> {
    let mut raw: *const CFString = ptr::null();
    let mut size = size_of::<*const CFString>() as u32;
    check(unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut raw).cast(),
        )
    })?;
    let raw = NonNull::new(raw.cast_mut()).ok_or(Status(kAudioHardwareUnspecifiedError))?;
    // SAFETY: CoreAudio returns an owned (retained) CFString.
    Ok(unsafe { CFRetained::from_raw(raw) }.to_string())
}

pub fn set<T: Pod>(id: ObjectId, address: AudioObjectPropertyAddress, value: &T) -> Result<(), Status> {
    check(unsafe {
        AudioObjectSetPropertyData(
            id,
            NonNull::from(&address),
            0,
            ptr::null(),
            size_of::<T>() as u32,
            NonNull::from(value).cast(),
        )
    })
}

/// `dlsym` lookup in the already loaded images; used for APIs newer than the deployment target.
pub fn symbol(name: &std::ffi::CStr) -> Option<NonNull<std::ffi::c_void>> {
    NonNull::new(unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) })
}
