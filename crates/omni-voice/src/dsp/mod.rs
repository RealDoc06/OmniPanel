//! Voice DSP: the per-preset effect [`Chain`] and the [`Processor`] that wraps it
//! with input/output gain, noise gate, click-free preset crossfades and a safety
//! limiter.
//!
//! Everything here is allocation-free after construction, so it can run inside an
//! audio callback. Chains are built (and dropped) off the audio thread and handed
//! over lock-free. All processing is mono `f32` at the input device rate.

mod delay;
mod filter;
mod gate;
mod pitch;
mod reverb;
mod robot;
mod simple;

use std::f32::consts::{PI, TAU};

use omni_core::{Effect, VoiceSettings};

pub(crate) use filter::Biquad;
use gate::Gate;

/// Frames processed per inner block; callers may pass buffers of any length.
const BLOCK: usize = 256;
/// Length of the crossfade between the old and the new chain on a preset swap.
const FADE_SECS: f32 = 0.05;
/// Recent input replayed into a new chain so its delay lines are filled when it fades in.
const PRIME_SECS: f32 = 0.06;

/// Parameters that change without rebuilding the chain (linear gains).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub input_gain: f32,
    pub output_gain: f32,
    /// Noise gate open threshold as a linear peak level; `None` disables the gate.
    pub gate_threshold: Option<f32>,
}

impl Default for Params {
    fn default() -> Self {
        Self { input_gain: 1.0, output_gain: 1.0, gate_threshold: None }
    }
}

impl From<&VoiceSettings> for Params {
    fn from(s: &VoiceSettings) -> Self {
        Self {
            input_gain: db_to_gain(param(s.input_gain_db, -60.0, 24.0, 0.0)),
            output_gain: db_to_gain(param(s.output_gain_db, -60.0, 24.0, 0.0)),
            gate_threshold: s.noise_gate.then(|| db_to_gain(param(s.gate_threshold_db, -100.0, 0.0, -50.0))),
        }
    }
}

/// An effect chain built from a preset's effects. Building allocates, processing never does.
///
/// Chains are loudness-matched: building one measures it with speech-shaped
/// noise and applies a make-up gain (-6..+12 dB), so switching presets does not
/// jump in level. Explicit [`Effect::Gain`] stages are excluded and still apply.
pub struct Chain {
    stages: Vec<Stage>,
    makeup: f32,
}

impl Chain {
    pub fn new(effects: &[Effect], sample_rate: f32) -> Self {
        let mut chain =
            Self { stages: effects.iter().filter_map(|e| Stage::new(e, sample_rate)).collect(), makeup: 1.0 };
        chain.makeup = chain.measure_makeup(sample_rate);
        chain.reset();
        chain
    }

    /// Process `buf` in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        for stage in &mut self.stages {
            stage.process(buf);
        }
        if self.makeup != 1.0 {
            buf.iter_mut().for_each(|x| *x *= self.makeup);
        }
    }

    /// Gain that brings the (non-`Gain`) stages back to the level of their input.
    fn measure_makeup(&mut self, sr: f32) -> f32 {
        if self.stages.iter().all(|s| matches!(s, Stage::Gain(_))) {
            return 1.0;
        }
        // ~0.6 s of noise shaped like the long-term speech spectrum (80 Hz - 500 Hz,
        // -6 dB/oct above),
        // at a typical microphone level of -24 dBFS RMS.
        let len = (0.6 * sr) as usize;
        let mut probe = vec![0.0f32; len];
        let (mut state, mut hp, mut lp) = (0x9e37_79b9_u32, 0.0f32, 0.0f32);
        let (a_hp, a_lp) = (1.0 - (-TAU * 80.0 / sr).exp(), 1.0 - (-TAU * 500.0 / sr).exp());
        for s in &mut probe {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = (state >> 8) as f32 / (1 << 23) as f32 - 1.0;
            hp += (white - hp) * a_hp;
            lp += ((white - hp) - lp) * a_lp;
            *s = lp;
        }
        let measure_from = len / 3; // past the chain's latency and reverb build-up
        let scale = db_to_gain(-24.0) / rms(&probe[measure_from..]);
        probe.iter_mut().for_each(|s| *s *= scale);
        let reference = rms(&probe[measure_from..]);

        for block in probe.chunks_mut(BLOCK) {
            for stage in self.stages.iter_mut().filter(|s| !matches!(s, Stage::Gain(_))) {
                stage.process(block);
            }
        }
        let level = rms(&probe[measure_from..]);
        if level > 1e-6 { (reference / level).clamp(db_to_gain(-6.0), db_to_gain(12.0)) } else { 1.0 }
    }

    /// Clear all internal state (delay lines, filters, oscillators keep their phase).
    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
    }
}

