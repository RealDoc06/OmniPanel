//! Output side of the input -> output hand-over.
//!
//! The input callback pushes processed mono samples into a lock-free SPSC ring.
//! Each output callback pulls them through a [`Reader`], which resamples from the
//! input rate to the output rate and steers the ratio slightly so the ring fill
//! stays at a small target. Two devices never share a clock, so without this the
//! latency would drift until the ring under- or overflows. Large deviations
//! (stalls, start-up) are fixed by skipping samples or padding silence.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rtrb::Consumer;

use crate::dsp::{Biquad, hermite};

/// Time constant of the drift controller.
const STEER_SECS: f64 = 5.0;
/// Max ratio correction (±0.5%, a few cents: inaudible).
const MAX_STEER: f64 = 0.005;
/// Extra fill on top of one input plus one output block, for scheduling jitter.
const MARGIN_SECS: f64 = 0.003;
/// Time constant of the callback-size peak hold (a one-off large callback fades out).
const HOLD_SECS: f64 = 10.0;
/// A gap between callbacks this much longer than two of them is a stall.
const STALL_SECS: f64 = 0.1;

pub(crate) struct Reader {
    rx: Consumer<f32>,
    /// Latest input callback size, published by the input callback.
    in_block: Arc<AtomicUsize>,
    /// Peak-held callback sizes (input and output frames).
    in_hold: f64,
    out_hold: f64,
    in_rate: f64,
    out_rate: f64,
    base_ratio: f64,
    /// Input samples consumed per output sample right now.
    ratio: f64,
    frac: f64,
    hist: [f32; 4],
    avg_fill: f64,
    primed: bool,
    /// Fade-in after (re)priming, fade-out on underrun: never a hard step.
    gain: f32,
    gain_step: f32,
    last: f32,
    /// Anti-aliasing when the output rate is lower than the input rate.
    aa: Option<[Biquad; 2]>,
    /// See [`Self::starting_over_after_a_stall`].
    mark: Option<Mark>,
}

/// What a reader that starts over after a stall goes by.
struct Mark {
    read: Box<dyn Fn() -> u64 + Send>,
    /// The mark at the previous callback, and when that was.
    seen: u64,
    called: Option<Instant>,
}

impl Reader {
    pub fn new(rx: Consumer<f32>, in_rate: u32, out_rate: u32, in_block: Arc<AtomicUsize>) -> Self {
        let (in_rate, out_rate) = (f64::from(in_rate), f64::from(out_rate));
        let base_ratio = in_rate / out_rate;
        Self {
            rx,
            in_block,
            in_hold: 0.0,
            out_hold: 0.0,
            in_rate,
            out_rate,
            base_ratio,
            ratio: base_ratio,
            frac: 0.0,
            hist: [0.0; 4],
            avg_fill: 0.0,
            primed: false,
            gain: 0.0,
            gain_step: (1.0 / (0.005 * out_rate)) as f32,
            last: 0.0,
            aa: (base_ratio > 1.01).then(|| Biquad::butterworth4((0.45 * out_rate) as f32, in_rate as f32)),
            mark: None,
        }
    }

    /// Makes a reader that comes back from a stall (no callback for much longer
    /// than its callbacks take) throw away everything that waited in the ring when
    /// `mark()` changed meanwhile: that was written before the change and is not
    /// played late. Whatever the ring holds and however large the callbacks are.
    pub fn starting_over_after_a_stall(mut self, mark: impl Fn() -> u64 + Send + 'static) -> Self {
        let seen = mark();
        self.mark = Some(Mark { read: Box::new(mark), seen, called: None });
        self
    }

    /// Render `frames` output samples; `put(i, sample)` writes frame `i`.
    /// Call once per output callback.
    pub fn render(&mut self, frames: usize, put: impl FnMut(usize, f32)) {
        self.render_at(Instant::now(), frames, put);
    }

    /// [`Self::render`] for a callback that came at `now`.
    fn render_at(&mut self, now: Instant, frames: usize, put: impl FnMut(usize, f32)) {
        if let Some(mark) = self.mark.as_mut() {
            let callback = self.out_hold.max(frames as f64) / self.out_rate;
            let stalled = mark
                .called
                .replace(now)
                .is_some_and(|last| now.duration_since(last).as_secs_f64() > 2.0 * callback + STALL_SECS);
            let read = (mark.read)();
            if std::mem::replace(&mut mark.seen, read) != read && stalled {
                self.flush();
            }
        }
        self.begin(frames, frames);
        self.fill(frames, put);
    }

