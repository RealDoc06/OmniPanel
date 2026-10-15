//! Pairing codes and device tokens.
//!
//! The desktop shows a 6-digit code; a deck that sends it gets a random token.
//! Only the token's SHA-256 is stored. A code dies after 10 minutes, after a
//! successful pairing, or after too many wrong guesses. Too many wrong
//! guesses also lock pairing for every connection for a while, so opening
//! many sockets doesn't multiply the guess rate.

use base64::Engine;
use omni_core::{PairingInfo, now_ms};
use rand::Rng;
use sha2::{Digest, Sha256};

const CODE_TTL_MS: u64 = 10 * 60 * 1000;
const MAX_FAILURES: u32 = 5;
/// How long every pairing attempt is refused after `MAX_FAILURES` wrong codes.
pub const LOCKOUT_MS: u64 = 30_000;

#[derive(Default)]
pub struct Pairing {
    active: Option<PairingInfo>,
    /// The last code ran out (it wasn't stopped or used).
    expired: bool,
    failures: u32,
    /// `now_ms()` until which every attempt is refused.
    locked_until: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verify {
    Ok,
    Wrong,
    /// Too many failures: a new code was generated and pairing is locked.
    Rotated,
    /// Locked after too many failures, try again later.
    Locked,
    /// The code ran out; the desktop offers a new one.
    Expired,
    Inactive,
}

impl Pairing {
    pub fn start(&mut self) -> PairingInfo {
        let code = format!("{:06}", rand::rng().random_range(0..1_000_000));
        let info = PairingInfo { code, expires_at: now_ms() + CODE_TTL_MS };
        self.active = Some(info.clone());
        self.expired = false;
        self.failures = 0;
        self.locked_until = 0;
        info
    }

    pub fn stop(&mut self) {
        self.active = None;
        self.expired = false;
    }

    pub fn current(&mut self) -> Option<PairingInfo> {
        if self.active.as_ref().is_some_and(|p| p.expires_at <= now_ms()) {
            self.active = None;
            self.expired = true;
        }
        self.active.clone()
    }

    pub fn verify(&mut self, code: &str) -> Verify {
        let Some(active) = self.current() else {
            return if self.expired { Verify::Expired } else { Verify::Inactive };
        };
        if now_ms() < self.locked_until {
            return Verify::Locked;
        }
        if constant_time_eq(active.code.as_bytes(), code.trim().as_bytes()) {
            self.active = None;
            return Verify::Ok;
        }
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            // Fresh code, same deadline: guessing can't keep pairing open.
            let next = PairingInfo { expires_at: active.expires_at, ..self.start() };
            self.active = Some(next);
            self.locked_until = now_ms() + LOCKOUT_MS;
            return Verify::Rotated;
        }
        Verify::Wrong
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A new device token (256 bits, base64url).
pub fn new_token() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// "Android tablet", "iPhone", "Mac" ... from a user agent.
pub fn platform_from_user_agent(ua: &str) -> String {
    let ua_lower = ua.to_ascii_lowercase();
    let name = if ua_lower.contains("ipad") || (ua_lower.contains("macintosh") && ua_lower.contains("mobile")) {
        "iPad"
    } else if ua_lower.contains("iphone") || ua_lower.contains("ipod") {
        "iPhone"
    } else if ua_lower.contains("android") {
        if ua_lower.contains("mobile") { "Android phone" } else { "Android tablet" }
    } else if ua_lower.contains("windows") {
        "Windows"
    } else if ua_lower.contains("macintosh") || ua_lower.contains("mac os") {
        "Mac"
    } else if ua_lower.contains("cros") {
        "Chromebook"
    } else if ua_lower.contains("linux") {
        "Linux"
    } else {
        "Browser"
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_lifecycle() {
        let mut p = Pairing::default();
        assert_eq!(p.verify("000000"), Verify::Inactive);
        let info = p.start();
        assert_eq!(info.code.len(), 6);
        let wrong = if info.code == "111111" { "222222" } else { "111111" };
        assert_eq!(p.verify(wrong), Verify::Wrong);
        assert_eq!(p.verify(&info.code), Verify::Ok);
        // a code works once
        assert_eq!(p.verify(&info.code), Verify::Inactive);
        // a code that ran out is told apart from pairing being off
        p.start();
        p.active.as_mut().unwrap().expires_at = now_ms();
        assert_eq!(p.verify(wrong), Verify::Expired);
        p.stop();
        assert_eq!(p.verify(wrong), Verify::Inactive);
    }

    #[test]
    fn brute_force_rotates_code_and_locks_every_connection() {
        let mut p = Pairing::default();
        let first = p.start();
        let wrong = if first.code == "999999" { "888888" } else { "999999" };
        let results: Vec<_> = (0..MAX_FAILURES).map(|_| p.verify(wrong)).collect();
        assert_eq!(results.last(), Some(&Verify::Rotated));
        let rotated = p.current().unwrap();
        assert_eq!(rotated.expires_at, first.expires_at, "guessing must not extend pairing");
        // Locked for everyone, even with the right code.
        assert_eq!(p.verify(&rotated.code), Verify::Locked);
        p.locked_until = now_ms();
        assert_eq!(p.verify(&rotated.code), Verify::Ok);

        // "Pair a device" again lifts the lock right away.
        p.start();
        for _ in 0..MAX_FAILURES {
            p.verify(wrong);
        }
        let fresh = p.start();
        assert_eq!(p.verify(&fresh.code), Verify::Ok);
    }

    #[test]
    fn tokens_are_unique_and_hashed() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert_eq!(hash_token(&a).len(), 64);
        assert_eq!(hash_token(&a), hash_token(&a));
    }
}
