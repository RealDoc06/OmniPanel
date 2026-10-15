//! Built-in themes, voice presets and the starter profile.

use crate::action::{Action, MediaCommand, NavTarget, SystemCommand, VoiceOp, VolumeOp, VolumeTarget};
use crate::model::{
    Control, ControlKind, Effect, FilterKind, Icon, Page, Profile, Style, Theme, ThemeColors, VoicePreset,
};
use crate::new_id;

pub const DEFAULT_THEME_ID: &str = "midnight";

pub fn builtin_themes() -> Vec<Theme> {
    #[rustfmt::skip]
    let t = |id: &str, name: &str, radius: u8, c: [&str; 13]| Theme {
        id: id.into(),
        name: name.into(),
        radius,
        colors: ThemeColors {
            background: c[0].into(), surface: c[1].into(), surface_hover: c[2].into(), border: c[3].into(),
            text: c[4].into(), text_muted: c[5].into(), accent: c[6].into(), accent_text: c[7].into(),
            success: c[8].into(), warning: c[9].into(), danger: c[10].into(), button: c[11].into(), button_text: c[12].into(),
        },
    };
    // background, surface, surfaceHover, border, text, textMuted, accent, accentText,
    // success, warning, danger, button, buttonText
    #[rustfmt::skip]
    let themes = vec![
        t("midnight", "Midnight", 10, ["#000000", "#0b0b0b", "#161616", "#1f1f1f", "#ffffff", "#8c8c8c", "#4f8cff", "#ffffff", "#22c55e", "#f59e0b", "#ef4444", "#121212", "#ffffff"]),
        t("graphite", "Graphite", 10, ["#111214", "#1a1b1e", "#232428", "#2c2e33", "#ececec", "#9a9ca3", "#a78bfa", "#111214", "#4ade80", "#fbbf24", "#f87171", "#1f2024", "#ececec"]),
        t("nord", "Nord", 8, ["#2e3440", "#3b4252", "#434c5e", "#4c566a", "#eceff4", "#a7b0c0", "#88c0d0", "#2e3440", "#a3be8c", "#ebcb8b", "#bf616a", "#3b4252", "#eceff4"]),
        t("dracula", "Dracula", 10, ["#282a36", "#303241", "#3a3c4e", "#44475a", "#f8f8f2", "#9ea3c4", "#bd93f9", "#282a36", "#50fa7b", "#f1fa8c", "#ff5555", "#343746", "#f8f8f2"]),
        t("synthwave", "Synthwave", 12, ["#0d0221", "#160a33", "#1f1147", "#2d1b5e", "#fdfdff", "#a99bd1", "#ff2a6d", "#ffffff", "#05d9e8", "#f9c80e", "#ff3864", "#1a0b3d", "#ffffff"]),
        t("forest", "Forest", 10, ["#0b1210", "#111b18", "#182621", "#22332d", "#e8f2ec", "#8fa79a", "#34d399", "#04130d", "#34d399", "#facc15", "#f87171", "#13201c", "#e8f2ec"]),
        t("paper", "Paper", 10, ["#f6f6f4", "#ffffff", "#ededea", "#dededa", "#111111", "#6b6b6b", "#2563eb", "#ffffff", "#16a34a", "#d97706", "#dc2626", "#ffffff", "#111111"]),
        t("contrast", "High Contrast", 4, ["#000000", "#000000", "#1a1a1a", "#ffffff", "#ffffff", "#e6e6e6", "#ffff00", "#000000", "#00ff00", "#ffa500", "#ff3030", "#000000", "#ffffff"]),
    ];
    themes
}

