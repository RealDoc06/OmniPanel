import assert from "node:assert/strict";
import { test } from "node:test";
import { formatAccelerator, isAccelerator, keyName, normalizeAccelerator, toAccelerator, type KeyInput } from "./accelerator.ts";

const key = (code: string, mods: Partial<KeyInput> = {}): KeyInput => ({
  code,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

test("maps physical keys to accelerator names", () => {
  assert.equal(keyName("KeyM"), "M");
  assert.equal(keyName("Digit7"), "7");
  assert.equal(keyName("F13"), "F13");
  assert.equal(keyName("F25"), null);
  assert.equal(keyName("ArrowLeft"), "Left");
  assert.equal(keyName("Numpad3"), "Numpad3");
  assert.equal(keyName("Comma"), "Comma");
  assert.equal(keyName("ShiftLeft"), null);
  assert.equal(keyName("IntlBackslash"), null);
});

test("uses CmdOrCtrl for the platform command key", () => {
  assert.equal(toAccelerator(key("KeyM", { metaKey: true, shiftKey: true }), true), "CmdOrCtrl+Shift+M");
  assert.equal(toAccelerator(key("KeyM", { ctrlKey: true, shiftKey: true }), false), "CmdOrCtrl+Shift+M");
  assert.equal(toAccelerator(key("KeyK", { ctrlKey: true, metaKey: true }), true), "CmdOrCtrl+Ctrl+K");
  assert.equal(toAccelerator(key("KeyK", { ctrlKey: true, metaKey: true }), false), "CmdOrCtrl+Super+K");
  assert.equal(toAccelerator(key("Digit1", { altKey: true }), false), "Alt+1");
  assert.equal(toAccelerator(key("F13"), true), "F13");
});

test("returns null while only modifiers are held", () => {
  assert.equal(toAccelerator(key("ShiftLeft", { shiftKey: true }), true), null);
  assert.equal(toAccelerator(key("MetaLeft", { metaKey: true }), true), null);
});

test("records supported numpad equivalents and rejects unsupported engine keys", () => {
  assert.equal(toAccelerator(key("NumpadEnter"), true), "Enter");
  assert.equal(toAccelerator(key("NumpadEqual", { ctrlKey: true }), false), "CmdOrCtrl+Equal");
  assert.equal(toAccelerator(key("ScrollLock"), false), null);
  for (const name of ["ScrollLock", "NumpadEnter", "NumpadEqual"]) {
    assert.equal(isAccelerator(name), false, name);
    assert.equal(isAccelerator(`Ctrl+${name}`), false, name);
  }
  for (const name of ["Enter", "Equal", "NumAdd", "NumpadPlus", "MediaPrev", "VolumeMute"]) {
    assert.equal(isAccelerator(name), true, name);
  }
});

test("honors the engine's macOS key restrictions", () => {
  for (const name of ["Insert", "Pause", "PrintScreen", "NumLock", "MediaStop", "F21", "F24"]) {
    assert.equal(isAccelerator(name, true), false, name);
    assert.equal(toAccelerator(key(name), true), null, name);
    assert.equal(isAccelerator(name, false), true, name);
  }
  assert.equal(isAccelerator("Ctrl+PrtSc", true), false);
  assert.equal(isAccelerator("F20", true), true);
});

test("normalizes aliases and modifier order", () => {
  assert.equal(normalizeAccelerator("Shift+CmdOrCtrl+M"), normalizeAccelerator("CommandOrControl+Shift+m"));
  assert.equal(normalizeAccelerator("Control+Option+Up"), "ctrl+alt+arrowup");
  assert.notEqual(normalizeAccelerator("Ctrl+M"), normalizeAccelerator("CmdOrCtrl+M"));
});

test("formats for display", () => {
  assert.equal(formatAccelerator("CmdOrCtrl+Shift+M", true), "⌘⇧M");
  assert.equal(formatAccelerator("CmdOrCtrl+Shift+M", false), "Ctrl+Shift+M");
  assert.equal(formatAccelerator("Super+Alt+Up", false), "Win+Alt+Up");
});

test("validates typed accelerators", () => {
  for (const text of ["CmdOrCtrl+Shift+M", "control+option+Return", "F13", "Cmd+KeyA", "Digit7", "Ctrl+Plus", "Ctrl++", "+", "Alt+,", "Spacebar"]) {
    assert.equal(isAccelerator(text), true, text);
  }
  for (const text of ["banana+++NotAKey", "Shift", "Ctrl+Shift", "", "Ctrl+", "Ctrl+++", "Ctrl+A+B", "Ctrl+NotAKey", "\u0000"]) {
    assert.equal(isAccelerator(text), false, text);
  }
  assert.equal(normalizeAccelerator("Ctrl++"), normalizeAccelerator("Control+Plus"));
});