/// Lock-free hand-over of chains between the control thread and the audio callback.
pub(crate) struct ChainPort {
    pub incoming: rtrb::Consumer<Box<Chain>>,
    /// Chains that are no longer used, dropped by the control thread.
    pub retired: rtrb::Producer<Box<Chain>>,
}

/// Full voice pipeline: input gain -> noise gate -> effect chain -> output gain -> limiter.
pub struct Processor {
    chain: Box<Chain>,
    /// Previous chain while it fades out.
    outgoing: Option<Box<Chain>>,
    /// Next chain, started once the current fade finishes.
    pending: Option<Box<Chain>>,
    port: Option<ChainPort>,
    fade_pos: usize,
    fade_len: usize,
    gate: Gate,
    input_gain: Smoothed,
    output_gain: Smoothed,
    history: Box<[f32]>,
    history_pos: usize,
    prime_len: usize,
    scratch: Box<[f32]>,
    peak_in: f32,
    peak_out: f32,
}

impl Processor {
    pub fn new(sample_rate: f32, chain: Chain) -> Self {
        let prime_len = (PRIME_SECS * sample_rate) as usize;
        Self {
            chain: Box::new(chain),
            outgoing: None,
            pending: None,
            port: None,
            fade_pos: 0,
            fade_len: ((FADE_SECS * sample_rate) as usize).max(1),
            gate: Gate::new(sample_rate),
            input_gain: Smoothed::new(1.0, 0.01, sample_rate),
            output_gain: Smoothed::new(1.0, 0.01, sample_rate),
            history: vec![0.0; prime_len.next_power_of_two()].into_boxed_slice(),
            history_pos: 0,
            prime_len,
            scratch: vec![0.0; BLOCK].into_boxed_slice(),
            peak_in: 0.0,
            peak_out: 0.0,
        }
    }

    /// Receive chains from (and return retired ones to) the control thread.
    pub(crate) fn with_port(mut self, port: ChainPort) -> Self {
        self.port = Some(port);
        self
    }

    pub fn set_params(&mut self, params: Params) {
        self.input_gain.target = params.input_gain;
        self.output_gain.target = params.output_gain;
        self.gate.threshold = params.gate_threshold;
    }

    /// Switch to `chain`: it is primed with recent input, then crossfaded in.
    pub fn set_chain(&mut self, chain: Box<Chain>) {
        if let Some(skipped) = self.pending.replace(chain) {
            self.retire(skipped);
        }
    }

    /// Peak input (after input gain) and output levels since the previous call.
    pub fn take_peaks(&mut self) -> (f32, f32) {
        (std::mem::take(&mut self.peak_in), std::mem::take(&mut self.peak_out))
    }

    /// Process mono samples in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        while let Some(chain) = self.port.as_mut().and_then(|p| p.incoming.pop().ok()) {
            self.set_chain(chain);
        }
        for block in buf.chunks_mut(BLOCK) {
            self.process_block(block);
        }
    }

    fn process_block(&mut self, buf: &mut [f32]) {
        let mut peak = self.peak_in;
        for x in buf.iter_mut() {
            let v = if x.is_finite() { *x } else { 0.0 };
            *x = v * self.input_gain.next();
            peak = peak.max(x.abs());
        }
        self.peak_in = peak;
        self.gate.process(buf);

        if self.outgoing.is_none()
            && let Some(next) = self.pending.take()
        {
            self.start_fade(next);
        }
        self.record_history(buf);

        if let Some(old) = self.outgoing.as_mut() {
            let other = &mut self.scratch[..buf.len()];
            other.copy_from_slice(buf);
            old.process(other);
            self.chain.process(buf);
            let step = 1.0 / self.fade_len as f32;
            for (y, &o) in buf.iter_mut().zip(other.iter()) {
                let t = (self.fade_pos as f32 * step).min(1.0);
                let g = 0.5 - 0.5 * (PI * t).cos();
                *y = o + (*y - o) * g;
                self.fade_pos += 1;
            }
            if self.fade_pos >= self.fade_len
                && let Some(old) = self.outgoing.take()
            {
                self.retire(old);
            }
        } else {
            self.chain.process(buf);
        }

        let mut corrupted = false;
        let mut peak = self.peak_out;
        for y in buf.iter_mut() {
            let mut v = *y * self.output_gain.next();
            if !v.is_finite() {
                v = 0.0;
                corrupted = true;
            }
            v = limit(v);
            peak = peak.max(v.abs());
            *y = v;
        }
        self.peak_out = peak;
        if corrupted {
            self.chain.reset();
            if let Some(old) = self.outgoing.as_mut() {
                old.reset();
            }
        }
    }

    /// Replay recent input through `next` so it starts warm, then begin the crossfade.
    fn start_fade(&mut self, mut next: Box<Chain>) {
        let mask = self.history.len() - 1;
        let start = self.history_pos.wrapping_sub(self.prime_len);
        // Fade the replay in, otherwise its abrupt start would come back later through long delays.
        let ramp = (self.prime_len / 12).max(1);
        let mut done = 0;
        while done < self.prime_len {
            let n = (self.prime_len - done).min(BLOCK);
            for (i, s) in self.scratch[..n].iter_mut().enumerate() {
                let k = done + i;
                // `start` wraps below zero whenever the write position is below `prime_len`.
                *s = self.history[start.wrapping_add(k) & mask] * (k as f32 / ramp as f32).min(1.0);
            }
            next.process(&mut self.scratch[..n]);
            done += n;
        }
        self.outgoing = Some(std::mem::replace(&mut self.chain, next));
        self.fade_pos = 0;
    }

    fn record_history(&mut self, buf: &[f32]) {
        let mask = self.history.len() - 1;
        for &x in buf {
            self.history[self.history_pos] = x;
            self.history_pos = (self.history_pos + 1) & mask;
        }
    }

    fn retire(&mut self, chain: Box<Chain>) {
        if let Some(port) = self.port.as_mut() {
            // Only fails if the control thread stopped draining; then dropping here is the lesser evil.
            let _ = port.retired.push(chain);
        }
    }
}

