//! Listening to apps without changing how they play: an unmuted Core Audio process
//! tap (macOS 14.2+), read through a private aggregate device that holds nothing
//! but the tap. omni-voice mixes what it hears into the voice output, so a call
//! hears an app the user is listening to.
//!
//! Unlike the volume taps in `tap.rs`, nothing is muted or played back, and the tap
//! is a stereo mixdown of the apps' processes on whatever device they play. Taps
//! and private aggregates die with our process, so a crash leaves nothing behind.

use std::ffi::{CStr, c_void};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

use objc2::AllocAnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareDestroyAggregateDevice, AudioObjectAddPropertyListener,
    AudioObjectID, AudioObjectPropertyAddress, AudioObjectSetPropertyData, CATapDescription, CATapMuteBehavior,
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceNameKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioDevicePropertyDeviceIsAlive,
    kAudioDevicePropertyNominalSampleRate, kAudioHardwarePropertyProcessObjectList, kAudioSubTapDriftCompensationKey,
    kAudioSubTapUIDKey, kAudioTapPropertyDescription, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsFloat,
    kAudioFormatLinearPCM,
};
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFRetained, CFString, CFType};
use objc2_foundation::{NSArray, NSNumber, NSString, NSUUID};

use super::ca::{self, ObjectId};
use super::process::Processes;
use super::tap::{self, UID_PREFIX};
use super::tcc::{self, Permission};
use crate::{AudioError, Result};

const UNSUPPORTED: &str = "Sharing app sound needs macOS 14.2 or later.";
const PERMISSION_HINT: &str = "to share app sound, allow OmniPanel under System Settings > Privacy & Security > \
     Screen & System Audio Recording (System Audio Recording Only)";
/// Mono frames handed to the sink at once; longer IO cycles go out in pieces.
const PIECE: usize = 4096;
/// IO silence after which a tap counts as stalled (generous for start-up).
const STALL: Duration = Duration::from_secs(2);

/// Receives the captured sound as mono samples, on a real-time thread: it must not
/// block or allocate.
pub type Sink = Box<dyn FnMut(&[f32]) + Send>;

type Changed = Arc<dyn Fn() + Send + Sync>;

/// Why apps cannot be captured on this macOS, if so.
pub fn unsupported() -> Option<&'static str> {
    (!tap::supported()).then_some(UNSUPPORTED)
}

/// Captures the sound of chosen apps. Idle (no tap, no permission prompt) until
/// [`Self::sync`] names an app that is running.
pub struct AppCapture {
    processes: Processes,
    running: Option<Running>,
    /// The apps the running tap was made for.
    apps: Vec<String>,
    changed: Changed,
    _listening: Listening,
}

impl AppCapture {
    /// `changed` is called from other threads when audio processes start or stop
    /// (apps launch helpers for playback) and when the permission prompt is
    /// answered; answer it with [`Self::sync`]. (An owner that let go of the
    /// capture meanwhile finds the answer at its next `sync` of a new one.)
    pub fn new(changed: impl Fn() + Send + Sync + 'static) -> Self {
        let changed: Changed = Arc::new(changed);
        Self {
            processes: Processes::default(),
            running: None,
            apps: Vec::new(),
            _listening: Listening::start(changed.clone()),
            changed,
        }
    }

    /// Capture exactly the apps in `apps` (their exact names, ignoring case) that
    /// run now, keeping a healthy tap going. A new tap asks `open` for a sink,
    /// telling it the sample rate; the sink of a replaced or stopped tap is dropped.
    /// Returns the apps that are not running.
    pub fn sync(&mut self, apps: &[String], open: &mut dyn FnMut(u32) -> Sink) -> Result<Vec<String>> {
        let (objects, waiting) = self.objects_of(apps)?;
        // Another list of apps gets a new tap (and sink): the old one may still be
        // delivering sound of an app that is no longer wanted.
        if self.apps != apps {
            self.running = None;
            self.apps = apps.to_vec();
        }
        if objects.is_empty() {
            self.running = None;
            return Ok(waiting);
        }
        if let Some(running) = self.running.as_mut()
            && running.healthy()
            // An app started or stopped a helper: retarget without a gap.
            && (running.objects == objects || running.set_objects(&objects).is_ok())
        {
            return Ok(waiting);
        }
        self.running = None;
        self.permit()?;
        self.running = Some(Running::start(objects, open)?);
        Ok(waiting)
    }

