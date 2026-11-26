//! Software gain and output routing for other processes' audio: a Core Audio process
//! tap (macOS 14.2+) muting its source, played back by an IOProc that applies our
//! gain, through a private aggregate device made of an output device and the tap.
//! Used for one app's volume and output device, and for the software master volume.
//!
//! A tap takes only the audio headed to one stream of the device it listens to (the
//! default output): what an app sends to another device by itself is not tapped (and
//! not muted). Played back on that same device it lands on the same channels, so
//! only the level changes. Played back on another device, the stereo pair of the
//! tapped device moves onto the stereo pair of that one.
//!
//! Same structure as AudioCap / FineTune. Taps use `mutedWhenTapped`, so a source is
//! silenced only while our IOProc reads it: when we stop (or our process dies, taking
//! its private taps and aggregates with it) it plays natively again.

use std::ffi::{CStr, c_void};
use std::ptr::{self, NonNull};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::time::{Duration, Instant};

use objc2::AllocAnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyClass;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareDestroyAggregateDevice, AudioHardwareIOProcStreamUsage,
    AudioObjectID, AudioObjectSetPropertyData, CATapDescription, CATapMuteBehavior,
    kAudioAggregateDeviceClockDeviceKey, kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey, kAudioAggregateDeviceSubDeviceListKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey,
    kAudioDevicePropertyDeviceIsAlive, kAudioDevicePropertyIOProcStreamUsage, kAudioDevicePropertyNominalSampleRate,
    kAudioDevicePropertyPreferredChannelsForStereo, kAudioDevicePropertyStreams, kAudioDeviceTransportTypeAggregate,
    kAudioDeviceTransportTypeBluetooth, kAudioDeviceTransportTypeBluetoothLE, kAudioDeviceTransportTypeVirtual,
    kAudioHardwarePropertyDevices, kAudioStreamPropertyVirtualFormat, kAudioSubDeviceUIDKey,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, kAudioTapPropertyDescription,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsFloat,
    kAudioFormatLinearPCM,
};
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFRetained, CFString, CFType};
use objc2_foundation::{NSArray, NSNumber, NSString, NSUUID};

use super::ca::{self, MAIN, ObjectId, Status};
use super::device;
use crate::{AudioError, Endpoint, Result};

/// Gain ramp time constant; long enough to avoid zipper noise and clicks.
const SMOOTHING_SECS: f64 = 0.015;
/// UID prefix of our aggregates, used to find leftovers of a crashed run.
const UID_PREFIX: &str = "dev.omnipanel.tap.";
/// IO silence after which a tap counts as stalled (generous for start-up and Bluetooth warm-up).
const STALL: Duration = Duration::from_secs(2);

type CreateTap = unsafe extern "C-unwind" fn(*const CATapDescription, *mut AudioObjectID) -> i32;
type DestroyTap = unsafe extern "C-unwind" fn(AudioObjectID) -> i32;

/// `AudioHardwareCreateProcessTap` / `DestroyProcessTap`, resolved at runtime so the
/// binary still launches on macOS releases without taps.
struct TapApi {
    create: CreateTap,
    destroy: DestroyTap,
}

fn api() -> Option<&'static TapApi> {
    static API: OnceLock<Option<TapApi>> = OnceLock::new();
    API.get_or_init(|| {
        let create = ca::symbol(c"AudioHardwareCreateProcessTap")?;
        let destroy = ca::symbol(c"AudioHardwareDestroyProcessTap")?;
        AnyClass::get(c"CATapDescription")?;
        // SAFETY: signatures from <CoreAudio/AudioHardware.h>.
        Some(unsafe {
            TapApi {
                create: std::mem::transmute::<*mut c_void, CreateTap>(create.as_ptr()),
                destroy: std::mem::transmute::<*mut c_void, DestroyTap>(destroy.as_ptr()),
            }
        })
    })
    .as_ref()
}

/// Process taps exist on this macOS (14.2+).
pub fn supported() -> bool {
    api().is_some()
}

