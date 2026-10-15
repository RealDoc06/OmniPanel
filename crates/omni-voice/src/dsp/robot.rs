//! Robot voice by phase-zeroing resynthesis ("robotization", DAFX ch. 7):
//! every `sr / frequency` samples a windowed frame is analysed, its phases are
//! set to zero and the resulting zero-phase grain is overlap-added. The grains
//! keep the voice's spectral envelope (so words stay intelligible) but repeat at
//! a fixed rate, which becomes the new, monotone pitch.

use std::f32::consts::TAU;

use super::delay::DelayLine;

pub(crate) struct Robot {
    fft: Fft,
    window: Box<[f32]>,
    input: Box<[f32]>,
    in_pos: usize,
    output: Box<[f32]>,
    out_pos: usize,
    re: Box<[f32]>,
    im: Box<[f32]>,
    hop: usize,
    count: usize,
    scale: f32,
    mix: f32,
    /// Dry signal delayed by the analysis latency, only when mixing.
    dry: Option<DelayLine>,
}

impl Robot {
    pub fn new(frequency: f32, mix: f32, sr: f32) -> Self {
        let hop = ((sr / frequency).round() as usize).max(16);
        // ~21 ms frames resolve the formants but not the input's own harmonics.
        let n = ((0.021 * sr) as usize).next_power_of_two().max((2 * hop).next_power_of_two()).clamp(256, 16_384);
        let window: Box<[f32]> = (0..n).map(|i| 0.5 - 0.5 * (TAU * i as f32 / n as f32).cos()).collect();
        Self {
            fft: Fft::new(n),
            window,
            input: vec![0.0; n].into_boxed_slice(),
            in_pos: 0,
            output: vec![0.0; n].into_boxed_slice(),
            out_pos: 0,
            re: vec![0.0; n].into_boxed_slice(),
            im: vec![0.0; n].into_boxed_slice(),
            hop,
            count: 0,
            // Grains barely overlap, so match energy: frame energy (3/8 · n · σ²) per hop.
            scale: (8.0 * hop as f32 / (3.0 * n as f32)).sqrt() / n as f32,
            mix,
            dry: (mix < 1.0).then(|| DelayLine::new(n)),
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        let n = self.input.len();
        let mask = n - 1;
        for x in buf.iter_mut() {
            self.input[self.in_pos] = *x;
            self.in_pos = (self.in_pos + 1) & mask;
            let wet = std::mem::take(&mut self.output[self.out_pos]);
            self.out_pos = (self.out_pos + 1) & mask;
            self.count += 1;
            if self.count >= self.hop {
                self.count = 0;
                self.grain();
            }
            *x = match self.dry.as_mut() {
                Some(dry) => {
                    dry.push(*x);
                    let d = dry.tap(n - 1);
                    d + (wet - d) * self.mix
                }
                None => wet,
            };
        }
    }

    /// Analyse the last `n` input samples and overlap-add one zero-phase grain.
    fn grain(&mut self) {
        let n = self.input.len();
        let mask = n - 1;
        for i in 0..n {
            self.re[i] = self.input[(self.in_pos + i) & mask] * self.window[i];
        }
        self.im.fill(0.0);
        self.fft.forward(&mut self.re, &mut self.im);
        for (re, im) in self.re.iter_mut().zip(self.im.iter_mut()) {
            *re = (*re * *re + *im * *im).sqrt();
            *im = 0.0;
        }
        // The magnitude spectrum is real and even, so a forward FFT equals n · IFFT.
        self.fft.forward(&mut self.re, &mut self.im);
        let half = n / 2;
        for i in 0..n {
            let g = self.re[(i + half) & mask] * self.window[i] * self.scale;
            self.output[(self.out_pos + i) & mask] += g;
        }
    }

    pub fn reset(&mut self) {
        self.input.fill(0.0);
        self.output.fill(0.0);
        if let Some(dry) = self.dry.as_mut() {
            dry.clear();
        }
    }
}

/// In-place iterative radix-2 complex FFT with precomputed twiddles.
struct Fft {
    cos: Box<[f32]>,
    sin: Box<[f32]>,
    rev: Box<[u32]>,
}

impl Fft {
    fn new(n: usize) -> Self {
        assert!(n.is_power_of_two());
        let bits = n.trailing_zeros();
        let angle = |k: usize| -TAU * k as f32 / n as f32;
        Self {
            cos: (0..n / 2).map(|k| angle(k).cos()).collect(),
            sin: (0..n / 2).map(|k| angle(k).sin()).collect(),
            rev: (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect(),
        }
    }

    fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let n = re.len();
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let stride = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * stride], self.sin[k * stride]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_dft() {
        let n = 16;
        let x: Vec<f32> = (0..n).map(|i| ((i * 7 % 5) as f32 - 2.0) * 0.3).collect();
        let (mut re, mut im) = (x.clone(), vec![0.0; n]);
        Fft::new(n).forward(&mut re, &mut im);
        for k in 0..n {
            let (mut er, mut ei) = (0.0f32, 0.0f32);
            for (i, &v) in x.iter().enumerate() {
                let a = -TAU * (i * k) as f32 / n as f32;
                er += v * a.cos();
                ei += v * a.sin();
            }
            assert!((re[k] - er).abs() < 1e-4 && (im[k] - ei).abs() < 1e-4, "bin {k}");
        }
    }
}