    /// Sorted HAL process objects of every app the queries name, and the queries
    /// naming no running app. Stricter than per-app volume, which also takes ids
    /// and parts of names: "Discord" must never start sharing "Discord PTB" once
    /// Discord quits.
    fn objects_of(&mut self, apps: &[String]) -> Result<(Vec<ObjectId>, Vec<String>)> {
        let groups = self.processes.groups(false)?;
        let (mut objects, mut waiting) = (Vec::new(), Vec::new());
        for app in apps {
            let query = app.trim().to_lowercase();
            let named = groups.iter().filter(|g| !query.is_empty() && g.name.trim().to_lowercase() == query);
            let before = objects.len();
            objects.extend(named.flat_map(|g| g.objects.iter().copied()));
            if objects.len() == before {
                waiting.push(app.clone());
            }
        }
        objects.sort_unstable();
        objects.dedup();
        Ok((objects, waiting))
    }

    /// Ok when a tap may be created now. Otherwise asks once per run (the system
    /// prompt, whose answer triggers `changed`) and explains.
    fn permit(&self) -> Result<()> {
        // Asked and answered in this run. The answer is kept: the status can lag behind it.
        static PROMPTED: AtomicBool = AtomicBool::new(false);
        static GRANTED: AtomicBool = AtomicBool::new(false);
        if GRANTED.load(Relaxed) {
            return Ok(());
        }
        match tcc::status() {
            Some(Permission::Granted) | None => Ok(()),
            Some(Permission::Denied) => Err(AudioError::Permission(PERMISSION_HINT.into())),
            Some(Permission::Undetermined) => {
                if !PROMPTED.swap(true, Relaxed) {
                    let changed = self.changed.clone();
                    tcc::request(move |granted| {
                        GRANTED.fetch_or(granted, Relaxed);
                        changed();
                    });
                }
                Err(AudioError::Permission("approve the System Audio Recording prompt to share app sound".into()))
            }
        }
    }
}

/// A running tap; dropping it stops the capture and frees the sink.
struct Running {
    tap: ObjectId,
    uuid: String,
    aggregate: ObjectId,
    io_proc: AudioDeviceIOProcID,
    started: bool,
    /// Owned by the IO thread while the IOProc exists; freed in `Drop` after it.
    client: *mut Client,
    /// IO cycles run (counted by the IO thread), their count at the last health
    /// check and when it last moved.
    cycles: Arc<AtomicU32>,
    seen: (u32, Instant),
    objects: Vec<ObjectId>,
    rate: f64,
}

// SAFETY: `client` is only dereferenced by the IO thread and freed once the IOProc is gone.
unsafe impl Send for Running {}

