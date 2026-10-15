//! Noise gate with hysteresis and hold, so it neither chatters on levels near
//! the threshold nor chops word endings.

use super::flush;

/// The gate closes only after the level stays this far below the open threshold.
const HYSTERESIS: f32 = 0.5; // -6 dB
const HOLD_SECS: f32 = 0.08;

pub(crate) struct Gate {
    /// Open threshold (linear peak). `None` = always open.
    pub threshold: Option<f32>,
    env: f32,
    gain: f32,
    open: bool,
    hold: u32,
    hold_len: u32,
    env_attack: f32,
    env_release: f32,
    gain_open: f32,
    gain_close: f32,
}

impl Gate {
    pub fn new(sr: f32) -> Self {
        let coef = |secs: f32| 1.0 - (-1.0 / (secs * sr)).exp();
        Self {
            threshold: None,
            env: 0.0,
            gain: 1.0,
            open: true,
            hold: 0,
            hold_len: (HOLD_SECS * sr) as u32,
            env_attack: coef(0.0005),
            env_release: coef(0.02),
            gain_open: coef(0.0015),
            gain_close: coef(0.06),
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        let Some(open_at) = self.threshold else {
            self.open = true;
            if self.gain < 1.0 {
                for x in buf.iter_mut() {
                    self.gain = (self.gain + (1.0 - self.gain) * self.gain_open).min(1.0);
                    *x *= self.gain;
                }
                if 1.0 - self.gain < 1e-4 {
                    self.gain = 1.0;
                }
            }
            return;
        };
        let close_at = open_at * HYSTERESIS;
        for x in buf.iter_mut() {
            let level = x.abs();
            let c = if level > self.env { self.env_attack } else { self.env_release };
            self.env = flush(self.env + (level - self.env) * c);

            if self.env >= open_at {
                self.open = true;
            }
            if self.open {
                if self.env >= close_at {
                    self.hold = self.hold_len;
                } else if self.hold > 0 {
                    self.hold -= 1;
                } else {
                    self.open = false;
                }
            }

            let target = if self.open { 1.0 } else { 0.0 };
            let c = if target > self.gain { self.gain_open } else { self.gain_close };
            self.gain = flush(self.gain + (target - self.gain) * c);
            if self.gain < 1e-5 && !self.open {
                self.gain = 0.0;
            }
            *x *= self.gain;
        }
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }
}