/// Destroys aggregates a previous, crashed run may have left behind (they would keep
/// their app muted). Private aggregates normally die with their process.
pub fn remove_orphans() {
    let Ok(devices) = ca::get_list::<u32>(ca::SYSTEM, ca::global(kAudioHardwarePropertyDevices)) else { return };
    for device in devices {
        if device::uid(device).is_ok_and(|uid| uid.starts_with(UID_PREFIX)) {
            tracing::info!("removing leftover per-app volume device {device}");
            unsafe { AudioHardwareDestroyAggregateDevice(device) };
        }
    }
}

/// State shared with the real-time IO thread: atomics only, no locks or allocation.
struct Shared {
    /// Target gain (`f32` bits), written by `Tap::set_gain`.
    target: AtomicU32,
    /// Smoothed gain, owned by the IO thread.
    current: AtomicU32,
    /// IO cycles run, to detect a stalled device.
    cycles: AtomicU32,
    /// Peak levels since the last `take_peaks` (non-negative `f32` bits, so `fetch_max` works).
    peak_in: AtomicU32,
    peak_out: AtomicU32,
    /// Largest jump between two consecutive samples of the tap's first channel since
    /// the last `take_step`, and the sample it continues from: dropouts and clock
    /// drift show up as jumps a steady tone never makes.
    peak_step: AtomicU32,
    last_sample: AtomicU32,
    /// Fixed before the IOProc starts.
    layout: Layout,
}

/// Whose audio a tap captures, of what is played to the stream it listens to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// These processes (one app).
    Processes(Vec<ObjectId>),
    /// Every process except these (software master).
    AllExcept(Vec<ObjectId>),
}

#[derive(Debug, Clone, Copy)]
struct Layout {
    /// Index of the tap's first buffer in the IOProc input list.
    tap_buffer: u32,
    mapping: Mapping,
    /// Per-frame smoothing coefficient.
    coeff: f32,
}

/// Which tap channel plays on which output channel (both 0-based; output channels
/// are counted across buffers).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mapping {
    /// Played on the tapped device: every channel of the tapped stream, back onto the
    /// channel it was headed to. `offset` is the stream's first channel.
    Straight { offset: u32 },
    /// Played on another device: the tapped device's stereo pair (as tap channels) onto
    /// that device's stereo pair. Anything else the app plays there is dropped.
    Stereo { from: [u32; 2], to: [u32; 2] },
}

impl Mapping {
    /// The `index`-th (tap channel, output channel) to play, until `None`.
    fn pair(self, index: u32) -> Option<(u32, u32)> {
        match self {
            Mapping::Straight { offset } => Some((index, offset + index)),
            // A mono output gets the left channel once, not both on top of each other.
            Mapping::Stereo { from, to } => {
                let once = index == 0 || (index == 1 && to[1] != to[0]);
                once.then(|| (from[index as usize], to[index as usize]))
            }
        }
    }
}

/// A running tap; dropping it restores the app's native output.
pub struct Tap {
    tap: ObjectId,
    tap_uid: String,
    aggregate: ObjectId,
    io_proc: AudioDeviceIOProcID,
    running: bool,
    shared: Box<Shared>,
    label: String,
    from_uid: String,
    device_uid: String,
    /// Index of the output stream of `from` the tap listens to.
    stream: u32,
    /// IO cycle count at the last health check, and when it last moved.
    seen_cycles: u32,
    seen_at: Instant,
    /// Sample rates of `device` and `from` the aggregate was built for.
    rates: (f64, f64),
    pub source: Source,
    /// Output device the tap listens to (the default output when it was built).
    pub from: ObjectId,
    /// Output device the aggregate plays to: `from`, or the device an app is sent to.
    pub device: ObjectId,
}