impl Running {
    fn start(objects: Vec<ObjectId>, open: &mut dyn FnMut(u32) -> Sink) -> Result<Self> {
        let api = tap::api().ok_or_else(|| AudioError::Unsupported(UNSUPPORTED.into()))?;
        let mut running = Running {
            tap: 0,
            uuid: String::new(),
            aggregate: 0,
            io_proc: None,
            started: false,
            client: ptr::null_mut(),
            cycles: Arc::default(),
            seen: (0, Instant::now()),
            objects,
            rate: 0.0,
        };
        // On any error below, `Drop` releases whatever was created so far.
        let description = running.description(None);
        running.uuid = autoreleasepool(|_| unsafe { description.UUID().UUIDString() }.to_string());
        ca::check(unsafe { (api.create)(&*description, &mut running.tap) })
            .map_err(|s| AudioError::Backend(format!("could not create the process tap (CoreAudio error {s})")))?;

        let format = tap_format(running.tap)?;
        running.rate = format.mSampleRate;
        let aggregate = aggregate_description(&running.uuid);
        ca::check(unsafe {
            AudioHardwareCreateAggregateDevice(aggregate.as_ref(), NonNull::from(&mut running.aggregate))
        })?;
        // The tap is the aggregate's one input stream.
        if !tap::wait_until_ready(running.aggregate, 1) {
            return Err(AudioError::Backend("the process tap did not start".into()));
        }
        // The aggregate clocks the IO: its rate is the one the sink is told. (It has
        // no other member, so it runs at the tap's.)
        running.rate = aggregate_rate(running.aggregate).unwrap_or(running.rate);
        let sink = open(running.rate.round() as u32);
        let client = Client { sink, mono: vec![0.0; PIECE].into_boxed_slice(), cycles: running.cycles.clone() };
        running.client = Box::into_raw(Box::new(client));
        ca::check(unsafe {
            AudioDeviceCreateIOProcID(
                running.aggregate,
                Some(io_proc),
                running.client.cast(),
                NonNull::from(&mut running.io_proc),
            )
        })?;
        ca::check(unsafe { AudioDeviceStart(running.aggregate, running.io_proc) })?;
        running.started = true;
        running.seen.1 = Instant::now();
        Ok(running)
    }

    /// Retargets the tap in place, without interrupting the capture.
    fn set_objects(&mut self, objects: &[ObjectId]) -> Result<()> {
        let previous = std::mem::replace(&mut self.objects, objects.to_vec());
        let description = self.description(Some(&self.uuid));
        let raw = Retained::as_ptr(&description);
        let address = ca::global(kAudioTapPropertyDescription);
        let result = ca::check(unsafe {
            AudioObjectSetPropertyData(
                self.tap,
                NonNull::from(&address),
                0,
                ptr::null(),
                size_of::<*const CATapDescription>() as u32,
                NonNull::from(&raw).cast(),
            )
        });
        if result.is_err() {
            self.objects = previous;
        }
        Ok(result?)
    }

    /// False once the aggregate died (coreaudiod restart), stopped calling us, or
    /// the tap changed sample rate (the output device did): all need a rebuild.
    fn healthy(&mut self) -> bool {
        let cycles = self.cycles.load(Relaxed);
        if cycles != self.seen.0 {
            self.seen = (cycles, Instant::now());
        }
        ca::get::<u32>(self.aggregate, ca::global(kAudioDevicePropertyDeviceIsAlive)).is_ok_and(|v| v != 0)
            && self.seen.1.elapsed() < STALL
            && tap_format(self.tap).is_ok_and(|f| aggregate_rate(self.aggregate).unwrap_or(f.mSampleRate) == self.rate)
    }