    /// First half of [`Self::render`], for a consumer that produces one callback
    /// in pieces: look at the ring once for a callback of `frames` frames, then
    /// [`Self::fill`] the pieces. `block` is how much the consumer takes in one
    /// go, which is more than `frames` when its callbacks come in bursts.
    pub fn begin(&mut self, frames: usize, block: usize) {
        let decay = 1.0 - frames as f64 / (HOLD_SECS * self.out_rate);
        let in_block = self.in_block.load(Ordering::Relaxed) as f64;
        self.in_hold = (self.in_hold * decay).max(in_block);
        self.out_hold = (self.out_hold * decay).max(block as f64);
        let fill = self.rx.slots() as f64;
        let in_started = in_block > 0.0;
        // Ring fill (input samples) to aim for: one block each way plus jitter margin.
        // Capped at half the ring so huge callbacks can still prime (with dropouts)
        // instead of waiting forever for a fill the ring cannot hold.
        let half_ring = 0.5 * self.rx.buffer().capacity() as f64;
        let target = (self.in_hold + self.out_hold * self.base_ratio + MARGIN_SECS * self.in_rate).min(half_ring);

        if !self.primed {
            if !in_started || fill < target {
                return;
            }
            self.skip((fill - target) as usize);
            self.primed = true;
            self.avg_fill = target;
            self.frac = 0.0;
            self.hist = [0.0; 4];
        } else if fill > 2.0 * target + self.in_hold {
            // Way behind (e.g. the output stalled): drop the backlog instead of drifting.
            self.skip((fill - target) as usize);
            self.avg_fill = target;
        } else {
            let alpha = 1.0 - (-(frames as f64) / (0.5 * self.out_rate)).exp();
            self.avg_fill += (fill - self.avg_fill) * alpha;
        }
        let steer = ((self.avg_fill - target) / (STEER_SECS * self.in_rate)).clamp(-MAX_STEER, MAX_STEER);
        self.ratio = self.base_ratio * (1.0 + steer);
    }

    /// Second half of [`Self::render`]: the next `frames` samples of the callback
    /// [`Self::begin`] announced (silence until the ring is primed).
    pub fn fill(&mut self, frames: usize, mut put: impl FnMut(usize, f32)) {
        if !self.primed {
            return self.silence(0, frames, &mut put);
        }
        for i in 0..frames {
            self.frac += self.ratio;
            while self.frac >= 1.0 {
                let Ok(mut s) = self.rx.pop() else {
                    self.primed = false;
                    return self.silence(i, frames, &mut put);
                };
                if let Some([a, b]) = self.aa.as_mut() {
                    s = b.tick(a.tick(s));
                }
                self.hist = [self.hist[1], self.hist[2], self.hist[3], s];
                self.frac -= 1.0;
            }
            let [x0, x1, x2, x3] = self.hist;
            self.gain = (self.gain + self.gain_step).min(1.0);
            self.last = hermite(x0, x1, x2, x3, self.frac as f32) * self.gain;
            put(i, self.last);
        }
        if let Some(aa) = self.aa.as_mut() {
            aa.iter_mut().for_each(Biquad::flush);
        }
    }

    /// Fill frames `from..to` with the last sample decaying to zero (~2 ms).
    fn silence(&mut self, from: usize, to: usize, put: &mut impl FnMut(usize, f32)) {
        let decay = (-1.0 / (0.002 * self.out_rate)).exp() as f32;
        for i in from..to {
            self.last *= decay;
            if self.last.abs() < 1e-6 {
                self.last = 0.0;
            }
            put(i, self.last);
        }
        self.gain = 0.0;
    }

    /// The writer is gone and everything it wrote was played, or will never be:
    /// what is left of a ring that is not primed stays below its target for good.
    pub fn finished(&self) -> bool {
        self.rx.is_abandoned() && (self.rx.is_empty() || !self.primed)
    }

    /// Samples are coming out (the ring is primed), as opposed to still filling up.
    pub fn playing(&self) -> bool {
        self.primed
    }

    /// The writer is gone: nothing more will come.
    pub fn ended(&self) -> bool {
        self.rx.is_abandoned()
    }

    /// Throw away what is buffered; playing starts over with what comes after.
    pub fn flush(&mut self) {
        self.skip(self.rx.slots());
        self.primed = false;
        self.gain = 0.0;
        self.last = 0.0;
    }

