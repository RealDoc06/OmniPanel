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
pub mod proc;
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

    /// No control outside the grid and no cell covered twice.
    fn assert_valid_layout(profile: &Profile) {
        for page in &profile.pages {
            for (i, a) in page.controls.iter().enumerate() {
                assert!(a.w >= 1 && a.h >= 1 && a.x + a.w <= profile.cols && a.y + a.h <= profile.rows, "{a:?}");
                for b in &page.controls[i + 1..] {
                    let overlap = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
                    assert!(!overlap, "{a:?} overlaps {b:?}");
                }
            }
        }
    }

    fn control(id: &str, x: u8, y: u8, w: u8, h: u8) -> Control {
        Control { id: id.into(), x, y, w, h, ..Control::default() }
    }

    #[test]
    fn normalize_moves_controls_that_leave_a_smaller_grid() {
        let mut profile = defaults::default_profile();
        let count = profile.controls().count();
        profile.cols = 3;
        profile.normalize();
        assert_valid_layout(&profile);
        assert_eq!(profile.controls().count(), count);
        // The 1x3 volume slider moves whole instead of shrinking.
        assert!(profile.controls().any(|c| matches!(c.kind, ControlKind::Slider { .. }) && c.h == 3));
    }

    #[test]
    fn normalize_reflows_overlapping_controls() {
        let page = |id: &str, controls| Page { id: id.into(), name: id.into(), controls };
        let mut profile = Profile {
            rows: 2,
            cols: 2,
            pages: vec![
                page(
                    "one",
                    vec![
                        control("wide", 0, 0, 2, 1),
                        control("under", 1, 0, 1, 1),
                        control("tall", 1, 0, 1, 2),
                        control("off-grid", 7, 7, 1, 1),
                    ],
                ),
                page("two", vec![control("full", 0, 0, 2, 2)]),
            ],
            ..Profile::default()
        };
        profile.normalize();
        assert_valid_layout(&profile);
        let at = |id: &str| profile.controls().find(|c| c.id == id).map(|c| (c.x, c.y, c.w, c.h));
        assert_eq!(at("wide"), Some((0, 0, 2, 1)));
        assert_eq!(at("under"), Some((0, 1, 1, 1)));
        assert_eq!(at("off-grid"), Some((1, 1, 1, 1)));
        assert_eq!(at("full"), Some((0, 0, 2, 2)));
        // No room for its 1x2 span on pages one and two: a new page, size kept.
        assert_eq!(profile.pages.len(), 3);
        assert_eq!(profile.pages[2].controls[0].id, "tall");
        assert_eq!(at("tall"), Some((0, 0, 1, 2)));

        let once = profile.clone();
        profile.normalize();
        assert_eq!(profile, once);
    }

    #[test]
    fn normalize_repairs_duplicate_ids() {
        let page = |id: &str, controls| Page { id: id.into(), name: id.into(), controls };
        let profile = |id: &str, pages| Profile { id: id.into(), pages, ..Profile::default() };
        let mut config = Config {
            profiles: vec![
                profile(
                    "p",
                    vec![page("a", vec![control("c", 0, 0, 1, 1), control("c", 1, 0, 1, 1)]), page("a", vec![])],
                ),
                profile("p", vec![page("a", vec![control("c", 0, 0, 1, 1), control("", 1, 0, 1, 1)])]),
            ],
            ..Config::default()
        };
        config.normalize();
        assert_eq!((config.profiles[0].id.as_str(), config.profiles[0].pages[0].id.as_str()), ("p", "a"));
        assert_ne!(config.profiles[1].id, "p");
        assert_ne!(config.profiles[0].pages[1].id, "a");
        // Pages are looked up within their profile.
        assert_eq!(config.profiles[1].pages[0].id, "a");
        let controls: std::collections::HashSet<_> =
            config.profiles.iter().flat_map(|p| p.controls()).map(|c| c.id.clone()).collect();
        assert_eq!(controls.len(), 4);
        assert!(controls.contains("c") && !controls.contains(""));
    }
}
// path filter test