    /// A private stereo mixdown of `objects` that leaves them playing.
    fn description(&self, uuid: Option<&str>) -> Retained<CATapDescription> {
        autoreleasepool(|_| unsafe {
            let numbers: Vec<_> = self.objects.iter().map(|&o| NSNumber::new_u32(o)).collect();
            let description = CATapDescription::initStereoMixdownOfProcesses(
                CATapDescription::alloc(),
                &NSArray::from_retained_slice(&numbers),
            );
            description.setName(&NSString::from_str("OmniPanel share"));
            description.setPrivate(true);
            description.setMuteBehavior(CATapMuteBehavior::Unmuted);
            if let Some(uuid) = uuid.and_then(|u| NSUUID::initWithUUIDString(NSUUID::alloc(), &NSString::from_str(u))) {
                description.setUUID(&uuid);
            }
            description
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // Order matters: once AudioDeviceStop / DestroyIOProcID return, the IO thread
        // no longer touches `client`.
        unsafe {
            if self.started {
                AudioDeviceStop(self.aggregate, self.io_proc);
            }
            // An IOProc that could not be removed may still be called: its client
            // is then leaked rather than freed under it.
            let removed = self.io_proc.is_none() || AudioDeviceDestroyIOProcID(self.aggregate, self.io_proc) == 0;
            if self.aggregate != 0 {
                AudioHardwareDestroyAggregateDevice(self.aggregate);
            }
            if self.tap != 0
                && let Some(api) = tap::api()
            {
                (api.destroy)(self.tap);
            }
            if removed && !self.client.is_null() {
                drop(Box::from_raw(self.client));
            }
        }
    }
}

/// The tap's stream format; anything but 32-bit float at a real sample rate is refused.
fn tap_format(tap: ObjectId) -> Result<AudioStreamBasicDescription> {
    let format = ca::get::<AudioStreamBasicDescription>(tap, ca::global(kAudioTapPropertyFormat))?;
    if format.mFormatID != kAudioFormatLinearPCM
        || format.mFormatFlags & kAudioFormatFlagIsFloat == 0
        || format.mBitsPerChannel != 32
    {
        return Err(AudioError::Unsupported("the process tap does not use a float format".into()));
    }
    if !(format.mSampleRate.is_finite() && format.mSampleRate >= 1.0) {
        return Err(AudioError::Backend("the process tap has no sample rate yet".into()));
    }
    Ok(format)
}

/// The aggregate's sample rate, when it reports a real one.
fn aggregate_rate(aggregate: ObjectId) -> Option<f64> {
    let rate = ca::get::<f64>(aggregate, ca::global(kAudioDevicePropertyNominalSampleRate)).ok()?;
    (rate.is_finite() && rate >= 1.0).then_some(rate)
}

/// Private aggregate whose only member is the tap: its input is the tap's stream
/// and there is no output. The UID prefix lets `tap::remove_orphans` find leftovers.
fn aggregate_description(tap_uid: &str) -> CFRetained<CFDictionary<CFString, CFType>> {
    let string = |s: &str| CFRetained::<CFType>::from(&*CFString::from_str(s));
    let boolean = |b: bool| CFRetained::<CFType>::from(&**CFBoolean::new(b));
    let dict = |pairs: &[(&CStr, CFRetained<CFType>)]| {
        let keys: Vec<_> = pairs.iter().map(|(k, _)| CFString::from_str(k.to_str().unwrap_or_default())).collect();
        let keys: Vec<&CFString> = keys.iter().map(|k| &**k).collect();
        let values: Vec<&CFType> = pairs.iter().map(|(_, v)| &**v).collect();
        CFDictionary::<CFString, CFType>::from_slices(&keys, &values)
    };
    let tap = dict(&[(kAudioSubTapUIDKey, string(tap_uid)), (kAudioSubTapDriftCompensationKey, boolean(false))]);
    dict(&[
        (kAudioAggregateDeviceNameKey, string("OmniPanel share")),
        (kAudioAggregateDeviceUIDKey, string(&format!("{UID_PREFIX}{tap_uid}"))),
        (kAudioAggregateDeviceIsPrivateKey, boolean(true)),
        (kAudioAggregateDeviceTapAutoStartKey, boolean(true)),
        (
            kAudioAggregateDeviceTapListKey,
            CFRetained::<CFType>::from(&**CFArray::<CFType>::from_objects(&[tap.as_ref()])),
        ),
    ])
}

/// What the IO thread owns.
struct Client {
    sink: Sink,
    /// Scratch for the downmix.
    mono: Box<[f32]>,
    cycles: Arc<AtomicU32>,
}

impl Client {
    /// Downmixes every channel of `list` to mono and hands it to the sink.
    /// Real-time: no allocation, no locks.
    ///
    /// # Safety
    /// `list` must point to a valid `AudioBufferList` whose buffers hold `f32` samples.
    unsafe fn deliver(&mut self, list: *const AudioBufferList) {
        self.cycles.fetch_add(1, Relaxed);
        let count = unsafe { (*list).mNumberBuffers } as usize;
        // Derive from the raw list pointer: the array extends past its declared length of 1.
        let buffers = unsafe { ptr::addr_of!((*list).mBuffers).cast::<AudioBuffer>() };
        let buffer = |i: usize| unsafe { *buffers.add(i) };
        let usable = |b: &AudioBuffer| !b.mData.is_null() && b.mNumberChannels > 0;
        let (mut channels, mut frames) = (0usize, usize::MAX);
        for b in (0..count).map(buffer).filter(usable) {
            channels += b.mNumberChannels as usize;
            frames = frames.min(b.mDataByteSize as usize / 4 / b.mNumberChannels as usize);
        }
        if channels == 0 {
            return;
        }
        let scale = 1.0 / channels as f32;
        let mut done = 0;
        while done < frames {
            let mono = &mut self.mono[..(frames - done).min(PIECE)];
            mono.fill(0.0);
            for b in (0..count).map(buffer).filter(usable) {
                let width = b.mNumberChannels as usize;
                // SAFETY: `done + mono.len() <= frames`, and the buffer holds at least `frames` frames.
                let samples =
                    unsafe { std::slice::from_raw_parts(b.mData.cast::<f32>().add(done * width), mono.len() * width) };
                for (out, frame) in mono.iter_mut().zip(samples.chunks_exact(width)) {
                    *out += frame.iter().sum::<f32>();
                }
            }
            mono.iter_mut().for_each(|s| *s *= scale);
            (self.sink)(mono);
            done += mono.len();
        }
    }
}

/// Real-time callback of the tap-only aggregate: `input` is the tap's stream.
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    if let Some(client) = unsafe { client.cast::<Client>().as_mut() } {
        unsafe { client.deliver(input.as_ptr()) };
    }
    0
}

/// Who hears about changes of the list of audio processes: the newest capture.
///
/// CoreAudio cannot say when a removed listener has returned from its last
/// call, so the one listener is installed once and never removed, and it only
/// reaches a capture through this slot.
static LISTENER: Mutex<Option<(u64, Changed)>> = Mutex::new(None);

/// A capture's turn in [`LISTENER`]; dropping it ends the turn.
struct Listening(u64);

impl Listening {
    fn start(changed: Changed) -> Self {
        static INSTALL: Once = Once::new();
        static TURNS: AtomicU64 = AtomicU64::new(0);
        INSTALL.call_once(|| {
            let address = ca::global(kAudioHardwarePropertyProcessObjectList);
            let status = unsafe {
                AudioObjectAddPropertyListener(ca::SYSTEM, NonNull::from(&address), Some(on_change), ptr::null_mut())
            };
            if status != 0 {
                tracing::warn!("CoreAudio listener not installed: {}", ca::Status(status));
            }
        });
        let turn = TURNS.fetch_add(1, Relaxed);
        *LISTENER.lock().unwrap_or_else(PoisonError::into_inner) = Some((turn, changed));
        Self(turn)
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        let mut listener = LISTENER.lock().unwrap_or_else(PoisonError::into_inner);
        // A newer capture may have taken over already.
        if listener.as_ref().is_some_and(|(turn, _)| *turn == self.0) {
            *listener = None;
        }
    }
}

unsafe extern "C-unwind" fn on_change(
    _object: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    _client: *mut c_void,
) -> i32 {
    // Called after the lock is let go: `changed` may make or drop a capture.
    let changed = LISTENER.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|(_, changed)| changed.clone());
    if let Some(changed) = changed {
        changed();
    }
    0
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::macos::testing::{Tone, require_permission};

