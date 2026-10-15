//! Memoryless and oscillator effects: ring modulator, distortion, tremolo, bitcrusher.

use super::{Lfo, soft_clip};

pub(crate) struct RingMod {
    osc: Lfo,
    mix: f32,
}

impl RingMod {
    pub fn new(freq: f32, mix: f32, sr: f32) -> Self {
        Self { osc: Lfo::new(freq, sr), mix }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            *x *= 1.0 - self.mix + self.mix * self.osc.next();
        }
    }
}

/// Soft clipper. `drive` 0..1 maps to 0..30 dB of pre-gain with partial make-up gain.
pub(crate) struct Distortion {
    pre: f32,
    post: f32,
    mix: f32,
}

impl Distortion {
    pub fn new(drive: f32, mix: f32) -> Self {
        let pre = 10f32.powf(1.5 * drive);
        Self { pre, post: pre.powf(-0.7), mix }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            let wet = soft_clip(*x * self.pre) * self.post;
            *x += (wet - *x) * self.mix;
        }
    }
}

pub(crate) struct Tremolo {
    lfo: Lfo,
    depth: f32,
}

impl Tremolo {
    pub fn new(rate: f32, depth: f32, sr: f32) -> Self {
        Self { lfo: Lfo::new(rate, sr), depth }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            // Full level at the LFO trough, `1 - depth` at the crest.
            *x *= 1.0 - self.depth * (0.5 + 0.5 * self.lfo.next());
        }
    }
}

/// Quantizes to `bits` (mid-tread, so silence stays silent) and holds every
/// sample for `downsample` frames.
pub(crate) struct Bitcrush {
    levels: f32,
    hold: u8,
    count: u8,
    held: f32,
}

impl Bitcrush {
    pub fn new(bits: u8, downsample: u8) -> Self {
        Self { levels: 2f32.powi(i32::from(bits) - 1), hold: downsample, count: 0, held: 0.0 }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            if self.count == 0 {
                self.held = (*x * self.levels).round() / self.levels;
            }
            self.count += 1;
            if self.count >= self.hold {
                self.count = 0;
            }
            *x = self.held;
        }
    }

    pub fn reset(&mut self) {
        self.count = 0;
        self.held = 0.0;
    }
}