impl Tap {
    /// Taps what `source` plays on `from` and plays it through `device` at `gain`,
    /// fading in from silence.
    ///
    /// Blocks while macOS shows the System Audio Recording prompt, so callers check
    /// the permission first.
    pub fn new(label: &str, source: Source, from: ObjectId, device: ObjectId, gain: f32) -> Result<Self> {
        let api = api().ok_or_else(|| AudioError::Unsupported("per-app volume needs macOS 14.2 or later".into()))?;
        if [from, device].iter().any(|&d| device::transport(d) == kAudioDeviceTransportTypeAggregate) {
            // Aggregates cannot nest; flattening multi-output devices is not supported yet.
            return Err(AudioError::Unsupported(
                "per-app volume does not work with aggregate or multi-output devices".into(),
            ));
        }
        let mut tap = Tap {
            tap: 0,
            tap_uid: String::new(),
            aggregate: 0,
            io_proc: None,
            running: false,
            shared: Box::new(Shared {
                target: AtomicU32::new(gain.to_bits()),
                current: AtomicU32::new(0f32.to_bits()),
                cycles: AtomicU32::new(0),
                peak_in: AtomicU32::new(0),
                peak_out: AtomicU32::new(0),
                peak_step: AtomicU32::new(0),
                last_sample: AtomicU32::new(0),
                layout: Layout { tap_buffer: 0, mapping: Mapping::Straight { offset: 0 }, coeff: 1.0 },
            }),
            label: label.to_string(),
            from_uid: device::uid(from)?,
            stream: 0,
            seen_cycles: 0,
            seen_at: Instant::now(),
            rates: (sample_rate(device), sample_rate(from)),
            source,
            from,
            device,
            device_uid: device::uid(device)?,
        };
        // The tap listens to the stream holding the tapped device's left channel.
        let tapped = stereo_channels(from);
        let (stream, first, channels) = output_stream(from, tapped[0]);
        tap.stream = stream;
        let mapping = if from == device {
            Mapping::Straight { offset: first }
        } else {
            // A right channel outside that stream (another stream, or a mono device)
            // repeats the left.
            let left = tapped[0].saturating_sub(first);
            let right = tapped[1].checked_sub(first).filter(|&right| right < channels).unwrap_or(left);
            Mapping::Stereo { from: [left, right], to: stereo_channels(device) }
        };
        // On any error below, `Drop` releases whatever was created so far.
        let description = tap.description(None);
        tap.tap_uid = autoreleasepool(|_| unsafe { description.UUID().UUIDString() }.to_string());
        ca::check(unsafe { (api.create)(&*description, &mut tap.tap) })
            .map_err(|s| AudioError::Backend(format!("could not create the process tap (CoreAudio error {s})")))?;

        let aggregate = aggregate_description(device, &tap.device_uid, &tap.tap_uid, label, from != device);
        ca::check(unsafe {
            AudioHardwareCreateAggregateDevice(aggregate.as_ref(), NonNull::from(&mut tap.aggregate))
        })?;
        // The aggregate's inputs are the output device's own input streams, then the
        // tap. Never guess the index: reading a microphone stream instead would loop
        // it to the speakers.
        let tap_buffer = input_stream_count(device);
        if !wait_until_ready(tap.aggregate, tap_buffer + 1) {
            return Err(AudioError::Backend("the process tap did not attach to the output device".into()));
        }
        tap.shared.layout = layout(tap.aggregate, tap_buffer, mapping)?;
        ca::check(unsafe {
            AudioDeviceCreateIOProcID(
                tap.aggregate,
                Some(io_proc),
                ptr::from_ref::<Shared>(&tap.shared).cast_mut().cast(),
                NonNull::from(&mut tap.io_proc),
            )
        })?;
        tap.use_only_tap_input();
        ca::check(unsafe { AudioDeviceStart(tap.aggregate, tap.io_proc) })?;
        tap.running = true;
        Ok(tap)
    }

    /// Amplitude gain, 0.0 - 1.0; ramped smoothly by the IO thread.
    pub fn set_gain(&self, gain: f32) {
        self.shared.target.store(gain.to_bits(), Relaxed);
    }

