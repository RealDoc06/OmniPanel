//! Keyboard input: accelerator parsing (`Ctrl+Shift+M`, `Cmd+Space`, `F13`...)
//! and key presses / text entry through `enigo`.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use enigo::{Direction, Enigo, InputResult, Key, Keyboard, Settings};

use crate::{ActionError, Result, blocking};

/// Pause between holding the modifiers and clicking the key, so slow apps
/// see the modifiers first.
const MODIFIER_SETTLE: Duration = Duration::from_millis(10);

#[cfg(target_os = "macos")]
const PERMISSION_HINT: &str = "OmniPanel needs Accessibility permission to press keys. \
    Enable it in System Settings \u{2192} Privacy & Security \u{2192} Accessibility, then try again";

/// A parsed accelerator: modifiers to hold (in the order written) and the key to click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hotkey {
    pub modifiers: Vec<Key>,
    pub key: Key,
}

impl FromStr for Hotkey {
    type Err = String;

    /// Same format as Tauri's global shortcuts: `+`-separated, case-insensitive,
    /// modifiers first and the key last. A literal plus is `Plus` or a
    /// trailing `+` (`Ctrl++`).
    fn from_str(text: &str) -> std::result::Result<Self, String> {
        let text = text.trim();
        let (modifiers, key) = if text == "+" {
            ("", "+")
        } else if let Some(modifiers) = text.strip_suffix("++") {
            (modifiers, "+")
        } else {
            text.rsplit_once('+').unwrap_or(("", text))
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("\"{text}\" has no key"));
        }
        if modifier(key).is_some() {
            return Err(format!("\"{text}\" has no key after the modifiers"));
        }

        let mut held = Vec::new();
        if !modifiers.is_empty() {
            for token in modifiers.split('+') {
                let token = token.trim();
                let m = modifier(token).ok_or_else(|| format!("unknown modifier \"{token}\" in \"{text}\""))?;
                if !held.contains(&m) {
                    held.push(m);
                }
            }
        }
        let key = parse_key(key).ok_or_else(|| format!("unknown key \"{key}\" in \"{text}\" on this platform"))?;
        Ok(Self { modifiers: held, key })
    }
}

impl Hotkey {
    /// Hold the modifiers, click the key, release the modifiers in reverse.
    /// Modifiers are released even when the click fails.
    fn press(&self, keyboard: &mut impl Keyboard) -> InputResult<()> {
        let mut result = Ok(());
        let mut pressed = 0;
        for &modifier in &self.modifiers {
            result = keyboard.key(modifier, Direction::Press);
            if result.is_err() {
                break;
            }
            pressed += 1;
        }
        if result.is_ok() {
            if pressed > 0 {
                std::thread::sleep(MODIFIER_SETTLE);
            }
            result = keyboard.key(self.key, Direction::Click);
        }
        for &modifier in self.modifiers[..pressed].iter().rev() {
            result = result.and(keyboard.key(modifier, Direction::Release));
        }
        result
    }
}

fn modifier(token: &str) -> Option<Key> {
    Some(match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Key::Control,
        "shift" => Key::Shift,
        "alt" | "option" | "opt" => Key::Alt,
        "cmd" | "command" | "super" | "meta" | "win" | "windows" => Key::Meta,
        "cmdorctrl" | "cmdorcontrol" | "commandorctrl" | "commandorcontrol" => {
            if cfg!(target_os = "macos") {
                Key::Meta
            } else {
                Key::Control
            }
        }
        _ => return None,
    })
}

#[rustfmt::skip]
const FUNCTION_KEYS: &[Key] = &[
    Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6, Key::F7, Key::F8, Key::F9, Key::F10,
    Key::F11, Key::F12, Key::F13, Key::F14, Key::F15, Key::F16, Key::F17, Key::F18, Key::F19, Key::F20,
    #[cfg(not(target_os = "macos"))] Key::F21,
    #[cfg(not(target_os = "macos"))] Key::F22,
    #[cfg(not(target_os = "macos"))] Key::F23,
    #[cfg(not(target_os = "macos"))] Key::F24,
];

const NUMPAD_DIGITS: [Key; 10] = [
    Key::Numpad0,
    Key::Numpad1,
    Key::Numpad2,
    Key::Numpad3,
    Key::Numpad4,
    Key::Numpad5,
    Key::Numpad6,
    Key::Numpad7,
    Key::Numpad8,
    Key::Numpad9,
];

