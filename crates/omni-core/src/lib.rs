//! Shared domain model for OmniPanel.
//!
//! Every type that crosses a boundary (deck <-> host, host <-> desktop UI)
//! lives here and derives `ts_rs::TS`; `pnpm gen:types` regenerates
//! `packages/protocol/src/generated`.

pub mod action;
pub mod ai;
pub mod api;
pub mod defaults;
pub mod model;
pub mod protocol;
pub mod runtime;
pub mod store;

pub use action::*;
pub use ai::*;
pub use api::*;
pub use model::*;
pub use protocol::*;
pub use runtime::*;

use rand::distr::{Alphanumeric, SampleString};

/// Short random id for profiles, pages, controls, devices.
pub fn new_id() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 10)
}

/// Unix time in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_json_shape() {
        let action =
            Action::Volume { target: VolumeTarget::App { app: "Spotify".into() }, op: VolumeOp::Step { delta: 0.1 } };
        let json = serde_json::to_value(&action).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type": "volume", "target": {"type": "app", "app": "Spotify"}, "op": {"type": "step", "delta": 0.1f32}})
        );
        let script: Action =
            serde_json::from_str(r#"{"type":"runScript","interpreter":"shell","script":"echo hi","showOutput":true}"#)
                .unwrap();
        assert_eq!(
            script,
            Action::RunScript {
                interpreter: Interpreter::Shell,
                script: "echo hi".into(),
                show_output: true,
                timeout_secs: 30
            }
        );
    }

    #[test]
    fn default_config_is_normalized() {
        let mut config = Config::default();
        let before = config.clone();
        config.normalize();
        assert_eq!(config, before);
        let profile = &config.profiles[0];
        assert!(profile.controls().all(|c| c.x + c.w <= profile.cols && c.y + c.h <= profile.rows));
    }

    #[test]
    fn normalize_clamps_controls() {
        let mut profile = defaults::default_profile();
        profile.cols = 3;
        profile.normalize();
        assert!(profile.controls().all(|c| c.x < 3 && c.x + c.w <= 3));
    }
}