    /// Retargets the tap (an app spawned a helper, the master's exclusions changed)
    /// without interrupting playback. The kind of source cannot change.
    pub fn set_source(&mut self, source: &Source) -> Result<()> {
        if std::mem::discriminant(source) != std::mem::discriminant(&self.source) {
            return Err(AudioError::Backend("tap source kind changed".into()));
        }
        let previous = std::mem::replace(&mut self.source, source.clone());
        let description = self.description(Some(&self.tap_uid));
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
            self.source = previous;
        }
        Ok(result?)
    }

    /// False once the aggregate died (device unplugged, coreaudiod restart), stopped
    /// calling us, or a device it uses changed sample rate (Bluetooth headset switching
    /// to call mode): all of which need a rebuild. (`DeviceIsRunning` is no signal: it
    /// lags behind `AudioDeviceStart`.)
    pub fn healthy(&mut self) -> bool {
        let flag = |selector| ca::get::<u32>(self.aggregate, ca::global(selector)).is_ok_and(|v| v != 0);
        let cycles = self.shared.cycles.load(Relaxed);
        if cycles != self.seen_cycles {
            (self.seen_cycles, self.seen_at) = (cycles, Instant::now());
        }
        flag(kAudioDevicePropertyDeviceIsAlive)
            && self.seen_at.elapsed() < STALL
            && (sample_rate(self.device), sample_rate(self.from)) == self.rates
    }

    /// Peak input (tapped app) and output levels since the last call.
    #[cfg(test)]
    pub fn take_peaks(&self) -> (f32, f32) {
        let take = |a: &AtomicU32| f32::from_bits(a.swap(0, Relaxed));
        (take(&self.shared.peak_in), take(&self.shared.peak_out))
    }

    /// Largest jump between consecutive tapped samples since the last call.
    #[cfg(test)]
    pub fn take_step(&self) -> f32 {
        f32::from_bits(self.shared.peak_step.swap(0, Relaxed))
    }

    fn description(&self, uuid: Option<&str>) -> Retained<CATapDescription> {
        autoreleasepool(|_| unsafe {
            let numbers = |objects: &[ObjectId]| {
                NSArray::from_retained_slice(&objects.iter().map(|&o| NSNumber::new_u32(o)).collect::<Vec<_>>())
            };
            // Only audio headed to this device: what an app plays elsewhere is left alone.
            let (device, stream) = (NSString::from_str(&self.from_uid), self.stream as isize);
            let description = match &self.source {
                Source::Processes(objects) => CATapDescription::initWithProcesses_andDeviceUID_withStream(
                    CATapDescription::alloc(),
                    &numbers(objects),
                    &device,
                    stream,
                ),
                Source::AllExcept(objects) => CATapDescription::initExcludingProcesses_andDeviceUID_withStream(
                    CATapDescription::alloc(),
                    &numbers(objects),
                    &device,
                    stream,
                ),
            };
            description.setName(&NSString::from_str(&format!("OmniPanel {}", self.label)));
            description.setPrivate(true);
            description.setMuteBehavior(CATapMuteBehavior::MutedWhenTapped);
            if let Some(uuid) = uuid.and_then(|u| NSUUID::initWithUUIDString(NSUUID::alloc(), &NSString::from_str(u))) {
                description.setUUID(&uuid);
            }
            description
        })
    }

    /// Marks the output device's own input streams unused for our IOProc so the HAL
    /// never starts them: no microphone use (and prompt) on duplex devices like USB
    /// interfaces, no Bluetooth headset switching to call mode.
    fn use_only_tap_input(&self) {
        let Some(io_proc) = self.io_proc else { return };
        if self.shared.layout.tap_buffer == 0 {
            return; // the output device has no inputs of its own
        }
        let streams = input_stream_count(self.aggregate);
        // AudioHardwareIOProcStreamUsage is variable-length: header + one flag per stream.
        let size = size_of::<AudioHardwareIOProcStreamUsage>() + 4 * (streams as usize - 1);
        let mut buf = vec![0u64; size.div_ceil(8)];
        let usage = buf.as_mut_ptr().cast::<AudioHardwareIOProcStreamUsage>();
        unsafe {
            (*usage).mIOProc = NonNull::new_unchecked(io_proc as *mut c_void);
            (*usage).mNumberStreams = streams;
            let flags = ptr::addr_of_mut!((*usage).mStreamIsOn).cast::<u32>();
            for i in 0..streams {
                *flags.add(i as usize) = u32::from(i >= self.shared.layout.tap_buffer);
            }
        }
        let address = ca::prop(kAudioDevicePropertyIOProcStreamUsage, device::scope(Endpoint::Input), MAIN);
        let status = unsafe {
            AudioObjectSetPropertyData(
                self.aggregate,
                NonNull::from(&address),
                0,
                ptr::null(),
                size as u32,
                NonNull::new_unchecked(usage.cast()),
            )
        };
        if status != 0 {
            tracing::warn!("tap {}: could not disable device inputs ({})", self.label, Status(status));
        }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        // Order matters: once AudioDeviceStop / DestroyIOProcID return, the IO thread
        // no longer touches `shared`, so the box can be freed with `self`.
        unsafe {
            if self.running {
                AudioDeviceStop(self.aggregate, self.io_proc);
            }
            if self.io_proc.is_some() {
                AudioDeviceDestroyIOProcID(self.aggregate, self.io_proc);
            }
            if self.aggregate != 0 {
                AudioHardwareDestroyAggregateDevice(self.aggregate);
            }
            if self.tap != 0
                && let Some(api) = api()
            {
                (api.destroy)(self.tap);
            }
        }
    }
}

