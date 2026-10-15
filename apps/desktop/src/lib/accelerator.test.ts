import assert from "node:assert/strict";
import { test } from "node:test";
import { formatAccelerator, keyName, normalizeAccelerator, toAccelerator, type KeyInput } from "./accelerator.ts";

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
