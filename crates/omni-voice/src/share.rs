//! App sound shared into the voice output, so a call hears what the user hears.
//!
//! [`AppCapture`] (one implementation per OS) records the chosen apps without
//! changing how they play and hands over one [`Feed`] per captured stream. Its
//! `sync` also says which of the apps have nothing to record right now. The
//! microphone callback owns the [`Mixer`], which adds every feed onto the
//! processed voice on its way to the voice output. Effects stay on the voice,
//! and the monitor gets none of it: the user already hears the apps.
//!
//! Each feed is a lock-free ring read through a [`Reader`], so a capture running
//! on another clock (or rate) than the microphone neither drifts nor glitches.
//! Sound servers hand out one period as several callbacks in a row, on both
//! ends; [`Burst`] sizes the buffering for the whole period.

// Without a capture (Windows for now) only the tests feed the mixer.
#![cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::audio::RING_CAPACITY_SECS;
use crate::dsp::limit;
use crate::reader::Reader;

#[cfg(target_os = "linux")]
#[path = "share/linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "share/macos.rs"]
mod platform;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[path = "share/unsupported.rs"]
mod platform;

pub(crate) use platform::{AppCapture, unsupported};

/// Streams mixed at once (an app can play several). Further ones stay unheard.
pub(crate) const MAX_FEEDS: usize = 16;
/// Mixer slots: a feed that just ended keeps its slot until it has played out,
/// which must not cost the stream replacing it a place.
const SLOTS: usize = 2 * MAX_FEEDS;

/// How much audio one end of a ring moves at once. PipeWire delivers a 1024-frame
/// period as four 5 ms callbacks back to back: buffering for 5 ms would run dry
/// inside every period, so callbacks that arrive much faster than real time
/// count as one block.
///
/// Only a block that keeps coming is a period. When the server stalls (PipeWire
/// relinking its graph because some stream started) it then flushes the backlog
/// in one long run of callbacks; buffering for that would add its whole length
/// as delay for nothing. So a block counts for at most twice the two before it.
struct Burst {
    rate: f64,
    /// Frames of the block so far, of its latest callback, and when that came.
    frames: usize,
    latest: usize,
    at: Option<Instant>,
    /// What the two blocks before this one counted for, and this one so far.
    before: [usize; 2],
    counted: usize,
}

impl Burst {
    fn new(rate: u32) -> Self {
        Self { rate: f64::from(rate), frames: 0, latest: 0, at: None, before: [0; 2], counted: 0 }
    }

    /// Size, for buffering, of the block that the callback of `frames` frames at
    /// `now` belongs to.
    fn grow(&mut self, now: Instant, frames: usize) -> usize {
        // Real time puts a callback's own length between two of them; a quarter of
        // that still tells delivery in pieces from scheduling jitter.
        let together = self
            .at
            .is_some_and(|at| now.saturating_duration_since(at).as_secs_f64() * self.rate * 4.0 < self.latest as f64);
        if together {
            self.frames += frames;
        } else {
            self.before = [self.before[1], self.counted];
            self.frames = frames;
        }
        self.latest = frames;
        self.at = Some(now);
        self.counted = self.frames.min(2 * self.before[0].max(self.before[1])).max(frames);
        self.counted
    }
}

/// Writing end of a feed, owned by a capture callback. Dropping it ends the feed
/// once the mixer has played what is left.
pub(crate) struct FeedWriter {
    tx: Producer<f32>,
    block: Arc<AtomicUsize>,
    burst: Burst,
}

impl FeedWriter {
    /// Push one capture callback of `frames` mono samples. Realtime-safe; samples
    /// that do not fit (the mixer stalled) are dropped.
    pub fn write(&mut self, frames: usize, samples: impl Iterator<Item = f32>) {
        self.write_at(Instant::now(), frames, samples);
    }

