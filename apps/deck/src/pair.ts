// Pairing screen: 6-digit code entered on a big on-screen keypad (old devices have
// awful keyboards), plus an editable device name. Geometry is computed in JS; screens
// too short for the keypad and the info column scroll instead of overlapping them.

import { clamp, el, place, setVisible } from "./dom";
import type { Rect } from "./dom";
import { defaultDeviceName } from "./device";
import { ICONS } from "./icons";
import { tapButton } from "./tap";

const CODE_LENGTH = 6;
const ANSWER_TIMEOUT_MS = 8000;
const ERROR_COLOR = "#ff6b6b";
const INFO_GAP = 12;
/** Gap between info rows on short screens. */
const TIGHT_INFO_GAP = 6;
/** Smallest keypad height (4 rows of ~30 px keys); shorter screens scroll instead. */
const MIN_KEYS_H = 148;

type KeyValue = "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "clear" | "back";
const KEY_ORDER: KeyValue[] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "clear", "0", "back"];

interface InfoRow {
  node: HTMLElement;
  h: number;
  w: number;
  optional: boolean;
  /** Line height when it differs from `h` (multi-line rows). */
  lh?: number;
}

export interface PairScreen {
  show(hostName: string): void;
  hide(): void;
  /** The host rejected the code (or never answered). */
  fail(message: string): void;
  /** The host accepted the code; the deck follows shortly. */
  accepted(): void;
  relayout(): void;
}