pub fn builtin_voice_presets() -> Vec<VoicePreset> {
    use Effect::*;
    let p = |id: &str, name: &str, effects: Vec<Effect>| VoicePreset { id: id.into(), name: name.into(), effects };
    vec![
        p("clean", "Clean", vec![]),
        p(
            "robot",
            "Robot",
            vec![Robot { frequency: 110.0, mix: 1.0 }, Reverb { room_size: 0.2, damping: 0.5, mix: 0.1 }],
        ),
        p("chipmunk", "Chipmunk", vec![Pitch { semitones: 7.0 }]),
        p(
            "deep",
            "Deep",
            vec![Pitch { semitones: -5.0 }, Filter { kind: FilterKind::Lowpass, frequency: 6000.0, q: 0.7 }],
        ),
        p(
            "demon",
            "Demon",
            vec![
                Pitch { semitones: -8.0 },
                Distortion { drive: 0.35, mix: 0.5 },
                Reverb { room_size: 0.7, damping: 0.4, mix: 0.3 },
            ],
        ),
        p(
            "radio",
            "Radio",
            vec![
                Filter { kind: FilterKind::Bandpass, frequency: 1500.0, q: 0.9 },
                Distortion { drive: 0.25, mix: 0.6 },
            ],
        ),
        p(
            "telephone",
            "Telephone",
            vec![Filter { kind: FilterKind::Bandpass, frequency: 1700.0, q: 1.4 }, Bitcrush { bits: 8, downsample: 2 }],
        ),
        p("echo", "Echo", vec![Echo { delay_ms: 280.0, feedback: 0.45, mix: 0.4 }]),
        p("cathedral", "Cathedral", vec![Reverb { room_size: 0.92, damping: 0.25, mix: 0.45 }]),
        p(
            "alien",
            "Alien",
            vec![
                Pitch { semitones: 4.0 },
                RingMod { frequency: 60.0, mix: 0.5 },
                Flanger { rate_hz: 0.3, depth_ms: 3.0, feedback: 0.6, mix: 0.5 },
            ],
        ),
        p(
            "megaphone",
            "Megaphone",
            vec![
                Filter { kind: FilterKind::Highpass, frequency: 600.0, q: 0.7 },
                Filter { kind: FilterKind::Lowpass, frequency: 4000.0, q: 0.7 },
                Distortion { drive: 0.6, mix: 0.8 },
            ],
        ),
        p(
            "underwater",
            "Underwater",
            vec![
                Filter { kind: FilterKind::Lowpass, frequency: 700.0, q: 1.2 },
                Chorus { rate_hz: 0.8, depth_ms: 8.0, mix: 0.6 },
            ],
        ),
    ]
}

fn builtin(name: &str) -> Option<Icon> {
    Some(Icon::Builtin { name: name.into() })
}

fn button(x: u8, y: u8, label: &str, icon: &str, action: Action) -> Control {
    Control {
        id: new_id(),
        x,
        y,
        w: 1,
        h: 1,
        style: Style { label: label.into(), icon: builtin(icon), ..Style::default() },
        kind: ControlKind::Button { action: Some(action) },
        hotkey: None,
    }
}

/// Toggle-style control: shows `active_icon` on `danger`-ish red while active.
fn toggle_button(x: u8, y: u8, label: &str, icon: &str, active_icon: &str, action: Action) -> Control {
    let mut c = button(x, y, label, icon, action);
    c.style.active_icon = builtin(active_icon);
    c.style.active_bg = Some("#b91c1c".into());
    c
}

/// Starter profile created on first launch.
pub fn default_profile() -> Profile {
    let media = |command| Action::Media { command };
    let main = Page {
        id: new_id(),
        name: "Main".into(),
        controls: vec![
            button(0, 0, "Play", "play", media(MediaCommand::PlayPause)),
            button(1, 0, "Previous", "skip-back", media(MediaCommand::Previous)),
            button(2, 0, "Next", "skip-forward", media(MediaCommand::Next)),
            toggle_button(
                3,
                0,
                "Mic",
                "mic",
                "mic-off",
                Action::Volume { target: VolumeTarget::Mic, op: VolumeOp::ToggleMute },
            ),
            toggle_button(
                0,
                1,
                "Sound",
                "volume-2",
                "volume-x",
                Action::Volume { target: VolumeTarget::Master, op: VolumeOp::ToggleMute },
            ),
            {
                let mut c = button(1, 1, "Voice FX", "audio-waveform", Action::Voice { op: VoiceOp::Toggle });
                c.style.active_label = Some("FX On".into());
                c
            },
            button(2, 1, "Next Voice", "shuffle", Action::Voice { op: VoiceOp::Cycle { preset_ids: vec![] } }),
            button(3, 1, "Lock", "lock", Action::System { command: SystemCommand::Lock }),
            button(
                0,
                2,
                "Vol -",
                "volume-1",
                Action::Volume { target: VolumeTarget::Master, op: VolumeOp::Step { delta: -0.05 } },
            ),
            button(
                1,
                2,
                "Vol +",
                "volume-2",
                Action::Volume { target: VolumeTarget::Master, op: VolumeOp::Step { delta: 0.05 } },
            ),
            button(2, 2, "Display Off", "monitor-off", Action::System { command: SystemCommand::DisplaySleep }),
            button(3, 2, "More", "chevron-right", Action::Navigate { to: NavTarget::NextPage }),
            Control {
                id: new_id(),
                x: 4,
                y: 0,
                w: 1,
                h: 3,
                style: Style { label: "Volume".into(), icon: builtin("volume-2"), ..Style::default() },
                kind: ControlKind::Slider { target: VolumeTarget::Master, vertical: true },
                hotkey: None,
            },
        ],
    };
    let more = Page {
        id: new_id(),
        name: "More".into(),
        controls: vec![button(0, 0, "Back", "chevron-left", Action::Navigate { to: NavTarget::PrevPage })],
    };
    Profile { id: new_id(), name: "Default".into(), rows: 3, cols: 5, theme_id: None, pages: vec![main, more] }
}