    /// [`Self::write`] for a callback that came at `now`.
    fn write_at(&mut self, now: Instant, frames: usize, samples: impl Iterator<Item = f32>) {
        if frames == 0 {
            return;
        }
        self.block.store(self.burst.grow(now, frames), Ordering::Relaxed);
        if let Ok(chunk) = self.tx.write_chunk_uninit(frames.min(self.tx.slots())) {
            // Like the microphone's: a NaN would stay in the resampler and the mix for good.
            chunk.fill_from_iter(samples.map(|s| if s.is_finite() { s } else { 0.0 }));
        }
    }
}

/// Reading end of one captured stream, resampled to the microphone's rate.
pub(crate) struct Feed(Reader);

/// A feed from a capture at `capture_rate` into a mix at `mix_rate`.
pub(crate) fn feed(capture_rate: u32, mix_rate: u32) -> (FeedWriter, Feed) {
    let (tx, rx) = RingBuffer::new((f64::from(capture_rate) * RING_CAPACITY_SECS) as usize);
    let block = Arc::new(AtomicUsize::new(0));
    let writer = FeedWriter { tx, block: block.clone(), burst: Burst::new(capture_rate) };
    (writer, Feed(Reader::new(rx, capture_rate, mix_rate, block)))
}

/// Adds the feeds onto the voice. Lives in the microphone callback: no locks, no
/// allocation, and feeds are freed on the audio thread (through [`MixerPort`]).
pub(crate) struct Mixer {
    feeds: [Option<Feed>; SLOTS],
    incoming: Consumer<Feed>,
    retired: Producer<Feed>,
    /// Microphone callbacks, which come in bursts like the captures'.
    burst: Burst,
    /// Gain while mixing a callback, its per-frame change, and where it ends (taken
    /// over exactly at the next callback: summed up steps do not land on it).
    gain: f32,
    step: f32,
    target: f32,
    /// [`Self::flush`] was asked for; done at the start of the next callback.
    flush: bool,
    /// The feeds' sum for one piece of a callback.
    apps: Box<[f32]>,
}

/// The audio thread's side of a [`Mixer`].
pub(crate) struct MixerPort {
    feeds: Producer<Feed>,
    retired: Consumer<Feed>,
}

/// A mixer for a microphone running at `rate`.
pub(crate) fn mixer(rate: u32) -> (Mixer, MixerPort) {
    let (feeds, incoming) = RingBuffer::new(SLOTS);
    let (retired_tx, retired) = RingBuffer::new(2 * SLOTS);
    let mixer = Mixer {
        feeds: [const { None }; SLOTS],
        incoming,
        retired: retired_tx,
        burst: Burst::new(rate),
        gain: 0.0,
        step: 0.0,
        target: 0.0,
        flush: false,
        apps: vec![0.0; 1024].into_boxed_slice(),
    };
    (mixer, MixerPort { feeds, retired })
}

impl MixerPort {
    /// Hand a feed to the mixer; false when it has not taken the previous ones yet.
    pub fn add(&mut self, feed: Feed) -> bool {
        self.drop_retired();
        self.feeds.push(feed).is_ok()
    }

    /// Free the feeds the mixer is done with.
    pub fn drop_retired(&mut self) {
        while self.retired.pop().is_ok() {}
    }
}

impl Mixer {
    /// Start the callback of `frames` frames that came at `now`, mixed at `gain`
    /// (ramped from the previous callback's): take new feeds, let go of ended
    /// ones. A `gain` of zero means sharing is shut: once faded out, what the
    /// feeds hold is thrown away, so nothing from before is heard when it opens
    /// again. False = nothing to add.
    pub fn begin(&mut self, now: Instant, frames: usize, gain: f32) -> bool {
        for slot in &mut self.feeds {
            // Freed by the audio thread, never here: if it is not taking feeds back
            // right now, the ended one keeps its slot until it does.
            if slot.as_ref().is_some_and(|feed| feed.0.finished())
                && let Some(feed) = slot.take()
                && let Err(rtrb::PushError::Full(feed)) = self.retired.push(feed)
            {
                *slot = Some(feed);
            }
        }
        // New feeds wait in their queue while every slot is taken. One whose stream
        // ended before it got here is never played: it goes back with the next round.
        for slot in self.feeds.iter_mut().filter(|slot| slot.is_none()) {
            match self.incoming.pop() {
                Ok(mut feed) => {
                    if feed.0.ended() {
                        feed.0.flush();
                    }
                    *slot = Some(feed);
                }
                Err(_) => break,
            }
        }
        self.gain = std::mem::replace(&mut self.target, gain);
        self.step = (gain - self.gain) / frames.max(1) as f32;
        let block = self.burst.grow(now, frames);
        let mut any = false;
        let flush = std::mem::take(&mut self.flush) || (gain == 0.0 && self.gain == 0.0);
        for feed in self.feeds.iter_mut().flatten() {
            if flush {
                feed.0.flush();
            }
            feed.0.begin(frames, block);
            any = true;
        }
        if !any {
            self.gain = gain;
        }
        any
    }