fn parse_key(token: &str) -> Option<Key> {
    let mut chars = token.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return (!c.is_whitespace() && !c.is_control()).then(|| Key::Unicode(c.to_ascii_lowercase()));
    }
    let name = token.to_ascii_lowercase();
    let name = name.as_str();

    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
        return FUNCTION_KEYS.get(n.checked_sub(1)?).copied();
    }
    // Tauri / DOM style names: KeyA, Digit1, Numpad5.
    for prefix in ["key", "digit"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            let mut chars = rest.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                return Some(Key::Unicode(c));
            }
        }
    }
    for prefix in ["numpad", "num"] {
        if let Some(digit) = name.strip_prefix(prefix).and_then(|d| d.parse::<usize>().ok()) {
            return NUMPAD_DIGITS.get(digit).copied();
        }
    }

    Some(match name {
        "enter" | "return" => Key::Return,
        "tab" => Key::Tab,
        "esc" | "escape" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "space" | "spacebar" => Key::Space,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" | "pgdown" => Key::PageDown,
        "up" | "arrowup" | "uparrow" => Key::UpArrow,
        "down" | "arrowdown" | "downarrow" => Key::DownArrow,
        "left" | "arrowleft" | "leftarrow" => Key::LeftArrow,
        "right" | "arrowright" | "rightarrow" => Key::RightArrow,
        "capslock" => Key::CapsLock,
        "plus" => Key::Unicode('+'),
        "minus" => Key::Unicode('-'),
        "equal" | "equals" => Key::Unicode('='),
        "comma" => Key::Unicode(','),
        "period" | "dot" => Key::Unicode('.'),
        "slash" => Key::Unicode('/'),
        "backslash" => Key::Unicode('\\'),
        "semicolon" => Key::Unicode(';'),
        "quote" | "apostrophe" => Key::Unicode('\''),
        "backquote" | "backtick" | "grave" => Key::Unicode('`'),
        "bracketleft" | "leftbracket" => Key::Unicode('['),
        "bracketright" | "rightbracket" => Key::Unicode(']'),
        "numpadadd" | "numadd" | "numpadplus" | "numplus" => Key::Add,
        "numpadsubtract" | "numsubtract" | "numpadminus" | "numminus" => Key::Subtract,
        "numpadmultiply" | "nummultiply" => Key::Multiply,
        "numpaddivide" | "numdivide" => Key::Divide,
        "numpaddecimal" | "numdecimal" => Key::Decimal,
        "playpause" | "mediaplaypause" | "mediaplay" => Key::MediaPlayPause,
        "medianext" | "mediatracknext" | "nexttrack" => Key::MediaNextTrack,
        "mediaprevious" | "mediaprev" | "mediatrackprevious" | "mediatrackprev" | "previoustrack" => {
            Key::MediaPrevTrack
        }
        "volumeup" | "audiovolumeup" => Key::VolumeUp,
        "volumedown" | "audiovolumedown" => Key::VolumeDown,
        "volumemute" | "audiovolumemute" | "mute" => Key::VolumeMute,
        // macOS has no keys for these.
        #[cfg(not(target_os = "macos"))]
        "mediastop" => Key::MediaStop,
        #[cfg(not(target_os = "macos"))]
        "insert" | "ins" => Key::Insert,
        #[cfg(not(target_os = "macos"))]
        "pause" => Key::Pause,
        #[cfg(not(target_os = "macos"))]
        "printscreen" | "prtsc" | "printscr" => Key::PrintScr,
        #[cfg(not(target_os = "macos"))]
        "numlock" => Key::Numlock,
        _ => return None,
    })
}

/// Shared input device. Created on first use and kept (a new X11 connection
/// per key press would be wasteful); dropped after any failure, which also
/// releases keys that might still be held.
static ENIGO: Mutex<Option<Enigo>> = Mutex::new(None);

