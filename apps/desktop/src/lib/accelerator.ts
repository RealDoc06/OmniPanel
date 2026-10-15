// Keyboard events -> Tauri accelerator strings ("CmdOrCtrl+Shift+M"), as accepted by
// the global-shortcut plugin and by the hotkey action. Pure functions, unit tested.

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
  "ScrollLock",
  "Pause",
  "NumLock",
  "NumpadAdd",
  "NumpadSubtract",
  "NumpadMultiply",
  "NumpadDivide",
  "NumpadDecimal",
  "NumpadEnter",
  "NumpadEqual",
  "AudioVolumeUp",
  "AudioVolumeDown",
  "AudioVolumeMute",
  "MediaPlayPause",
  "MediaStop",
  "MediaTrackNext",
  "MediaTrackPrevious",
]);

/** Accelerator key name for a physical key, or null for modifiers / unsupported keys. */
export function keyName(code: string): string | null {
  if (MODIFIER_CODES.has(code)) return null;
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
  const key = keyName(e.code);
  return key ? [...modifiers(e, mac), key].join("+") : null;
}

const ALIASES: Record<string, string> = {
  commandorcontrol: "cmdorctrl",
  commandorctrl: "cmdorctrl",
  cmdorcontrol: "cmdorctrl",
  control: "ctrl",
  option: "alt",
  command: "super",
  cmd: "super",
  meta: "super",
  win: "super",
  up: "arrowup",
  down: "arrowdown",
  left: "arrowleft",
  right: "arrowright",
  esc: "escape",
  return: "enter",
};

const MOD_ORDER = ["cmdorctrl", "ctrl", "super", "alt", "shift"];

/** Canonical form for comparing accelerators ("Shift+Ctrl+A" equals "control+shift+a"). */
export function normalizeAccelerator(accelerator: string): string {
  const parts = accelerator
    .split("+")
    .map((p) => p.trim().toLowerCase())
    .filter(Boolean)
    .map((p) => ALIASES[p] ?? p);
  const mods = parts.filter((p) => MOD_ORDER.includes(p)).sort((a, b) => MOD_ORDER.indexOf(a) - MOD_ORDER.indexOf(b));
  const keys = parts.filter((p) => !MOD_ORDER.includes(p));
  return [...new Set(mods), ...keys].join("+");
}

const MAC_SYMBOLS: Record<string, string> = {
  CmdOrCtrl: "⌘",
  Cmd: "⌘",
  Super: "⌘",
  Ctrl: "⌃",
  Alt: "⌥",
  Shift: "⇧",
  Up: "↑",
  Down: "↓",
  Left: "←",
  Right: "→",
  Enter: "↩",
  Backspace: "⌫",
  Delete: "⌦",
  Escape: "⎋",
  Tab: "⇥",
  Space: "Space",
};

/** Display form: "⌘⇧M" on macOS, "Ctrl+Shift+M" elsewhere. */
export function formatAccelerator(accelerator: string, mac: boolean): string {
  const parts = accelerator.split("+").filter(Boolean);
  if (mac) return parts.map((p) => MAC_SYMBOLS[p] ?? p).join("");
  return parts.map((p) => (p === "CmdOrCtrl" ? "Ctrl" : p === "Super" ? "Win" : p)).join("+");
}