    /// Throw away what every feed holds, at the start of the next callback (so it
    /// includes the feeds still on their way in). For when an app stops being
    /// shared: what was captured of it so far must not be heard any more.
    pub fn flush(&mut self) {
        self.flush = true;
    }

    /// Add the callback's next `buf.len()` frames of every feed onto `buf`. Where an
    /// app sounds, the sum is limited like the voice so it cannot clip; the voice
    /// alone stays as it is. Only after [`Self::begin`] said true.
    pub fn add(&mut self, buf: &mut [f32]) {
        for voice in buf.chunks_mut(self.apps.len()) {
            let (gain, step) = (self.gain, self.step);
            let apps = &mut self.apps[..voice.len()];
            apps.fill(0.0);
            for feed in self.feeds.iter_mut().flatten() {
                feed.0.fill(apps.len(), |i, sample| apps[i] += sample * (gain + step * (i + 1) as f32));
            }
            self.gain += step * voice.len() as f32;
            for (voice, app) in voice.iter_mut().zip(apps.iter()) {
                if *app != 0.0 {
                    *voice = limit(*voice + app);
                }
            }
        }
    }
}

/// Whether `query` is exactly the app name `name`, ignoring case. Stricter than
/// per-app volume, which also takes ids and parts of names: a share must never
/// move to another app ("Discord PTB" once Discord quits, or the apps of a
/// binary that is called like an app that just closed).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn same(query: &str, name: &str) -> bool {
    let query = query.trim();
    !query.is_empty() && query.to_lowercase() == name.to_lowercase()
}