    fn skip(&mut self, n: usize) {
        if let Ok(chunk) = self.rx.read_chunk(n.min(self.rx.slots())) {
            chunk.commit_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Ring size the engine uses.
    fn ring(in_rate: f64) -> usize {
        (in_rate * crate::audio::RING_CAPACITY_SECS) as usize
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Simulates two free-running device clocks for `secs` and returns
    /// (underrun count after start-up, max ring fill in ms, output samples).
    fn simulate(
        in_rate: f64,
        in_block: usize,
        out_rate: f64,
        out_block: usize,
        secs: f64,
        ring: usize,
    ) -> (usize, f64, Vec<f32>) {
        let (mut tx, rx) = rtrb::RingBuffer::new(ring);
        let in_max = Arc::new(AtomicUsize::new(0));
        let mut reader = Reader::new(rx, in_rate as u32, 48_000, in_max.clone());
        // The reader believes the output runs at 48 kHz; `out_rate` is the real clock.
        let (mut t_in, mut t_out, mut phase) = (0.0, 0.0, 0.0f64);
        let (mut underruns, mut max_fill, mut out) = (0, 0.0f64, Vec::new());
        let mut buf = vec![0.0; out_block];
        while t_out < secs {
            if t_in <= t_out {
                let block: Vec<f32> = (0..in_block)
                    .map(|_| {
                        phase += 440.0 / 48_000.0;
                        (std::f64::consts::TAU * phase).sin() as f32 * 0.5
                    })
                    .collect();
                in_max.store(in_block, Ordering::Relaxed);
                let _ = tx.push_partial_slice(&block);
                t_in += in_block as f64 / in_rate;
            } else {
                let was_primed = reader.primed;
                reader.render(out_block, |i, s| buf[i] = s);
                if was_primed && !reader.primed && t_out > 1.0 {
                    underruns += 1;
                }
                max_fill = max_fill.max(reader.rx.slots() as f64 / 48.0);
                out.extend_from_slice(&buf);
                t_out += out_block as f64 / out_rate;
            }
        }
        (underruns, max_fill, out)
    }

    #[test]
    fn drifting_clocks_keep_latency_bounded() {
        // Nominally equal 48 kHz clocks, real ones 300 ppm apart in both directions,
        // with mismatched callback sizes.
        for out_rate in [48_000.0 * 1.0003, 48_000.0 * 0.9997] {
            let (underruns, max_fill_ms, _) = simulate(48_000.0, 480, out_rate, 256, 120.0, ring(48_000.0));
            assert_eq!(underruns, 0, "out clock {out_rate}");
            assert!(max_fill_ms < 40.0, "latency grew to {max_fill_ms} ms");
        }
    }

    #[test]
    fn resamples_between_rates() {
        // 44.1 kHz input played on a 48 kHz output: the 440 Hz tone (generated at
        // 48 kHz phase steps, so 404.25 Hz at 44.1 kHz) must come out at 404.25 Hz.
        let (underruns, _, out) = simulate(44_100.0, 441, 48_000.0, 512, 10.0, ring(44_100.0));
        assert_eq!(underruns, 0);
        let tail = &out[out.len() - 48_000 * 4..];
        let crossings = tail.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64 / 4.0;
        assert!((crossings - 404.25).abs() < 1.0, "{crossings} Hz");
    }

    #[test]
    fn huge_callbacks_still_play() {
        // 16384-frame callbacks each way need ~0.7 s of fill, which the engine's ring holds.
        let (underruns, _, out) = simulate(48_000.0, 16_384, 48_000.0, 16_384, 10.0, ring(48_000.0));
        assert_eq!(underruns, 0);
        assert!(rms(&out[out.len() / 2..]) > 0.3);
        // A ring too small for that fill plays with gaps instead of staying silent.
        let (_, _, out) = simulate(48_000.0, 16_384, 48_000.0, 16_384, 10.0, 24_000);
        assert!(rms(&out[out.len() / 2..]) > 0.1);
    }

    /// The output stalls while what it plays changes (an app stops being shared):
    /// when it comes back, nothing that waited in the ring from before is played.
    /// Also with callbacks so large that the ring never counts as "way behind".
    #[test]
    fn a_reader_back_from_a_stall_starts_over_when_its_mark_changed() {
        let heard_after_a_stall = |block: usize, marked: bool| {
            let (mut tx, rx) = rtrb::RingBuffer::new(96_000);
            let mark = Arc::new(AtomicUsize::new(1));
            let read = mark.clone();
            let mut reader = Reader::new(rx, 48_000, 48_000, Arc::new(AtomicUsize::new(block)))
                .starting_over_after_a_stall(move || read.load(Ordering::Relaxed) as u64);
            let period = Duration::from_secs_f64(block as f64 / 48_000.0);
            let mut now = Instant::now();
            // `callbacks` input blocks of `level`, each played at once unless `stalled`.
            let mut run = |now: &mut Instant, level: f32, callbacks: usize, stalled: bool| {
                let mut out = Vec::new();
                for _ in 0..callbacks {
                    for _ in 0..block {
                        let _ = tx.push(level);
                    }
                    *now += period;
                    if !stalled {
                        let mut buf = vec![0.0f32; block];
                        reader.render_at(*now, block, |i, s| buf[i] = s);
                        out.extend(buf);
                    }
                }
                out
            };
            assert!(run(&mut now, 0.9, 8, false).ends_with(&[0.9]));
            // The ring fills up while the reader is not called for three seconds.
            run(&mut now, 0.9, 3 * 48_000 / block, true);
            if marked {
                mark.store(2, Ordering::Relaxed);
            }
            run(&mut now, 0.1, 8, false)
        };
        for block in [480, 24_000] {
            let late = heard_after_a_stall(block, false);
            assert!(late.iter().any(|&s| s > 0.8), "{block}: an unmarked backlog is still played");
            let fresh = heard_after_a_stall(block, true);
            assert!(fresh.iter().all(|&s| s < 0.11), "{block}: max {}", fresh.iter().fold(0.0f32, |m, s| m.max(*s)));
            assert!(fresh.ends_with(&[0.1]), "{block}");
        }
    }
}
