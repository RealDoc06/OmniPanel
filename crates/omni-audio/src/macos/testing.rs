//! Helpers for the hardware tests (`cargo test -p omni-audio -- --ignored`).

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use objc2_core_audio::{
    kAudioHardwarePropertyProcessObjectList, kAudioProcessPropertyIsRunningOutput, kAudioProcessPropertyPID,
};

use super::ca::{self, ObjectId};
use super::tcc;

/// Tap tests need System Audio Recording for the app running them (the terminal);
/// without it tap creation blocks on the system prompt for a minute.
pub fn require_permission() {
    assert_eq!(tcc::status(), Some(tcc::Permission::Granted), "allow System Audio Recording for this terminal first");
}

/// A quiet 440 Hz stereo tone (about -34 dBFS) played by `afplay` for 20 s; killed on drop.
pub struct Tone(Child);

impl Tone {
    pub fn play() -> Self {
        let path = std::env::temp_dir().join("omni-audio-tone.wav");
        if !path.exists() {
            std::fs::write(&path, wav(48_000, 20)).unwrap();
        }
        Tone(Command::new("afplay").arg(&path).spawn().unwrap())
    }

    pub fn pid(&self) -> u32 {
        self.0.id()
    }

    /// The tone's HAL process object, once it is producing output.
    pub fn object(&self) -> ObjectId {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let objects = ca::get_list::<u32>(ca::SYSTEM, ca::global(kAudioHardwarePropertyProcessObjectList)).unwrap();
            let playing = objects.into_iter().find(|&o| {
                ca::get::<i32>(o, ca::global(kAudioProcessPropertyPID)) == Ok(self.pid() as i32)
                    && ca::get::<u32>(o, ca::global(kAudioProcessPropertyIsRunningOutput)) == Ok(1)
            });
            if let Some(object) = playing {
                return object;
            }
            assert!(Instant::now() < deadline, "afplay never started playing");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Tone {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 16-bit stereo PCM WAV with a 440 Hz sine at amplitude 650 / 32768.
fn wav(rate: u32, secs: u32) -> Vec<u8> {
    let frames = rate * secs;
    let mut out = Vec::with_capacity(44 + frames as usize * 4);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + frames * 4).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    for word in [16u32, 0x0002_0001, rate, rate * 4, 0x0010_0004] {
        out.extend_from_slice(&word.to_le_bytes()); // size, PCM + 2 ch, rate, byte rate, align + bits
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(frames * 4).to_le_bytes());
    for i in 0..frames {
        let s = ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 650.0) as i16;
        out.extend_from_slice(&s.to_le_bytes());
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// CPU time (user + system) used by this process so far.
pub fn cpu_time() -> Duration {
    let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    tv(usage.ru_utime) + tv(usage.ru_stime)
}