/// Identifies a list of shared apps (0 = none). The mix is only open while the
/// list the host committed and the one the capture was last lined up with have
/// the same key, see [`crate::VoiceEngine::commit_shares`].
pub(crate) fn key(apps: &[String]) -> u64 {
    if apps.is_empty() {
        return 0;
    }
    let mut hasher = std::hash::DefaultHasher::new();
    apps.hash(&mut hasher);
    hasher.finish() | 1
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A clock for callbacks of `block` frames at 48 kHz, each in `pieces` pieces
    /// that arrive back to back (10 us apart).
    fn clock(block: usize, pieces: usize) -> impl FnMut() -> Instant {
        let (start, mut n) = (Instant::now(), 0);
        move || {
            let at = start
                + Duration::from_secs_f64((n / pieces * block) as f64 / 48_000.0)
                + Duration::from_micros((n % pieces * 10) as u64);
            n += 1;
            at
        }
    }

    /// Feeds a 0.5 DC "app" through the mixer onto a silent voice for `callbacks`
    /// periods of `block` frames. Both ends get a period as `pieces` callbacks in a
    /// row, the microphone's all before the app's. Returns the mixed samples.
    fn mix(block: usize, pieces: usize, gain: f32, callbacks: usize) -> Vec<f32> {
        let (mut mixer, mut port) = mixer(48_000);
        let (mut writer, feed) = feed(48_000, 48_000);
        assert!(port.add(feed));
        let (mut mic_clock, mut app_clock) = (clock(block, pieces), clock(block, pieces));
        let piece = block / pieces;
        let mut out = Vec::new();
        for _ in 0..callbacks {
            for _ in 0..pieces {
                let mut buf = vec![0.0; piece];
                if mixer.begin(mic_clock(), piece, gain) {
                    // The microphone callback mixes in pieces of its scratch buffer.
                    buf.chunks_mut(1024).for_each(|part| mixer.add(part));
                }
                out.extend(buf);
            }
            for _ in 0..pieces {
                writer.write_at(app_clock(), piece, std::iter::repeat_n(0.5, piece));
            }
        }
        out
    }

    #[test]
    fn feed_reaches_the_mix_at_the_share_gain() {
        let out = mix(480, 1, 0.5, 50);
        assert!(out[..480].iter().all(|&s| s == 0.0), "silent until the feed is primed");
        let tail = &out[out.len() - 4800..];
        assert!(tail.iter().all(|s| (s - 0.25).abs() < 1e-3), "{:?}", &tail[..4]);
    }

    #[test]
    fn callbacks_larger_than_the_scratch_buffer_play_without_gaps() {
        // A 4096-frame callback is mixed in 1024-frame parts and must still be seen
        // as one, or the feed would run dry inside every callback.
        let out = mix(4096, 1, 1.0, 40);
        let tail = &out[out.len() - 8 * 4096..];
        assert!(tail.iter().all(|s| (s - 0.5).abs() < 1e-3));
    }

    #[test]
    fn periods_delivered_in_pieces_play_without_gaps() {
        // PipeWire: a 960-frame period as four 5 ms callbacks back to back, on the
        // microphone and on the app capture. Buffering for 5 ms would drop out.
        let out = mix(960, 4, 1.0, 200);
        let tail = &out[out.len() - 100 * 960..];
        let gaps = tail.iter().filter(|s| (**s - 0.5).abs() > 1e-3).count();
        assert_eq!(gaps, 0, "dropouts in {} samples", tail.len());
    }

    #[test]
    fn only_recurring_runs_of_callbacks_are_a_block() {
        let mut burst = Burst::new(48_000);
        let start = Instant::now();
        let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.0);
        // `count` callbacks in a row from `ms`, `gap` ms apart: the largest block reported.
        let run = |burst: &mut Burst, ms: f64, count: u32, gap: f64| {
            (0..count).map(|i| burst.grow(at(ms + gap * f64::from(i)), 240)).max()
        };
        // Real time, with scheduling jitter (3 ms late, then 2 ms after that): single callbacks.
        for ms in [0.0, 5.0, 13.0, 15.0] {
            assert_eq!(burst.grow(at(ms), 240), 240);
        }
        // Periods of four callbacks in a row: one block, from the second period on.
        assert_eq!(run(&mut burst, 40.0, 4, 0.02), Some(480));
        assert_eq!(run(&mut burst, 60.0, 4, 0.02), Some(960));
        assert_eq!(run(&mut burst, 80.0, 4, 0.02), Some(960));
        // A stall, then a second of backlog flushed in 10 ms: not a period to buffer for.
        assert_eq!(run(&mut burst, 1100.0, 200, 0.05), Some(1920));
        assert_eq!(run(&mut burst, 1120.0, 4, 0.02), Some(960));
        // Periods that really got longer are followed within a few of them.
        assert_eq!(run(&mut burst, 1200.0, 32, 0.02), Some(3840));
        assert_eq!(run(&mut burst, 1400.0, 32, 0.02), Some(7680));
    }

    #[test]
    fn sum_is_limited_and_gain_changes_ramp() {
        let (mut mixer, mut port) = mixer(48_000);
        let (mut writer, feed) = feed(48_000, 48_000);
        assert!(port.add(feed));
        let (mut mic_clock, mut app_clock) = (clock(480, 1), clock(480, 1));
        let mut last = Vec::new();
        for i in 0..50 {
            writer.write_at(app_clock(), 480, std::iter::repeat_n(0.9, 480));
            let mut voice = vec![0.3; 480];
            // The gain steps from 0.25 to 1 at callback 40.
            if mixer.begin(mic_clock(), 480, if i < 40 { 0.25 } else { 1.0 }) {
                mixer.add(&mut voice);
            }
            if i == 40 {
                assert!(voice.windows(2).all(|w| w[1] >= w[0]), "gain ramps inside the callback");
                assert!(voice[0] < 0.53 && voice[479] > 0.95);
            }
            last = voice;
        }
        assert!(last.iter().all(|&s| s > 0.95 && s <= 1.0), "0.3 + 0.9 stays below full scale: {}", last[0]);

        // While the app is silent the voice is left alone (it is limited already).
        let mut voice = vec![0.9; 480];
        writer.write_at(app_clock(), 480, std::iter::repeat_n(0.0, 480));
        for _ in 0..10 {
            voice.fill(0.9);
            writer.write_at(app_clock(), 480, std::iter::repeat_n(0.0, 480));
            if mixer.begin(mic_clock(), 480, 1.0) {
                mixer.add(&mut voice);
            }
        }
        assert!(voice.iter().all(|&s| s == 0.9));
    }

    #[test]
    fn nothing_from_before_is_heard_when_sharing_opens_again() {
        let (mut mixer, mut port) = mixer(48_000);
        let (mut writer, feed) = feed(48_000, 48_000);
        assert!(port.add(feed));
        let (mut mic_clock, mut app_clock) = (clock(480, 1), clock(480, 1));
        let mut run = |app: f32, gain: f32, callbacks: usize| {
            let mut out = Vec::new();
            for _ in 0..callbacks {
                writer.write_at(app_clock(), 480, std::iter::repeat_n(app, 480));
                let mut buf = vec![0.0; 480];
                if mixer.begin(mic_clock(), 480, gain) {
                    mixer.add(&mut buf);
                }
                out.extend(buf);
            }
            out
        };
        assert!(run(0.7, 1.0, 20).ends_with(&[0.7]));
        // Shut: faded out within one callback, then silent while the app plays on.
        let shut = run(0.7, 0.0, 10);
        assert!(shut[480..].iter().all(|&s| s == 0.0));
        // Open again with the app now at 0.4: the 0.7 from before never comes out.
        let open = run(0.4, 1.0, 20);
        assert!(open.iter().all(|&s| s < 0.41), "max {}", open.iter().fold(0.0f32, |m, s| m.max(*s)));
        assert!(open.ends_with(&[0.4]));
    }

    #[test]
    fn an_app_that_stops_being_shared_is_cut_at_once() {
        // Two apps; in callbacks of 1323 frames, whose 1024-frame pieces do not sum
        // up to the gain exactly.
        let (mut mixer, mut port) = mixer(48_000);
        let (mut spotify, feed_a) = feed(48_000, 48_000);
        let (mut firefox, feed_b) = feed(48_000, 48_000);
        assert!(port.add(feed_a) && port.add(feed_b));
        let (mut mic_clock, mut app_clock) = (clock(1323, 1), clock(1323, 1));
        let mut callback = |mixer: &mut Mixer, gain: f32| {
            let mut buf = vec![0.0; 1323];
            if mixer.begin(mic_clock(), 1323, gain) {
                buf.chunks_mut(1024).for_each(|part| mixer.add(part));
            }
            buf
        };
        let mut heard = Vec::new();
        for _ in 0..20 {
            let at = app_clock();
            spotify.write_at(at, 1323, std::iter::repeat_n(0.5, 1323));
            firefox.write_at(at, 1323, std::iter::repeat_n(0.2, 1323));
            heard = callback(&mut mixer, 0.7);
        }
        assert!(heard.iter().all(|s| (s - 0.49).abs() < 1e-3), "both apps at the share level: {}", heard[0]);

        // Spotify is unshared between two callbacks: its capture ends, the mixer is flushed.
        drop(spotify);
        mixer.flush();
        for _ in 0..20 {
            firefox.write_at(app_clock(), 1323, std::iter::repeat_n(0.2, 1323));
            heard = callback(&mut mixer, 0.7);
            assert!(
                heard.iter().all(|&s| s < 0.141),
                "nothing of Spotify is left: {:?}",
                heard.iter().find(|s| **s >= 0.141)
            );
        }
        assert!(heard.ends_with(&[0.2 * 0.7]));

        // A feed that was still on its way in when its app was unshared is not heard either.
        let (mut discord, feed_c) = feed(48_000, 48_000);
        discord.write_at(app_clock(), 48_000, std::iter::repeat_n(0.9, 48_000));
        assert!(port.add(feed_c));
        drop(discord);
        mixer.flush();
        for _ in 0..5 {
            firefox.write_at(app_clock(), 1323, std::iter::repeat_n(0.2, 1323));
            heard = callback(&mut mixer, 0.7);
            assert!(heard.iter().all(|&s| s < 0.141), "nothing of Discord: {:?}", heard.iter().find(|s| **s >= 0.141));
        }

        // Shut, the gain lands on zero exactly, so the feeds are emptied while shut.
        for _ in 0..3 {
            firefox.write_at(app_clock(), 1323, std::iter::repeat_n(0.2, 1323));
            heard = callback(&mut mixer, 0.0);
        }
        assert!(heard.iter().all(|&s| s == 0.0) && mixer.gain == 0.0);
    }

    #[test]
    fn ended_feeds_go_back_to_be_freed() {
        let (mut mixer, mut port) = mixer(48_000);
        let now = Instant::now();
        let (writer, feed) = feed(48_000, 48_000);
        assert!(port.add(feed));
        assert!(mixer.begin(now, 480, 1.0));
        drop(writer);
        assert!(!mixer.begin(now, 480, 1.0), "an ended feed leaves the mix");
        assert_eq!(port.retired.slots(), 1);
        port.drop_retired();
        assert_eq!(port.retired.slots(), 0);

        // A stream that ended before its feed ever filled up must not keep its slot.
        let (mut writer, feed) = super::feed(48_000, 48_000);
        assert!(port.add(feed));
        writer.write_at(now, 100, std::iter::repeat_n(0.5, 100));
        assert!(mixer.begin(now, 480, 1.0));
        drop(writer);
        assert!(!mixer.begin(now, 480, 1.0), "nothing more will come: the feed is over");
        port.drop_retired();

        // More feeds than slots: the extra one is neither dropped nor mixed.
        port.drop_retired();
        let mut writers = Vec::new();
        for _ in 0..SLOTS {
            let (writer, feed) = super::feed(48_000, 48_000);
            writers.push(writer);
            assert!(port.add(feed));
        }
        assert!(mixer.begin(now, 480, 1.0));
        let (_writer, extra) = super::feed(48_000, 48_000);
        assert!(port.add(extra));
        mixer.begin(now, 480, 1.0);
        assert_eq!((port.feeds.slots(), port.retired.slots()), (SLOTS - 1, 0), "it waits for a slot");
        writers.pop();
        mixer.begin(now, 480, 1.0);
        mixer.begin(now, 480, 1.0);
        assert_eq!((port.feeds.slots(), port.retired.slots()), (SLOTS, 1), "and takes the one that frees up");
    }

    #[test]
    fn queries_name_an_app_exactly() {
        assert!(same(" SPOTIFY ", "Spotify"));
        assert!(same("firefox nightly", "Firefox Nightly"));
        // Part of a name is not enough: with Discord gone, "Discord" is nobody.
        assert!(!same("Discord", "Discord PTB"));
        assert!(!same("nightly", "Firefox Nightly"));
        assert!(!same("", ""));
    }

    #[test]
    fn lists_of_shared_apps_have_their_own_key() {
        let list = |apps: &[&str]| key(&apps.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        assert_eq!(list(&[]), 0);
        assert_ne!(list(&["Spotify"]), 0);
        assert_eq!(list(&["Spotify"]), list(&["Spotify"]));
        assert_ne!(list(&["Spotify"]), list(&["Firefox"]));
        assert_ne!(list(&["Spotify"]), list(&["Spotify", "Firefox"]));
    }
}