    #[repr(C)]
    struct List<const N: usize> {
        count: u32,
        buffers: [AudioBuffer; N],
    }

    fn buffer(channels: u32, data: &mut [f32]) -> AudioBuffer {
        AudioBuffer {
            mNumberChannels: channels,
            mDataByteSize: (data.len() * 4) as u32,
            mData: data.as_mut_ptr().cast(),
        }
    }

    /// Runs `deliver` on `list`; returns the pieces the sink received.
    fn deliver<const N: usize>(list: &mut List<N>) -> Vec<Vec<f32>> {
        let pieces = Arc::new(Mutex::new(Vec::new()));
        let seen = pieces.clone();
        let mut client = Client {
            sink: Box::new(move |mono| seen.lock().unwrap().push(mono.to_vec())),
            mono: vec![0.0; PIECE].into_boxed_slice(),
            cycles: Arc::default(),
        };
        unsafe { client.deliver(ptr::from_mut(list).cast()) };
        drop(client);
        Arc::try_unwrap(pieces).unwrap().into_inner().unwrap()
    }

    #[test]
    fn stereo_is_downmixed_to_mono() {
        let mut stereo: Vec<f32> = (0..8).flat_map(|i| [i as f32, 1.0]).collect();
        let mut list = List { count: 1, buffers: [buffer(2, &mut stereo)] };
        let expected: Vec<f32> = (0..8).map(|i| (i as f32 + 1.0) / 2.0).collect();
        assert_eq!(deliver(&mut list), [expected]);
    }