/** `submit` sends the code and name to the host; returns false if there is no connection. */
export function createPairScreen(
  parent: HTMLElement,
  submit: (code: string, name: string) => boolean,
): PairScreen {
  const root = el("div", "pair", parent);
  const logo = el("img", "pair-logo", root);
  logo.src = "icons/icon-192.png";
  logo.alt = "";
  const title = el("div", "pair-title", root);
  const hint = el("div", "pair-hint", root);
  hint.textContent = "Enter the code shown in OmniPanel on your computer.";
  const codeRow = el("div", "pair-code", root);
  const boxes: HTMLElement[] = [];
  for (let i = 0; i < CODE_LENGTH; i++) boxes.push(el("div", "pair-digit", codeRow));
  const status = el("div", "pair-status", root);
  status.setAttribute("aria-live", "polite");
  const nameRow = el("div", "pair-name", root);
  const nameLabel = el("span", "pair-name-label", nameRow);
  nameLabel.textContent = "Device name";
  const nameValue = el("span", "pair-name-value", nameRow);
  const nameIcon = el("span", "pair-name-icon", nameRow);
  nameIcon.innerHTML = ICONS.edit;

  const keys: HTMLElement[] = [];
  for (let i = 0; i < KEY_ORDER.length; i++) {
    const value = KEY_ORDER[i];
    const key = el("div", "pair-key", root);
    if (value === "clear") {
      key.className += " small";
      key.textContent = "Clear";
    } else if (value === "back") {
      key.innerHTML = '<span class="pair-key-icon">' + ICONS.backspace + "</span>";
    } else {
      key.textContent = value;
    }
    tapButton(key, function () {
      press(value);
    }, value === "back" ? "Delete" : undefined);
    keys.push(key);
  }

  // Name editor: its own screen so the on-screen keyboard can't squash the keypad.
  const edit = el("div", "pair-edit", root);
  const editBox = el("div", "pair-edit-box", edit);
  const editLabel = el("div", "pair-edit-label", editBox);
  editLabel.id = "device-name-label";
  editLabel.textContent = "Device name";
  const input = el("input", "pair-input", editBox);
  input.setAttribute("aria-labelledby", editLabel.id);
  input.type = "text";
  input.maxLength = 32;
  input.setAttribute("autocomplete", "off");
  input.setAttribute("autocorrect", "off");
  input.setAttribute("autocapitalize", "words");
  input.setAttribute("spellcheck", "false");
  const done = el("div", "pair-done", editBox);
  done.textContent = "Done";
  setVisible(edit, false);

  let visible = false;
  let host = "";
  let code = "";
  let name = defaultDeviceName();
  let message = "";
  let messageIsError = false;
  let busy = false;
  let editing = false;
  let answerTimer = 0;
  setVisible(root, false);

  function paint(): void {
    title.textContent = host ? "Connect to " + host : "Connect this device";
    nameValue.textContent = name;
    for (let i = 0; i < CODE_LENGTH; i++) {
      boxes[i].textContent = i < code.length ? code.charAt(i) : "";
      const filled = i < code.length;
      boxes[i].className = "pair-digit" + (filled ? " filled" : "") + (i === code.length && !busy ? " next" : "");
    }
    status.textContent = message;
    status.style.color = messageIsError ? ERROR_COLOR : "";
    for (let i = 0; i < keys.length; i++) {
      keys[i].style.opacity = busy ? "0.4" : "";
    }
  }

  function setMessage(text: string, isError: boolean): void {
    message = text;
    messageIsError = isError;
  }

  function stopAnswerTimer(): void {
    window.clearTimeout(answerTimer);
    answerTimer = 0;
  }

  function trySubmit(): void {
    if (busy || code.length !== CODE_LENGTH) return;
    if (!submit(code, name)) {
      setMessage("Not connected. Retrying...", true);
      paint();
      return;
    }
    busy = true;
    setMessage("Connecting...", false);
    stopAnswerTimer();
    answerTimer = window.setTimeout(function () {
      fail("OmniPanel did not answer. Try again.");
    }, ANSWER_TIMEOUT_MS);
    paint();
  }

  function press(value: KeyValue): void {
    if (busy) return;
    if (value === "clear") code = "";
    else if (value === "back") code = code.slice(0, -1);
    else if (code.length < CODE_LENGTH) code += value;
    setMessage("", false);
    paint();
    if (code.length === CODE_LENGTH) trySubmit();
  }

  function fail(text: string): void {
    stopAnswerTimer();
    busy = false;
    code = "";
    setMessage(text, true);
    paint();
  }

  function openEditor(): void {
    if (busy) return;
    editing = true;
    input.value = name;
    root.scrollTop = 0; // the editor covers the unscrolled screen
    setVisible(edit, true);
    input.focus();
    // Replace the suggested name with one tap + typing.
    try {
      input.setSelectionRange(0, input.value.length);
    } catch (_e) {
      // some old browsers don't allow it
    }
  }

  function closeEditor(): void {
    if (!editing) return;
    editing = false;
    const trimmed = input.value.replace(/^\s+|\s+$/g, "");
    name = trimmed || defaultDeviceName();
    input.blur();
    setVisible(edit, false);
    window.scrollTo(0, 0); // iOS can leave the page scrolled after the keyboard closes
    paint();
    layout();
    if (visible) nameRow.focus(); // keyboards continue where they opened the editor
  }

  tapButton(nameRow, openEditor);
  tapButton(done, closeEditor);
  input.addEventListener("blur", closeEditor, false);
  input.addEventListener("keydown", function (e) {
    if (e.keyCode === 13) closeEditor();
  }, false);

  // Physical keyboards (desktop browsers, tablets with keyboards).
  document.addEventListener("keydown", function (e) {
    if (!visible || editing) return;
    if (e.keyCode >= 48 && e.keyCode <= 57) press(String(e.keyCode - 48) as KeyValue);
    else if (e.keyCode >= 96 && e.keyCode <= 105) press(String(e.keyCode - 96) as KeyValue);
    else if (e.keyCode === 8) press("back");
    else if (e.keyCode === 13) trySubmit();
  }, false);

  /** Code box geometry for an info column `width` wide. */
  function codeMetrics(width: number): { h: number; w: number; digitW: number; digitGap: number } {
    const h = clamp(Math.round(width / 9), 40, 58);
    const digitW = Math.round(h * 0.82);
    const digitGap = Math.round(h * 0.16);
    return { h: h, w: digitW * CODE_LENGTH + digitGap * (CODE_LENGTH - 1), digitW: digitW, digitGap: digitGap };
  }

  /** Info column rows at full size, top to bottom; layoutInfo drops the optional ones (h = 0) when space is tight. */
  function infoRows(width: number): InfoRow[] {
    const code = codeMetrics(width);
    return [
      { node: logo, h: 52, w: 52, optional: true },
      { node: title, h: 30, w: width, optional: false },
      { node: hint, h: 38, w: Math.min(width, 340), optional: true, lh: 19 },
      { node: codeRow, h: code.h, w: code.w, optional: false },
      { node: status, h: 36, w: width, optional: false, lh: 18 },
      { node: nameRow, h: 40, w: Math.min(width, 340), optional: false },
    ];
  }

  /** Height of the shown rows (h > 0) with `gap` between them. */
  function rowsHeight(rows: InfoRow[], gap: number): number {
    let sum = 0;
    let count = 0;
    for (let i = 0; i < rows.length; i++) {
      if (rows[i].h > 0) {
        sum += rows[i].h;
        count++;
      }
    }
    return sum + gap * (count - 1);
  }

  /** The smallest info column: required rows only, tight gaps. */
  function minInfoHeight(width: number): number {
    const rows = infoRows(width);
    for (let i = 0; i < rows.length; i++) if (rows[i].optional) rows[i].h = 0;
    return rowsHeight(rows, TIGHT_INFO_GAP);
  }

  /** Lays out the info column (logo, title, code, name) inside `area`, vertically centered. */
  function layoutInfo(area: Rect): void {
    const rows = infoRows(area.w);
    // Drop the optional rows (logo first, then hint) when space is tight, then tighten the gaps.
    for (let i = 0; i < rows.length && rowsHeight(rows, INFO_GAP) > area.h; i++) {
      if (rows[i].optional) rows[i].h = 0;
    }
    const gap = rowsHeight(rows, INFO_GAP) > area.h ? TIGHT_INFO_GAP : INFO_GAP;
    let y = area.y + Math.max(0, (area.h - rowsHeight(rows, gap)) / 2);
    for (let i = 0; i < rows.length; i++) {
      const row = rows[i];
      setVisible(row.node, row.h > 0);
      if (row.h === 0) continue;
      place(row.node, { x: area.x + (area.w - row.w) / 2, y: y, w: row.w, h: row.h });
      row.node.style.lineHeight = Math.round(row.lh || row.h) + "px";
      y += row.h + gap;
    }
    const code = codeMetrics(area.w);
    const digitFont = Math.round(code.h * 0.52);
    for (let i = 0; i < boxes.length; i++) {
      place(boxes[i], { x: i * (code.digitW + code.digitGap), y: 0, w: code.digitW, h: code.h });
      boxes[i].style.lineHeight = code.h - 4 + "px";
      boxes[i].style.fontSize = digitFont + "px";
    }
  }

  function layoutKeys(area: Rect): void {
    const gap = clamp(Math.round(area.w * 0.03), 6, 12);
    const keyW = (area.w - gap * 2) / 3;
    const keyH = (area.h - gap * 3) / 4;
    for (let i = 0; i < keys.length; i++) {
      const col = i % 3;
      const row = Math.floor(i / 3);
      const key = keys[i];
      place(key, { x: area.x + col * (keyW + gap), y: area.y + row * (keyH + gap), w: keyW, h: keyH });
      key.style.lineHeight = Math.round(keyH) + "px";
      key.style.fontSize = KEY_ORDER[i] === "clear" ? "15px" : Math.round(clamp(keyH * 0.44, 18, 34)) + "px";
      key.style.borderRadius = Math.round(clamp(keyH * 0.16, 8, 12)) + "px";
    }
  }

  function layout(): void {
    const w = root.clientWidth;
    const h = root.clientHeight;
    if (!w || !h) return;
    const margin = clamp(Math.round(Math.min(w, h) * 0.04), 10, 28);
    // Content height: taller than the screen when even the smallest layout doesn't fit (then it scrolls).
    let fullH = h;
    if (w > h * 1.15) {
      // Landscape: info on the left, keypad on the right.
      const keysW = clamp(Math.round(w * 0.42), 200, 380);
      const infoW = w - keysW - margin * 3;
      fullH = Math.max(h, minInfoHeight(infoW) + margin * 2);
      layoutKeys({ x: w - margin - keysW, y: margin, w: keysW, h: h - margin * 2 });
      layoutInfo({ x: margin, y: margin, w: infoW, h: fullH - margin * 2 });
    } else {
      // Portrait: info on top, keypad at the bottom. The keypad shrinks (down to MIN_KEYS_H)
      // so the info's required rows always fit above it.
      const infoW = w - margin * 2;
      const infoMin = minInfoHeight(infoW);
      const keysW = Math.min(infoW, 380);
      const keysH = Math.max(MIN_KEYS_H, Math.min(Math.round((keysW / 3) * 0.62) * 4 + 24, Math.round(h * 0.46), h - margin * 3 - infoMin));
      fullH = Math.max(h, infoMin + keysH + margin * 3);
      layoutKeys({ x: (w - keysW) / 2, y: fullH - margin - keysH, w: keysW, h: keysH });
      layoutInfo({ x: margin, y: margin, w: infoW, h: fullH - keysH - margin * 3 });
    }
    // Only a screen that overflows lets touches scroll it (main.ts blocks touch moves elsewhere).
    root.className = fullH > h ? "pair scroll" : "pair";
    nameIcon.style.width = nameIcon.style.height = "16px";
  }

  return {
    show: function (hostName) {
      host = hostName;
      if (!visible) {
        visible = true;
        code = "";
        busy = false;
        setMessage("", false);
        setVisible(root, true);
      }
      paint();
      layout();
    },
    hide: function () {
      visible = false;
      stopAnswerTimer();
      busy = false;
      code = "";
      setMessage("", false);
      if (editing) closeEditor();
      setVisible(root, false);
    },
    fail: fail,
    accepted: function () {
      stopAnswerTimer();
      busy = true;
      setMessage("Connected. Loading buttons...", false);
      paint();
    },
    relayout: function () {
      if (visible) layout();
    },
  };
}
