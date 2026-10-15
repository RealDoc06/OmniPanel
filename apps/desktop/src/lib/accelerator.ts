// Keyboard events -> Tauri accelerator strings ("CmdOrCtrl+Shift+M"), as accepted by
// the global-shortcut plugin and by the "Press keys" action. Also validates and
// formats typed accelerators. Pure functions, unit tested.

export interface KeyInput {
  code: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

const MODIFIER_CODES = new Set([
  "ShiftLeft",
  "ShiftRight",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "MetaLeft",
  "MetaRight",
  "OSLeft",
  "OSRight",
  "Fn",
  "FnLock",
]);

/** `KeyboardEvent.code` values the accelerator parser understands verbatim. */
const PASSTHROUGH = new Set([
  "Space",
  "Enter",
  "Tab",
  "Backspace",
  "Delete",
  "Escape",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "Insert",
  "Minus",
  "Equal",
  "BracketLeft",
  "BracketRight",
  "Backslash",
  "Semicolon",
  "Quote",
  "Comma",
  "Period",
  "Slash",
  "Backquote",
  "CapsLock",
  "PrintScreen",
  "Pause",
  "NumLock",
  "NumpadAdd",
  "NumpadSubtract",
  "NumpadMultiply",
  "NumpadDivide",
  "NumpadDecimal",
  "AudioVolumeUp",
  "AudioVolumeDown",
  "AudioVolumeMute",
  "MediaPlayPause",
  "MediaStop",
  "MediaTrackNext",
  "MediaTrackPrevious",
]);

/** Accelerator key name for a physical key, or null for modifiers / unsupported keys. */
export function keyName(code: string, mac = false): string | null {
  if (MODIFIER_CODES.has(code)) return null;
  if (mac && MAC_UNSUPPORTED.has(code.toLowerCase())) return null;
  if (code === "NumpadEnter") return "Enter";
  if (code === "NumpadEqual") return "Equal";
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1] ?? null;
  const digit = /^Digit(\d)$/.exec(code);
  if (digit) return digit[1] ?? null;
  if (/^F([1-9]|1\d|2[0-4])$/.test(code)) return code;
  if (/^Numpad\d$/.test(code)) return code;
  const arrow = /^Arrow(Up|Down|Left|Right)$/.exec(code);
  if (arrow) return arrow[1] ?? null;
  return PASSTHROUGH.has(code) ? code : null;
}

/**
 * Modifiers in canonical order. The platform "command" key (Cmd on macOS, Ctrl
 * elsewhere) becomes `CmdOrCtrl` so profiles work on every OS.
 */
export function modifiers(e: KeyInput, mac: boolean): string[] {
  const mods: string[] = [];
  if (mac ? e.metaKey : e.ctrlKey) mods.push("CmdOrCtrl");
  if (mac && e.ctrlKey) mods.push("Ctrl");
  if (!mac && e.metaKey) mods.push("Super");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  return mods;
}

/** Full accelerator for a key press, or null while only modifiers are held. */
export function toAccelerator(e: KeyInput, mac: boolean): string | null {
  const key = keyName(e.code, mac);
  return key ? [...modifiers(e, mac), key].join("+") : null;
}

const ALIASES: Record<string, string> = {
  commandorcontrol: "cmdorctrl",
  commandorctrl: "cmdorctrl",
  cmdorcontrol: "cmdorctrl",
  control: "ctrl",
  option: "alt",
  opt: "alt",
  command: "super",
  cmd: "super",
  meta: "super",
  win: "super",
  windows: "super",
  up: "arrowup",
  down: "arrowdown",
  left: "arrowleft",
  right: "arrowright",
  esc: "escape",
  return: "enter",
  del: "delete",
  spacebar: "space",
  pgup: "pageup",
  pgdn: "pagedown",
  pgdown: "pagedown",
  uparrow: "arrowup",
  downarrow: "arrowdown",
  leftarrow: "arrowleft",
  rightarrow: "arrowright",
  equals: "equal",
  dot: "period",
  apostrophe: "quote",
  backtick: "backquote",
  grave: "backquote",
  leftbracket: "bracketleft",
  rightbracket: "bracketright",
  ins: "insert",
  prtsc: "printscreen",
  printscr: "printscreen",
  numadd: "numpadadd",
  numplus: "numpadadd",
  numpadplus: "numpadadd",
  numsubtract: "numpadsubtract",
  numminus: "numpadsubtract",
  numpadminus: "numpadsubtract",
  nummultiply: "numpadmultiply",
  numdivide: "numpaddivide",
  numdecimal: "numpaddecimal",
  playpause: "mediaplaypause",
  mediaplay: "mediaplaypause",
  medianext: "mediatracknext",
  nexttrack: "mediatracknext",
  mediaprevious: "mediatrackprevious",
  mediaprev: "mediatrackprevious",
  mediatrackprev: "mediatrackprevious",
  previoustrack: "mediatrackprevious",
  volumeup: "audiovolumeup",
  volumedown: "audiovolumedown",
  volumemute: "audiovolumemute",
  mute: "audiovolumemute",
};