/// One effect of a chain. Enum dispatch keeps the hot loop free of virtual calls.
enum Stage {
    Pitch(pitch::PitchShifter),
    Robot(robot::Robot),
    RingMod(simple::RingMod),
    Echo(delay::Echo),
    Reverb(reverb::Reverb),
    Distortion(simple::Distortion),
    Filter(Biquad),
    Chorus(delay::Chorus),
    Flanger(delay::Flanger),
    Tremolo(simple::Tremolo),
    Bitcrush(simple::Bitcrush),
    Gain(f32),
}

impl Stage {
    /// `None` for effects that would not change the signal.
    fn new(effect: &Effect, sr: f32) -> Option<Self> {
        let mix = |m: f32| param(m, 0.0, 1.0, 0.0);
        Some(match *effect {
            Effect::Pitch { semitones } => {
                let st = param(semitones, -12.0, 12.0, 0.0);
                if st.abs() < 0.01 {
                    return None;
                }
                Self::Pitch(pitch::PitchShifter::new(st, sr))
            }
            Effect::Robot { frequency, mix: m } if mix(m) > 0.0 => {
                Self::Robot(robot::Robot::new(param(frequency, 40.0, 1000.0, 110.0), mix(m), sr))
            }
            Effect::RingMod { frequency, mix: m } if mix(m) > 0.0 => {
                Self::RingMod(simple::RingMod::new(param(frequency, 1.0, 5000.0, 60.0), mix(m), sr))
            }
            Effect::Echo { delay_ms, feedback, mix: m } if mix(m) > 0.0 => Self::Echo(delay::Echo::new(
                param(delay_ms, 1.0, 2000.0, 300.0),
                param(feedback, 0.0, 0.95, 0.4),
                mix(m),
                sr,
            )),
            Effect::Reverb { room_size, damping, mix: m } if mix(m) > 0.0 => Self::Reverb(reverb::Reverb::new(
                param(room_size, 0.0, 1.0, 0.5),
                param(damping, 0.0, 1.0, 0.5),
                mix(m),
                sr,
            )),
            Effect::Distortion { drive, mix: m } if mix(m) > 0.0 => {
                Self::Distortion(simple::Distortion::new(param(drive, 0.0, 1.0, 0.5), mix(m)))
            }
            Effect::Filter { kind, frequency, q } => Self::Filter(Biquad::new(
                kind,
                param(frequency, 20.0, 0.45 * sr, 1000.0),
                param(q, 0.1, 20.0, 0.707),
                sr,
            )),
            Effect::Chorus { rate_hz, depth_ms, mix: m } if mix(m) > 0.0 => Self::Chorus(delay::Chorus::new(
                param(rate_hz, 0.01, 10.0, 0.8),
                param(depth_ms, 0.0, 20.0, 5.0),
                mix(m),
                sr,
            )),
            Effect::Flanger { rate_hz, depth_ms, feedback, mix: m } if mix(m) > 0.0 => {
                Self::Flanger(delay::Flanger::new(
                    param(rate_hz, 0.01, 10.0, 0.3),
                    param(depth_ms, 0.0, 10.0, 3.0),
                    param(feedback, -0.95, 0.95, 0.5),
                    mix(m),
                    sr,
                ))
            }
            Effect::Tremolo { rate_hz, depth } => {
                let depth = param(depth, 0.0, 1.0, 0.0);
                if depth <= 0.0 {
                    return None;
                }
                Self::Tremolo(simple::Tremolo::new(param(rate_hz, 0.01, 30.0, 5.0), depth, sr))
            }
            Effect::Bitcrush { bits, downsample } => {
                Self::Bitcrush(simple::Bitcrush::new(bits.clamp(1, 24), downsample.clamp(1, 64)))
            }
            Effect::Gain { db } => {
                let db = param(db, -60.0, 24.0, 0.0);
                if db == 0.0 {
                    return None;
                }
                Self::Gain(db_to_gain(db))
            }
            _ => return None,
        })
    }