fn sample_rate(device: ObjectId) -> f64 {
    ca::get::<f64>(device, ca::global(kAudioDevicePropertyNominalSampleRate)).unwrap_or(0.0)
}

fn input_stream_count(device: ObjectId) -> u32 {
    let address = ca::prop(kAudioDevicePropertyStreams, device::scope(Endpoint::Input), MAIN);
    ca::get_list::<u32>(device, address).map_or(0, |s| s.len() as u32)
}

/// A fresh aggregate can take a moment to come alive with all its input streams.
fn wait_until_ready(aggregate: ObjectId, inputs: u32) -> bool {
    let ready = || {
        ca::get::<u32>(aggregate, ca::global(kAudioDevicePropertyDeviceIsAlive)).is_ok_and(|v| v != 0)
            && input_stream_count(aggregate) == inputs
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    while !ready() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

/// The device's preferred left and right output channels (0-based, counted across
/// its streams).
fn stereo_channels(device: ObjectId) -> [u32; 2] {
    let address = ca::prop(kAudioDevicePropertyPreferredChannelsForStereo, device::scope(Endpoint::Output), MAIN);
    ca::get::<[u32; 2]>(device, address).unwrap_or([1, 2]).map(|channel| channel.saturating_sub(1))
}

/// The output stream holding the device's channel `channel` (0-based): its index,
/// first channel and channel count.
fn output_stream(device: ObjectId, channel: u32) -> (u32, u32, u32) {
    let output = device::scope(Endpoint::Output);
    let streams = ca::get_list::<u32>(device, ca::prop(kAudioDevicePropertyStreams, output, MAIN)).unwrap_or_default();
    let mut first = 0;
    for (index, &stream) in streams.iter().enumerate() {
        let channels = ca::get::<AudioStreamBasicDescription>(stream, ca::global(kAudioStreamPropertyVirtualFormat))
            .map_or(0, |f| f.mChannelsPerFrame);
        if channel < first + channels {
            return (index as u32, first, channels);
        }
        first += channels;
    }
    (0, 0, 0)
}

/// Checks the output format and fixes how the IOProc maps the tap.
fn layout(aggregate: ObjectId, tap_buffer: u32, mapping: Mapping) -> Result<Layout> {
    let output = device::scope(Endpoint::Output);
    let streams = ca::get_list::<u32>(aggregate, ca::prop(kAudioDevicePropertyStreams, output, MAIN))?;
    if streams.is_empty() {
        return Err(AudioError::Backend("output device has no output streams".into()));
    }
    for &stream in &streams {
        let format = ca::get::<AudioStreamBasicDescription>(stream, ca::global(kAudioStreamPropertyVirtualFormat))?;
        if format.mFormatID != kAudioFormatLinearPCM
            || format.mFormatFlags & kAudioFormatFlagIsFloat == 0
            || format.mBitsPerChannel != 32
        {
            return Err(AudioError::Unsupported("output device does not use a float format".into()));
        }
    }
    let rate = sample_rate(aggregate).max(1.0);
    Ok(Layout { tap_buffer, mapping, coeff: (1.0 - (-1.0 / (SMOOTHING_SECS * rate)).exp()) as f32 })
}

/// Private aggregate: `device` is the main (clock) sub-device, the tap its input.
/// `rerouted`: the tap listens to another device than `device`.
fn aggregate_description(
    device: ObjectId,
    output_uid: &str,
    tap_uid: &str,
    label: &str,
    rerouted: bool,
) -> CFRetained<CFDictionary<CFString, CFType>> {
    // Bluetooth and virtual outputs share the tap's clock; drift compensation there
    // only adds periodic crackle (FineTune's finding). A tap of another device never
    // shares the output's clock.
    let drift = rerouted
        || ![
            kAudioDeviceTransportTypeBluetooth,
            kAudioDeviceTransportTypeBluetoothLE,
            kAudioDeviceTransportTypeVirtual,
        ]
        .contains(&device::transport(device));

    let string = |s: &str| CFRetained::<CFType>::from(&*CFString::from_str(s));
    let boolean = |b: bool| CFRetained::<CFType>::from(&**CFBoolean::new(b));
    let dict = |pairs: &[(&CStr, CFRetained<CFType>)]| {
        let keys: Vec<_> = pairs.iter().map(|(k, _)| CFString::from_str(k.to_str().unwrap_or_default())).collect();
        let keys: Vec<&CFString> = keys.iter().map(|k| &**k).collect();
        let values: Vec<&CFType> = pairs.iter().map(|(_, v)| &**v).collect();
        CFDictionary::<CFString, CFType>::from_slices(&keys, &values)
    };
    let list = |item: CFRetained<CFDictionary<CFString, CFType>>| {
        CFRetained::<CFType>::from(&**CFArray::<CFType>::from_objects(&[item.as_ref()]))
    };
    dict(&[
        (kAudioAggregateDeviceNameKey, string(&format!("OmniPanel {label}"))),
        (kAudioAggregateDeviceUIDKey, string(&format!("{UID_PREFIX}{tap_uid}"))),
        (kAudioAggregateDeviceMainSubDeviceKey, string(output_uid)),
        (kAudioAggregateDeviceClockDeviceKey, string(output_uid)),
        (kAudioAggregateDeviceIsPrivateKey, boolean(true)),
        (kAudioAggregateDeviceIsStackedKey, boolean(false)),
        (kAudioAggregateDeviceTapAutoStartKey, boolean(true)),
        (kAudioAggregateDeviceSubDeviceListKey, list(dict(&[(kAudioSubDeviceUIDKey, string(output_uid))]))),
        (
            kAudioAggregateDeviceTapListKey,
            list(dict(&[(kAudioSubTapUIDKey, string(tap_uid)), (kAudioSubTapDriftCompensationKey, boolean(drift))])),
        ),
    ])
}

/// One channel inside an `AudioBufferList`: sample `i` is at `base + i * stride`.
#[derive(Clone, Copy)]
struct Channel {
    base: *mut f32,
    stride: usize,
    frames: usize,
}

/// Finds channel `index`, counting channels across the list's buffers from `first_buffer`.
///
/// # Safety
/// `list` must point to a valid `AudioBufferList` whose buffers hold `f32` samples.
unsafe fn channel(list: *const AudioBufferList, first_buffer: u32, index: u32) -> Option<Channel> {
    let count = unsafe { (*list).mNumberBuffers };
    // Derive from the raw list pointer: the array extends past its declared length of 1.
    let buffers = unsafe { ptr::addr_of!((*list).mBuffers).cast::<AudioBuffer>() };
    let mut index = index;
    for i in first_buffer..count {
        let buffer = unsafe { *buffers.add(i as usize) };
        let channels = buffer.mNumberChannels;
        if index < channels {
            return (!buffer.mData.is_null()).then(|| Channel {
                base: unsafe { buffer.mData.cast::<f32>().add(index as usize) },
                stride: channels as usize,
                frames: buffer.mDataByteSize as usize / 4 / channels as usize,
            });
        }
        index -= channels;
    }
    None
}

/// Real-time callback: adds the tap's channels onto the output channels `Mapping`
/// gives them, with a smoothed gain. Must not allocate, lock, or panic.
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    let Some(shared) = (unsafe { client.cast::<Shared>().as_ref() }) else { return 0 };
    shared.cycles.fetch_add(1, Relaxed);
    let layout = shared.layout;
    let (input, output) = (input.as_ptr(), output.as_ptr());

    unsafe {
        let buffers = ptr::addr_of_mut!((*output).mBuffers).cast::<AudioBuffer>();
        for i in 0..(*output).mNumberBuffers as usize {
            let buffer = *buffers.add(i);
            if !buffer.mData.is_null() {
                ptr::write_bytes(buffer.mData.cast::<u8>(), 0, buffer.mDataByteSize as usize);
            }
        }
    }

    let target = f32::from_bits(shared.target.load(Relaxed));
    let start = f32::from_bits(shared.current.load(Relaxed));
    let mut gain = start;
    let (mut peak_in, mut peak_out) = (0f32, 0f32);
    let mut last = f32::from_bits(shared.last_sample.load(Relaxed));
    let mut peak_step = 0f32;
    // Channel by channel, each replaying the same gain ramp: any channel count works
    // without per-channel state.
    let mut index = 0;
    while let Some((tapped, played)) = layout.mapping.pair(index)
        && let (Some(from), Some(to)) =
            unsafe { (channel(input, layout.tap_buffer, tapped), channel(output, 0, played)) }
    {
        gain = start;
        for i in 0..from.frames.min(to.frames) {
            gain += (target - gain) * layout.coeff;
            unsafe {
                let sample = *from.base.add(i * from.stride);
                let out = to.base.add(i * to.stride);
                *out += sample * gain;
                peak_in = peak_in.max(sample.abs());
                peak_out = peak_out.max((*out).abs());
                if index == 0 {
                    peak_step = peak_step.max((sample - last).abs());
                    last = sample;
                }
            }
        }
        index += 1;
    }
    if (target - gain).abs() < 1e-4 {
        gain = target;
    }
    shared.current.store(gain.to_bits(), Relaxed);
    shared.peak_in.fetch_max(peak_in.to_bits(), Relaxed);
    shared.peak_out.fetch_max(peak_out.to_bits(), Relaxed);
    shared.peak_step.fetch_max(peak_step.to_bits(), Relaxed);
    shared.last_sample.store(last.to_bits(), Relaxed);
    0
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::macos::testing::{Tone, cpu_time, require_permission};

    fn peaks_after(tap: &Tap, wait: Duration) -> (f32, f32) {
        std::thread::sleep(Duration::from_millis(150)); // let the gain ramp settle
        tap.take_peaks();
        std::thread::sleep(wait);
        tap.take_peaks()
    }

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

    fn shared(gain: f32, layout: Layout) -> Shared {
        let atomic = |v: f32| AtomicU32::new(v.to_bits());
        Shared {
            target: atomic(gain),
            current: atomic(0.0),
            cycles: AtomicU32::new(0),
            peak_in: atomic(0.0),
            peak_out: atomic(0.0),
            peak_step: atomic(0.0),
            last_sample: atomic(0.0),
            layout,
        }
    }

    fn run<const I: usize, const O: usize>(shared: &Shared, input: &mut List<I>, output: &mut List<O>) {
        let time = unsafe { std::mem::zeroed::<AudioTimeStamp>() };
        let time = NonNull::from(&time);
        let client = ptr::from_ref(shared).cast_mut().cast();
        unsafe { io_proc(0, time, NonNull::from(input).cast(), time, NonNull::from(output).cast(), time, client) };
    }

    /// The IOProc skips the device's own (disabled) input, plays the tapped stream
    /// back channel for channel from its first output channel, and ramps the gain
    /// instead of jumping.
    #[test]
    fn io_proc_maps_tap_to_output_and_ramps() {
        let mut tap: Vec<f32> = (0..8).flat_map(|_| [0.5, -0.25]).collect(); // 8 stereo frames
        let mut out = vec![9.0f32; 8 * 4]; // 4-channel interface, tapped stream on channels 3+4
        let mut input = List {
            count: 2,
            buffers: [
                AudioBuffer { mNumberChannels: 1, mDataByteSize: 32, mData: ptr::null_mut() },
                buffer(2, &mut tap),
            ],
        };
        let mut output = List { count: 1, buffers: [buffer(4, &mut out)] };

        let instant = Layout { tap_buffer: 1, mapping: Mapping::Straight { offset: 2 }, coeff: 1.0 };
        run(&shared(0.5, instant), &mut input, &mut output);
        for frame in out.chunks(4) {
            assert_eq!(frame, [0.0, 0.0, 0.25, -0.125]);
        }

        let smooth = shared(1.0, Layout { coeff: 0.25, ..instant });
        run(&smooth, &mut input, &mut output);
        let left: Vec<f32> = out.chunks(4).map(|f| f[2]).collect();
        assert_eq!(left[0], 0.5 * 0.25, "starts from silence");
        assert!(left.windows(2).all(|w| w[1] > w[0] && w[1] <= 0.5), "rises smoothly: {left:?}");
        assert!(out.chunks(4).all(|f| f[3] == -0.5 * f[2]), "channels share the ramp");
        assert_eq!(f32::from_bits(smooth.current.load(Relaxed)), left[7] * 2.0, "the ramp carries over");
        assert_eq!(f32::from_bits(smooth.peak_in.load(Relaxed)), 0.5);
    }

    /// Every channel of a wide stream gets the gain: none is left silent while its
    /// source is muted by the tap.
    #[test]
    fn io_proc_plays_every_channel_of_wide_streams() {
        const CHANNELS: u32 = 32;
        let mut tap = vec![0.5f32; 4 * CHANNELS as usize];
        let mut out = vec![9.0f32; 4 * CHANNELS as usize];
        let mut input = List { count: 1, buffers: [buffer(CHANNELS, &mut tap)] };
        let mut output = List { count: 1, buffers: [buffer(CHANNELS, &mut out)] };
        let straight = Layout { tap_buffer: 0, mapping: Mapping::Straight { offset: 0 }, coeff: 1.0 };
        run(&shared(0.5, straight), &mut input, &mut output);
        assert!(out.iter().all(|&s| s == 0.25), "{out:?}");
    }

    /// Sent to another device, the tapped device's stereo pair lands on that device's
    /// stereo pair, wherever either sits: here channels 3+4 of a 4-channel interface
    /// onto channels 5+6 of a 6-channel one. A mono output gets the left channel once.
    #[test]
    fn io_proc_moves_the_stereo_pair_to_another_device() {
        let mut tap: Vec<f32> = (0..4).flat_map(|_| [0.9, 0.9, 0.5, -0.25]).collect(); // 4 frames, 4 channels
        let mut out = vec![9.0f32; 4 * 6];
        let mut input = List { count: 1, buffers: [buffer(4, &mut tap)] };
        let mut output = List { count: 1, buffers: [buffer(6, &mut out)] };
        let mapping = Mapping::Stereo { from: [2, 3], to: [4, 5] };
        run(&shared(1.0, Layout { tap_buffer: 0, mapping, coeff: 1.0 }), &mut input, &mut output);
        for frame in out.chunks(6) {
            assert_eq!(frame, [0.0, 0.0, 0.0, 0.0, 0.5, -0.25]);
        }

        let mut mono = vec![9.0f32; 4];
        let mut output = List { count: 1, buffers: [buffer(1, &mut mono)] };
        let mapping = Mapping::Stereo { from: [2, 3], to: [0, 0] };
        run(&shared(1.0, Layout { tap_buffer: 0, mapping, coeff: 1.0 }), &mut input, &mut output);
        assert_eq!(mono, [0.5; 4]);
    }

    /// Taps `afplay` and checks the output follows the gain, on real hardware.
    #[test]
    #[ignore]
    fn tap_applies_gain_to_afplay() {
        require_permission();
        let tone = Tone::play();
        let object = tone.object();
        let device = device::default_device(Endpoint::Output).unwrap();
        let started = Instant::now();
        let mut tap = Tap::new("test", Source::Processes(vec![object]), device, device, 0.0).unwrap();
        eprintln!("tap ready in {:?}, layout {:?}", started.elapsed(), tap.shared.layout);

        let (input, output) = peaks_after(&tap, Duration::from_millis(500));
        eprintln!("gain 0: in {input:.4} out {output:.4}");
        assert!(input > 0.005, "the tap delivers silence");
        assert_eq!(output, 0.0);

        for gain in [0.125, 1.0] {
            tap.set_gain(gain);
            let cpu = cpu_time();
            let (input, output) = peaks_after(&tap, Duration::from_secs(1));
            let cpu = (cpu_time() - cpu).as_secs_f64() / 1.15 * 100.0;
            eprintln!("gain {gain}: in {input:.4} out {output:.4}, process cpu {cpu:.2}%");
            assert!((output / input - gain).abs() < 0.02, "output does not follow gain {gain}");
        }

        // Retarget in place (same process set, exercising the property write).
        tap.set_source(&Source::Processes(vec![object])).unwrap();
        assert!(tap.healthy());
        let started = Instant::now();
        drop(tap);
        eprintln!("teardown in {:?}", started.elapsed());
    }
}