fn connect() -> Result<Enigo> {
    // Ask for the macOS Accessibility permission once; later attempts only report it.
    static PROMPTED: AtomicBool = AtomicBool::new(false);
    let settings =
        Settings { open_prompt_to_get_permissions: !PROMPTED.swap(true, Ordering::Relaxed), ..Settings::default() };
    Enigo::new(&settings).map_err(|error| match error {
        #[cfg(target_os = "macos")]
        enigo::NewConError::NoPermission => ActionError::Failed(PERMISSION_HINT.into()),
        #[cfg(all(unix, not(target_os = "macos")))]
        enigo::NewConError::EstablishCon(_) => ActionError::Failed(
            "can't simulate keyboard input: no X11 display found (pure Wayland sessions are not supported)".into(),
        ),
        error => ActionError::Failed(format!("can't simulate keyboard input: {error}")),
    })
}

fn with_enigo<T>(work: impl FnOnce(&mut Enigo) -> InputResult<T>) -> Result<T> {
    let mut slot = ENIGO.lock().unwrap_or_else(PoisonError::into_inner);
    let enigo = match &mut *slot {
        Some(enigo) => enigo,
        empty => empty.insert(connect()?),
    };
    work(enigo).map_err(|error| {
        *slot = None;
        ActionError::Failed(format!("keyboard input failed: {error}"))
    })
}

/// Click one key. Blocking, call from `spawn_blocking`.
pub(crate) fn tap(key: Key) -> Result<()> {
    with_enigo(|enigo| enigo.key(key, Direction::Click))
}

/// Press an accelerator such as `Ctrl+Shift+M`.
pub(crate) async fn press(accelerator: &str) -> Result<()> {
    let hotkey: Hotkey = accelerator.parse().map_err(ActionError::Invalid)?;
    blocking(move || with_enigo(|enigo| hotkey.press(enigo))).await
}

