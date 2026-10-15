//! RBJ "Audio EQ Cookbook" biquads (transposed direct form II, f64 state).

use std::f64::consts::PI;

use omni_core::FilterKind;

#[derive(Clone)]
pub(crate) struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// Bandpass has 0 dB gain at `freq`.
    pub fn new(kind: FilterKind, freq: f32, q: f32, sr: f32) -> Self {
        let w0 = 2.0 * PI * f64::from(freq) / f64::from(sr);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * f64::from(q));
        let (b0, b1, b2) = match kind {
            FilterKind::Lowpass => ((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0),
            FilterKind::Highpass => ((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0),
            FilterKind::Bandpass => (alpha, 0.0, -alpha),
        };
        let a0 = 1.0 + alpha;
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: -2.0 * cos / a0, a2: (1.0 - alpha) / a0, z1: 0.0, z2: 0.0 }
    }

    /// 4th-order Butterworth lowpass as two cascaded sections (anti-aliasing).
    pub fn butterworth4(freq: f32, sr: f32) -> [Self; 2] {
        [Self::new(FilterKind::Lowpass, freq, 0.541_196, sr), Self::new(FilterKind::Lowpass, freq, 1.306_563, sr)]
    }

    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y as f32
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            *x = self.tick(*x);
        }
        self.flush();
    }

    /// Zero tiny state values; call once per block when using [`Self::tick`] directly.
    pub fn flush(&mut self) {
        if self.z1.abs() < 1e-30 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-30 {
            self.z2 = 0.0;
        }
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}