// Keep platform exceptions in sync with omni-actions/src/keys.rs::parse_key.
const MAC_UNSUPPORTED = new Set(["insert", "pause", "printscreen", "numlock", "mediastop", "f21", "f22", "f23", "f24"]);

export const isModifierKey = (code: string): boolean => MODIFIER_CODES.has(code);

const MOD_ORDER = ["cmdorctrl", "ctrl", "super", "alt", "shift"];

/** Like the host parser, a literal plus is written `Plus` or a trailing `++`. */
function acceleratorParts(accelerator: string): string[] {
  const text = accelerator.trim();
  if (text === "+") return ["Plus"];
  if (text.endsWith("++")) return [...text.slice(0, -2).split("+"), "Plus"];
  return text.split("+");
}

/** Canonical form for comparing accelerators ("Shift+Ctrl+A" equals "control+shift+a"). */
export function normalizeAccelerator(accelerator: string): string {
  const parts = acceleratorParts(accelerator)
    .map((p) => p.trim().toLowerCase())
    .filter(Boolean)
    .map((p) => ALIASES[p] ?? p);
  const mods = parts.filter((p) => MOD_ORDER.includes(p)).sort((a, b) => MOD_ORDER.indexOf(a) - MOD_ORDER.indexOf(b));
  const keys = parts.filter((p) => !MOD_ORDER.includes(p));
  return [...new Set(mods), ...keys].join("+");
}

/** Lowercase key names an accelerator may end with: the names `keyName` produces, plus alias targets. */
const KEY_NAMES = new Set([
  ..."abcdefghijklmnopqrstuvwxyz0123456789",
  ...Array.from({ length: 24 }, (_, i) => `f${i + 1}`),
  ...Array.from({ length: 10 }, (_, i) => `numpad${i}`),
  "arrowup",
  "arrowdown",
  "arrowleft",
  "arrowright",
  "plus",
  ...[...PASSTHROUGH].map((code) => code.toLowerCase()),
]);

/**
 * True for "modifiers + one key" ("CmdOrCtrl+Shift+M", "F13", "alt+,"), the shape the
 * host's key parser accepts. Any single character also counts as a key, as on the host.
 */
export function isAccelerator(accelerator: string, mac = false): boolean {
  const parts = acceleratorParts(accelerator).map((p) => {
    const part = p.trim().toLowerCase();
    return ALIASES[part] ?? part;
  });
  const key = parts.pop();
  if (!key || parts.some((p) => !MOD_ORDER.includes(p))) return false;
  if (mac && MAC_UNSUPPORTED.has(key)) return false;
  return KEY_NAMES.has(key) || /^(?:key[a-z]|digit\d|num\d)$/.test(key) || ([...key].length === 1 && !/[\s\p{C}]/u.test(key));
}

/** Display names keyed by normalized part (see ALIASES). */
const MAC_SYMBOLS: Record<string, string> = {
  cmdorctrl: "⌘",
  super: "⌘",
  ctrl: "⌃",
  alt: "⌥",
  shift: "⇧",
  arrowup: "↑",
  arrowdown: "↓",
  arrowleft: "←",
  arrowright: "→",
  enter: "↩",
  backspace: "⌫",
  delete: "⌦",
  escape: "⎋",
  tab: "⇥",
};

const OTHER_NAMES: Record<string, string> = { cmdorctrl: "Ctrl", super: "Win", ctrl: "Ctrl", alt: "Alt", shift: "Shift" };

/** Display form: "⌘⇧M" on macOS, "Ctrl+Shift+M" elsewhere. Typed spellings ("cmd+m") format the same. */
export function formatAccelerator(accelerator: string, mac: boolean): string {
  const parts = acceleratorParts(accelerator)
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => {
      const name = ALIASES[p.toLowerCase()] ?? p.toLowerCase();
      return (mac ? MAC_SYMBOLS : OTHER_NAMES)[name] ?? (p.length === 1 ? p.toUpperCase() : p);
    });
  return parts.join(mac ? "" : "+");
}
