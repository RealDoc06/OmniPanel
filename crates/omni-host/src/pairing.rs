//! Pairing codes and device tokens.
//!
//! The desktop shows a 6-digit code; a deck that sends it gets a random token.
//! Only the token's SHA-256 is stored. A code dies after 10 minutes, after a
//! successful pairing, or after too many wrong guesses.

use base64::Engine;
use omni_core::{PairingInfo, now_ms};
use rand::Rng;
use sha2::{Digest, Sha256};

const CODE_TTL_MS: u64 = 10 * 60 * 1000;
const MAX_FAILURES: u32 = 5;

#[derive(Default)]
pub struct Pairing {
    active: Option<PairingInfo>,
    failures: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verify {
    Ok,
    Wrong,
    /// Too many failures: a new code was generated.
    Rotated,
    Inactive,
}

impl Pairing {
    pub fn start(&mut self) -> PairingInfo {
        let code = format!("{:06}", rand::rng().random_range(0..1_000_000));
        let info = PairingInfo { code, expires_at: now_ms() + CODE_TTL_MS };
        self.active = Some(info.clone());
        self.failures = 0;
        info
    }

    pub fn stop(&mut self) {
        self.active = None;
    }

    pub fn current(&mut self) -> Option<PairingInfo> {
        if self.active.as_ref().is_some_and(|p| p.expires_at <= now_ms()) {
            self.active = None;
        }
        self.active.clone()
    }

    pub fn verify(&mut self, code: &str) -> Verify {
        let Some(active) = self.current() else { return Verify::Inactive };
        if constant_time_eq(active.code.as_bytes(), code.trim().as_bytes()) {
            self.active = None;
            return Verify::Ok;
        }
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            self.start();
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
    }

    #[test]
    fn brute_force_rotates_code() {
        let mut p = Pairing::default();
        let first = p.start().code;
        let wrong = if first == "999999" { "888888" } else { "999999" };
        let results: Vec<_> = (0..MAX_FAILURES).map(|_| p.verify(wrong)).collect();
        assert_eq!(results.last(), Some(&Verify::Rotated));
        assert!(p.current().is_some());
    }

    #[test]
    fn tokens_are_unique_and_hashed() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert_eq!(hash_token(&a).len(), 64);
        assert_eq!(hash_token(&a), hash_token(&a));
    }
}
