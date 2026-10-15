//! Delay-line effects: echo, chorus, flanger.

use super::{Lfo, additive_mix, flush, hermite};

/// Circular buffer; delay 0 is the most recently pushed sample.
pub(crate) struct DelayLine {
    buf: Box<[f32]>,
    mask: usize,
    pos: usize,
}

impl DelayLine {
    pub fn new(max_delay: usize) -> Self {
        let len = (max_delay + 4).next_power_of_two();
        Self { buf: vec![0.0; len].into_boxed_slice(), mask: len - 1, pos: 0 }
    }

    #[inline]
    pub fn push(&mut self, x: f32) {
        self.pos = (self.pos + 1) & self.mask;
        self.buf[self.pos] = x;
    }

    #[inline]
    pub fn tap(&self, delay: usize) -> f32 {
        self.buf[self.pos.wrapping_sub(delay) & self.mask]
    }

    /// Fractional delay (>= 1) with cubic interpolation.
    #[inline]
    pub fn tap_frac(&self, delay: f32) -> f32 {
        let i = delay as usize;
        let t = delay - i as f32;
        // Oldest to newest around the read point; `t` moves from tap(i) towards tap(i + 1).
        hermite(self.tap(i - 1), self.tap(i), self.tap(i + 1), self.tap(i + 2), t)
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

/// Feedback echo; every repeat is slightly darker, like a tape echo.
pub(crate) struct Echo {
    line: DelayLine,
    delay: usize,
    feedback: f32,
    dry: f32,
    wet: f32,
    damp: f32,
    lp: f32,
}

impl Echo {
    pub fn new(delay_ms: f32, feedback: f32, mix: f32, sr: f32) -> Self {
        let delay = ((delay_ms * 0.001 * sr) as usize).max(1);
        let (dry, wet) = additive_mix(mix);
        let damp = 1.0 - (-2.0 * std::f32::consts::PI * 5000.0 / sr).exp();
        Self { line: DelayLine::new(delay), delay, feedback, dry, wet, damp, lp: 0.0 }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            let d = self.line.tap(self.delay - 1);
            self.lp = flush(self.lp + (d - self.lp) * self.damp);
            self.line.push(flush(*x + self.lp * self.feedback));
            *x = *x * self.dry + d * self.wet;
        }
    }

    pub fn reset(&mut self) {
        self.line.clear();
        self.lp = 0.0;
    }
}

/// Two modulated voices with offset base delays and LFO phases.
pub(crate) struct Chorus {
    line: DelayLine,
    lfo: Lfo,
    base: [f32; 2],
    depth: f32,
    mix: f32,
}

impl Chorus {
    pub fn new(rate: f32, depth_ms: f32, mix: f32, sr: f32) -> Self {
        let ms = 0.001 * sr;
        let depth = depth_ms * ms;
        let base = [12.0 * ms, 19.0 * ms];
        Self { line: DelayLine::new((base[1] + depth) as usize + 4), lfo: Lfo::new(rate, sr), base, depth, mix }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            self.line.push(*x);
            let (a, b) = (self.lfo.at(0.0), self.lfo.at(0.25));
            self.lfo.advance();
            let wet = 0.5
                * (self.line.tap_frac(self.base[0] + self.depth * 0.5 * (1.0 + a))
                    + self.line.tap_frac(self.base[1] + self.depth * 0.5 * (1.0 + b)));
            *x += (wet - *x) * self.mix;
        }
    }

    pub fn reset(&mut self) {
        self.line.clear();
    }
}

pub(crate) struct Flanger {
    line: DelayLine,
    lfo: Lfo,
    min: f32,
    depth: f32,
    feedback: f32,
    mix: f32,
}

impl Flanger {
    pub fn new(rate: f32, depth_ms: f32, feedback: f32, mix: f32, sr: f32) -> Self {
        let depth = depth_ms * 0.001 * sr;
        let min = (0.0002 * sr).max(1.0);
        Self { line: DelayLine::new((min + depth) as usize + 4), lfo: Lfo::new(rate, sr), min, depth, feedback, mix }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            let d = self.min + self.depth * 0.5 * (1.0 + self.lfo.next());
            let delayed = self.line.tap_frac(d);
            self.line.push(flush(*x + delayed * self.feedback));
            *x += (delayed - *x) * self.mix;
        }
    }

    pub fn reset(&mut self) {
        self.line.clear();
    }
}