    fn process(&mut self, buf: &mut [f32]) {
        match self {
            Self::Pitch(s) => s.process(buf),
            Self::Robot(s) => s.process(buf),
            Self::RingMod(s) => s.process(buf),
            Self::Echo(s) => s.process(buf),
            Self::Reverb(s) => s.process(buf),
            Self::Distortion(s) => s.process(buf),
            Self::Filter(s) => s.process(buf),
            Self::Chorus(s) => s.process(buf),
            Self::Flanger(s) => s.process(buf),
            Self::Tremolo(s) => s.process(buf),
            Self::Bitcrush(s) => s.process(buf),
            Self::Gain(g) => buf.iter_mut().for_each(|x| *x *= *g),
        }
    }

    fn reset(&mut self) {
        match self {
            Self::Pitch(s) => s.reset(),
            Self::Robot(s) => s.reset(),
            Self::Echo(s) => s.reset(),
            Self::Reverb(s) => s.reset(),
            Self::Filter(s) => s.reset(),
            Self::Chorus(s) => s.reset(),
            Self::Flanger(s) => s.reset(),
            Self::Bitcrush(s) => s.reset(),
            Self::RingMod(_) | Self::Distortion(_) | Self::Tremolo(_) | Self::Gain(_) => {}
        }
    }
}

/// One-pole parameter smoother (avoids zipper noise on gain changes).
struct Smoothed {
    value: f32,
    target: f32,
    coef: f32,
}

impl Smoothed {
    fn new(value: f32, secs: f32, sr: f32) -> Self {
        Self { value, target: value, coef: 1.0 - (-1.0 / (secs * sr)).exp() }
    }

    #[inline]
    fn next(&mut self) -> f32 {
        let d = self.target - self.value;
        self.value = if d.abs() < 1e-6 { self.target } else { self.value + d * self.coef };
        self.value
    }
}

/// Sine oscillator for LFOs and the ring modulator. Phase in cycles, `[0, 1)`.
struct Lfo {
    phase: f32,
    inc: f32,
}

impl Lfo {
    fn new(hz: f32, sr: f32) -> Self {
        Self { phase: 0.0, inc: hz / sr }
    }

    /// Current value, `offset` in cycles.
    #[inline]
    fn at(&self, offset: f32) -> f32 {
        (TAU * (self.phase + offset)).sin()
    }

    #[inline]
    fn advance(&mut self) {
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
    }

    #[inline]
    fn next(&mut self) -> f32 {
        let s = self.at(0.0);
        self.advance();
        s
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Finite and clamped; NaN/inf (e.g. from a hand-edited config) fall back to `default`.
fn param(x: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if x.is_finite() { x.clamp(lo, hi) } else { default }
}

/// Dry/wet gains for effects that add to the voice (echo, reverb):
/// the dry voice stays at full level up to mix 0.5.
fn additive_mix(mix: f32) -> (f32, f32) {
    ((2.0 * (1.0 - mix)).min(1.0), (2.0 * mix).min(1.0))
}

/// Flush values that would decay into denormals (slow on x86) to zero.
#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < 1e-18 { 0.0 } else { x }
}

/// Rational tanh approximation, exact ±1 beyond |x| = 3 with a smooth join.
#[inline]
fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}

/// Safety limiter: transparent below 0.8, smooth saturation to ±1 above.
#[inline]
fn limit(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = x.abs();
    if a <= KNEE { x } else { (KNEE + (1.0 - KNEE) * soft_clip((a - KNEE) / (1.0 - KNEE))).copysign(x) }
}

/// 4-point cubic Hermite interpolation between `x1` and `x2` at `t` in `[0, 1)`.
#[inline]
pub(crate) fn hermite(x0: f32, x1: f32, x2: f32, x3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x2 - x0);
    let c2 = x0 - 2.5 * x1 + 2.0 * x2 - 0.5 * x3;
    let c3 = 0.5 * (x3 - x0) + 1.5 * (x1 - x2);
    ((c3 * t + c2) * t + c1) * t + x1
}

#[cfg(test)]
pub(crate) mod tests;
