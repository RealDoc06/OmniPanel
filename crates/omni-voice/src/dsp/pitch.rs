//! Real-time pitch shifter. A tap reads a delay line at `ratio` times the write
//! speed; when it runs out of room it jumps by about one window. The exact jump
//! target is picked by normalized cross-correlation (WSOLA-style), so the short
//! crossfade between the old and the new tap joins the waveforms in phase: no
//! comb filtering, warble or clicks typical of plain two-tap shifters.
//! Average latency is about 20 ms.

use super::Biquad;
use super::delay::DelayLine;

/// Smallest delay a tap may reach (room for the cubic interpolator).
const MIN_DELAY: f64 = 4.0;

pub(crate) struct PitchShifter {
    line: DelayLine,
    /// Change of a tap's delay per sample: `1 - ratio`.
    step: f64,
    up: bool,
    main: f64,
    old: f64,
    fade_pos: usize,
    curve: Box<[f32]>,
    /// Main-tap delay at which the next splice happens.
    trigger: f64,
    jump: usize,
    search: usize,
    segment: usize,
    initial: f64,
    /// Anti-aliasing lowpass when shifting up (reading faster decimates).
    aa: Option<[Biquad; 2]>,
}

impl PitchShifter {
    pub fn new(semitones: f32, sr: f32) -> Self {
        let ratio = 2f64.powf(f64::from(semitones) / 12.0);
        let ms = f64::from(sr) * 0.001;
        let jump = (22.0 * ms) as usize;
        let search = (6.7 * ms) as usize; // ±half a period down to ~75 Hz voices
        let segment = (8.0 * ms) as usize;
        let fade = (8.0 * ms) as usize;
        let up = ratio > 1.0;
        let (trigger, initial) = if up {
            let t = fade as f64 * (ratio - 1.0) + MIN_DELAY;
            (t, t + jump as f64)
        } else {
            ((jump + search) as f64 + MIN_DELAY, search as f64 + MIN_DELAY)
        };
        let max_delay = trigger as usize + jump + 2 * search + segment + fade + 8;
        let curve = (0..fade).map(|i| 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / fade as f32).cos()).collect();
        Self {
            line: DelayLine::new(max_delay),
            step: 1.0 - ratio,
            up,
            main: initial,
            old: initial,
            fade_pos: fade,
            curve,
            trigger,
            jump,
            search,
            segment,
            initial,
            aa: (ratio > 1.05).then(|| Biquad::butterworth4((0.45 * f64::from(sr) / ratio) as f32, sr)),
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        let fade_len = self.curve.len();
        for x in buf.iter_mut() {
            let input = match self.aa.as_mut() {
                Some([a, b]) => b.tick(a.tick(*x)),
                None => *x,
            };
            self.line.push(input);
            let mut y = self.line.tap_frac(self.main as f32);
            if self.fade_pos < fade_len {
                let o = self.line.tap_frac(self.old as f32);
                y = o + (y - o) * self.curve[self.fade_pos];
                self.fade_pos += 1;
                self.old += self.step;
            }
            self.main += self.step;
            let due = if self.up { self.main <= self.trigger } else { self.main >= self.trigger };
            if due && self.fade_pos >= fade_len {
                self.splice();
            }
            *x = y;
        }
        if let Some(aa) = self.aa.as_mut() {
            aa.iter_mut().for_each(Biquad::flush);
        }
    }

    /// Jump the main tap by about one window to the best-matching position and
    /// start crossfading from the old one.
    fn splice(&mut self) {
        let base = self.main.floor();
        let frac = self.main - base;
        let base = base as usize;
        let nominal = if self.up { base + self.jump } else { base - self.jump };
        let offset = self.best_offset(base, nominal);
        self.old = self.main;
        self.main = (nominal as isize + offset) as f64 + frac;
        self.fade_pos = 0;
    }

    /// Offset in `-search..=search` whose past segment best matches the one behind `base`.
    fn best_offset(&self, base: usize, nominal: usize) -> isize {
        let line = &self.line;
        let m = self.segment;
        let reference_energy: f32 = (0..m).step_by(2).map(|j| line.tap(base + j).powi(2)).sum();
        if reference_energy < 1e-9 * m as f32 {
            return 0; // silence: any splice point is as good as another
        }
        let score = |k: isize, step: usize| -> f32 {
            let c = (nominal as isize + k) as usize;
            let (mut dot, mut energy) = (0.0f32, 1e-12f32);
            for j in (0..m).step_by(step) {
                let v = line.tap(c + j);
                dot += line.tap(base + j) * v;
                energy += v * v;
            }
            dot / energy.sqrt()
        };
        let s = self.search as isize;
        let coarse = (-s..=s).step_by(2).map(|k| (k, score(k, 2))).max_by(|a, b| a.1.total_cmp(&b.1));
        let best = coarse.map_or(0, |(k, _)| k);
        [best - 1, best, best + 1]
            .into_iter()
            .filter(|k| k.abs() <= s)
            .map(|k| (k, score(k, 1)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(0, |(k, _)| k)
    }

    pub fn reset(&mut self) {
        self.line.clear();
        self.main = self.initial;
        self.old = self.initial;
        self.fade_pos = self.curve.len();
        if let Some(aa) = self.aa.as_mut() {
            aa.iter_mut().for_each(Biquad::reset);
        }
    }
}