    #[test]
    fn planar_buffers_and_long_cycles_are_handled() {
        // Two mono buffers (non-interleaved stereo), longer than one piece; a buffer
        // without data is skipped.
        let frames = PIECE + 10;
        let (mut left, mut right) = (vec![0.5f32; frames], vec![-0.25f32; frames]);
        let mut list = List {
            count: 3,
            buffers: [
                buffer(1, &mut left),
                AudioBuffer { mNumberChannels: 2, mDataByteSize: 64, mData: ptr::null_mut() },
                buffer(1, &mut right),
            ],
        };
        let pieces = deliver(&mut list);
        assert_eq!(pieces.iter().map(Vec::len).collect::<Vec<_>>(), [PIECE, 10]);
        assert!(pieces.iter().flatten().all(|&s| s == 0.125));
    }

    /// Captures `afplay` on real hardware: its tone arrives at its own level and in
    /// real time, and afplay keeps running output. (That it is still heard on the
    /// speakers rests on the tap being made unmuted; nothing here listens to them.)
    #[test]
    #[ignore]
    fn captures_afplay_at_its_level() {
        require_permission();
        let tone = Tone::play();
        let object = tone.object();
        let peak = Arc::new(Mutex::new((0.0f32, 0usize)));
        let seen = peak.clone();
        let started = Instant::now();
        let mut rate = 0;
        let running = Running::start(vec![object], &mut |r| {
            rate = r;
            let seen = seen.clone();
            Box::new(move |mono| {
                let mut seen = seen.lock().unwrap();
                seen.0 = mono.iter().fold(seen.0, |peak, s| peak.max(s.abs()));
                seen.1 += mono.len();
            })
        })
        .unwrap();
        eprintln!("tap ready in {:?} at {rate} Hz", started.elapsed());
        let mut running = running;
        std::thread::sleep(Duration::from_secs(1));
        assert!(running.healthy());
        let (peak, frames) = *peak.lock().unwrap();
        eprintln!("peak {peak:.4}, {frames} frames in 1 s");
        // The tone is 650 / 32768 = 0.0198 on both channels.
        assert!((peak - 0.0198).abs() < 0.002, "unexpected level {peak}");
        assert!(frames as f64 > 0.8 * f64::from(rate), "the tap delivered {frames} frames");
        assert_eq!(tone.object(), object, "afplay still runs output");
        drop(running);
    }
}
