//! Mono Freeverb (Jezar's public-domain design): 8 damped comb filters in
//! parallel followed by 4 series allpasses. Tunings are scaled from 44.1 kHz.

use super::{additive_mix, flush};

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const INPUT_GAIN: f32 = 0.015;
const WET_SCALE: f32 = 1.0;

struct Comb {
    buf: Box<[f32]>,
    pos: usize,
    store: f32,
}

struct Allpass {
    buf: Box<[f32]>,
    pos: usize,
}

pub(crate) struct Reverb {
    combs: [Comb; 8],
    allpasses: [Allpass; 4],
    feedback: f32,
    damp: f32,
    dry: f32,
    wet: f32,
}

impl Reverb {
    pub fn new(room_size: f32, damping: f32, mix: f32, sr: f32) -> Self {
        let scale = |len: usize| ((len as f32 * sr / 44_100.0) as usize).max(1);
        let (dry, wet) = additive_mix(mix);
        Self {
            combs: COMBS.map(|len| Comb { buf: vec![0.0; scale(len)].into_boxed_slice(), pos: 0, store: 0.0 }),
            allpasses: ALLPASSES.map(|len| Allpass { buf: vec![0.0; scale(len)].into_boxed_slice(), pos: 0 }),
            feedback: 0.7 + 0.28 * room_size,
            damp: 0.4 * damping,
            dry,
            wet: wet * WET_SCALE,
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        let (feedback, damp) = (self.feedback, self.damp);
        for x in buf.iter_mut() {
            let input = *x * INPUT_GAIN;
            let mut acc = 0.0;
            for c in &mut self.combs {
                let out = c.buf[c.pos];
                c.store = flush(out * (1.0 - damp) + c.store * damp);
                c.buf[c.pos] = input + c.store * feedback;
                c.pos = if c.pos + 1 == c.buf.len() { 0 } else { c.pos + 1 };
                acc += out;
            }
            for a in &mut self.allpasses {
                let b = a.buf[a.pos];
                a.buf[a.pos] = flush(acc + b * 0.5);
                a.pos = if a.pos + 1 == a.buf.len() { 0 } else { a.pos + 1 };
                acc = b - acc;
            }
            *x = *x * self.dry + acc * self.wet;
        }
    }

    pub fn reset(&mut self) {
        for c in &mut self.combs {
            c.buf.fill(0.0);
            c.store = 0.0;
        }
        for a in &mut self.allpasses {
            a.buf.fill(0.0);
        }
    }
}