/// Type text as if entered on the keyboard.
pub(crate) async fn type_text(text: &str) -> Result<()> {
    let text = text.to_owned();
    blocking(move || with_enigo(|enigo| enigo.text(&text))).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use enigo::InputError;

    fn parse(text: &str) -> Hotkey {
        text.parse().unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    fn hotkey(modifiers: &[Key], key: Key) -> Hotkey {
        Hotkey { modifiers: modifiers.to_vec(), key }
    }

    #[test]
    fn modifiers_and_letters() {
        assert_eq!(parse("Ctrl+Shift+M"), hotkey(&[Key::Control, Key::Shift], Key::Unicode('m')));
        assert_eq!(parse("cmd+space"), hotkey(&[Key::Meta], Key::Space));
        assert_eq!(parse("Super+L"), hotkey(&[Key::Meta], Key::Unicode('l')));
        assert_eq!(parse("Option+Alt+x"), hotkey(&[Key::Alt], Key::Unicode('x')));
        assert_eq!(parse(" Ctrl + 5 "), hotkey(&[Key::Control], Key::Unicode('5')));
        assert_eq!(parse("Z"), hotkey(&[], Key::Unicode('z')));
    }

    #[test]
    fn cmd_or_ctrl_follows_platform() {
        let expected = if cfg!(target_os = "macos") { Key::Meta } else { Key::Control };
        for text in ["CmdOrCtrl+K", "CommandOrControl+K", "cmdorcontrol+k"] {
            assert_eq!(parse(text), hotkey(&[expected], Key::Unicode('k')), "{text}");
        }
    }

    #[test]
    fn function_navigation_and_editing_keys() {
        assert_eq!(parse("Alt+F4"), hotkey(&[Key::Alt], Key::F4));
        assert_eq!(parse("F13"), hotkey(&[], Key::F13));
        assert_eq!(parse("f20"), hotkey(&[], Key::F20));
        assert_eq!(parse("Ctrl+Enter").key, Key::Return);
        for (name, key) in [
            ("Tab", Key::Tab),
            ("Escape", Key::Escape),
            ("Esc", Key::Escape),
            ("Backspace", Key::Backspace),
            ("Delete", Key::Delete),
            ("Home", Key::Home),
            ("End", Key::End),
            ("PageUp", Key::PageUp),
            ("PageDown", Key::PageDown),
            ("Space", Key::Space),
            ("Left", Key::LeftArrow),
            ("ArrowRight", Key::RightArrow),
            ("Up", Key::UpArrow),
            ("ArrowDown", Key::DownArrow),
            ("Numpad7", Key::Numpad7),
            ("NumpadAdd", Key::Add),
        ] {
            assert_eq!(parse(name).key, key, "{name}");
        }
    }

    #[test]
    fn punctuation() {
        assert_eq!(parse("Ctrl+/").key, Key::Unicode('/'));
        assert_eq!(parse("Cmd+,").key, Key::Unicode(','));
        assert_eq!(parse("Cmd+Shift+[").key, Key::Unicode('['));
        assert_eq!(parse("Ctrl+Plus"), hotkey(&[Key::Control], Key::Unicode('+')));
        assert_eq!(parse("Ctrl++"), hotkey(&[Key::Control], Key::Unicode('+')));
        assert_eq!(parse("Ctrl+Shift++"), hotkey(&[Key::Control, Key::Shift], Key::Unicode('+')));
        assert_eq!(parse("+"), hotkey(&[], Key::Unicode('+')));
        assert_eq!(parse("Ctrl+Minus").key, Key::Unicode('-'));
        assert_eq!(parse("Ctrl+-").key, Key::Unicode('-'));
        assert_eq!(parse("Ctrl+KeyA").key, Key::Unicode('a'));
        assert_eq!(parse("Digit7").key, Key::Unicode('7'));
    }

    #[test]
    fn media_keys() {
        assert_eq!(parse("MediaPlayPause").key, Key::MediaPlayPause);
        assert_eq!(parse("MediaTrackNext").key, Key::MediaNextTrack);
        assert_eq!(parse("MediaTrackPrevious").key, Key::MediaPrevTrack);
        assert_eq!(parse("AudioVolumeUp").key, Key::VolumeUp);
        assert_eq!(parse("VolumeDown").key, Key::VolumeDown);
        assert_eq!(parse("VolumeMute").key, Key::VolumeMute);
    }

    #[test]
    fn duplicate_modifiers_are_collapsed() {
        assert_eq!(parse("Ctrl+Control+Shift+A").modifiers, [Key::Control, Key::Shift]);
    }

    #[test]
    fn rejects_malformed_accelerators() {
        for text in [
            "",
            "  ",
            "Ctrl+",
            "Ctrl+Shift",
            "Ctrl",
            "Hyper+K",
            "Ctrl+Foo",
            "Ctrl++K",
            "Ctrl+Shift+",
            "F0",
            "F99",
            "+Ctrl",
        ] {
            assert!(text.parse::<Hotkey>().is_err(), "{text:?} should not parse");
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn extended_function_keys_off_macos() {
        assert_eq!(parse("F24").key, Key::F24);
        assert_eq!(parse("Insert").key, Key::Insert);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_has_no_f24() {
        assert!("F24".parse::<Hotkey>().is_err());
    }

    #[derive(Default)]
    struct Recorder {
        events: Vec<(Key, Direction)>,
        fail_click: bool,
    }

    impl Keyboard for Recorder {
        fn fast_text(&mut self, _: &str) -> InputResult<Option<()>> {
            Ok(None)
        }

        fn key(&mut self, key: Key, direction: Direction) -> InputResult<()> {
            if self.fail_click && direction == Direction::Click {
                return Err(InputError::Simulate("click failed"));
            }
            self.events.push((key, direction));
            Ok(())
        }

        fn raw(&mut self, _: u16, _: Direction) -> InputResult<()> {
            Ok(())
        }
    }

    #[test]
    fn press_holds_modifiers_around_the_click_and_releases_in_reverse() {
        let mut keyboard = Recorder::default();
        parse("Ctrl+Shift+M").press(&mut keyboard).unwrap();
        assert_eq!(
            keyboard.events,
            [
                (Key::Control, Direction::Press),
                (Key::Shift, Direction::Press),
                (Key::Unicode('m'), Direction::Click),
                (Key::Shift, Direction::Release),
                (Key::Control, Direction::Release),
            ]
        );
    }

    #[test]
    fn press_releases_modifiers_when_the_click_fails() {
        let mut keyboard = Recorder { fail_click: true, ..Recorder::default() };
        assert!(parse("Alt+Shift+Tab").press(&mut keyboard).is_err());
        assert_eq!(
            keyboard.events,
            [
                (Key::Alt, Direction::Press),
                (Key::Shift, Direction::Press),
                (Key::Shift, Direction::Release),
                (Key::Alt, Direction::Release),
            ]
        );
    }
}
