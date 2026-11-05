// The deck grid: builds the DOM once per layout, patches only changed controls on state
// messages, and turns touches and keys into press / slide / page-swipe events.
// All geometry is computed here in JS (no CSS grid / flexbox / variables on old WebViews).
// Controls are ARIA buttons / sliders in reading order, so keyboards and screen readers can use them.

import type { ControlState, DeckControl, DeckIcon, DeckLayout, NavTarget, ThemeColors } from "@omni/protocol";
import { readableOn } from "./color";
import { clamp, clearChildren, el, newMap, place, px, unreachable } from "./dom";
import type { Rect } from "./dom";
import { svg } from "./icons";
import { isActivationKey } from "./tap";
import { fitLabel, lineHeight, textWidth } from "./text";

/** Distance a finger may wander before a tap turns into a drag / swipe. */
const SLOP = 10;
/** ~15 slide messages per second while dragging. */
const SLIDE_INTERVAL_MS = 66;
/** Keeps the pressed look visible for very quick taps. */
const MIN_PRESS_MS = 70;
/** After releasing a slider, host echoes of older values are held back this long. */
const SLIDER_SETTLE_MS = 300;
/** Mouse events right after a touch are the browser's emulation of it. */
const GHOST_MOUSE_MS = 1000;
const MOUSE_ID = -1;
const DISABLED_OPACITY = "0.4";
/** One arrow key press moves a focused slider by this much. */
const SLIDER_KEY_STEP = 0.05;
const EMPTY_TEXT = "No buttons on this page yet. Add some in OmniPanel on your computer.";

export interface DeckEvents {
  press(id: string): void;
  slide(id: string, value: number): void;
  nav(to: NavTarget): void;
}

export interface DeckView {
  setLayout(layout: DeckLayout): void;
  /** Patches the listed controls; unknown ids (other pages) are ignored. */
  applyStates(states: ControlState[]): void;
  /** Rebuilds the current layout, e.g. after the stage was resized. */
  relayout(): void;
  /** While offline touches are ignored (the host can't hear them). */
  setOnline(online: boolean): void;
  /** Aborts touches in flight (page hidden, layout replaced). */
  cancelGestures(): void;
}

interface CellBase {
  ctl: DeckControl;
  rect: Rect;
  theme: ThemeColors;
  el: HTMLElement;
  iconEl: HTMLElement;
  labelEl: HTMLElement;
  state: ControlState;
  /** What is currently drawn, so repaints only touch the DOM when it changed. */
  drawnIcon: string;
  drawnLabel: string;
}

interface ButtonCell extends CellBase {
  kind: "button";
  hl: HTMLElement;
  pressed: boolean;
}

interface SliderCell extends CellBase {
  kind: "slider";
  vertical: boolean;
  fill: HTMLElement;
  valueEl: HTMLElement;
  value: number;
  drawnValue: string;
  dragging: boolean;
  /** Host values are ignored until this time (the host is still echoing our own drag). */
  settleUntil: number;
  /** Newest host state received while ignoring, applied once settled. */
  held: ControlState | null;
  settleTimer: number;
  sendTimer: number;
  lastSent: number;
  lastSentValue: number;
}

type Cell = ButtonCell | SliderCell;

interface Gesture {
  id: number;
  cell: Cell | null;
  /** Client coordinates where the gesture started and the stage's origin at that time. */
  startX: number;
  startY: number;
  originX: number;
  originY: number;
  startedAt: number;
  moved: boolean;
  /** The press was already sent on touch-down (single-page decks). */
  fired: boolean;
  swiped: boolean;
}

function iconKey(icon: DeckIcon | null): string {
  if (!icon) return "";
  switch (icon.type) {
    case "svg":
      return "s" + icon.markup;
    case "emoji":
      return "e" + icon.value;
    case "image":
      return "i" + icon.url;
    default:
      unreachable(icon);
      return "";
  }
}

/** Fills an icon box with the icon's content; sizing is done by `sizeIcon`. */
function drawIcon(box: HTMLElement, icon: DeckIcon | null): void {
  box.innerHTML = "";
  box.style.backgroundImage = "";
  if (!icon) return;
  switch (icon.type) {
    case "svg":
      box.innerHTML = svg(icon.markup);
      break;
    case "emoji":
      box.textContent = icon.value;
      break;
    case "image":
      // A background (not <img>) so `contain` scaling works without object-fit (Chrome < 31, iOS < 10).
      box.style.backgroundImage = 'url("' + icon.url.replace(/["\\\n\r]/g, encodeURIComponent) + '")';
      break;
    default:
      unreachable(icon);
  }
}

/** Positions an icon box (a centered square) and scales emoji text to it. */
function sizeIcon(box: HTMLElement, icon: DeckIcon | null, left: number, top: number, size: number): void {
  box.style.display = icon ? "" : "none";
  if (!icon) return;
  place(box, { x: left, y: top, w: size, h: size });
  box.className = "ico " + icon.type;
  if (icon.type === "emoji") {
    box.style.fontSize = px(size * 0.8);
    box.style.lineHeight = px(size);
  }
}

/** Positions a text box (`lines` lines of `size` px text) and sets its font metrics. */
function placeText(box: HTMLElement, x: number, y: number, w: number, size: number, lines: number): void {
  const lh = lineHeight(size);
  place(box, { x: x, y: y, w: w, h: lh * lines });
  box.style.fontSize = px(size);
  box.style.lineHeight = px(lh);
}

export function createDeck(frame: HTMLElement, stage: HTMLElement, events: DeckEvents): DeckView {
  let layout: DeckLayout | null = null;
  let cells: Cell[] = [];
  let byId = newMap<Cell>();
  /** Newest known state per control id, so a relayout doesn't revert to the layout's snapshot. */
  let states = newMap<ControlState>();
  let gap = 8;
  let online = true;
  let gestures: Gesture[] = [];
  let lastTouchAt = 0;
  let lastGestureAt = 0;

  // ---- building -----------------------------------------------------------------------

  function canSwipe(): boolean {
    return layout !== null && layout.pageCount > 1;
  }

  function haptic(): void {
    if (layout && layout.settings.haptics && typeof navigator.vibrate === "function") navigator.vibrate(10);
  }

  /** Stage-relative rect for a control; null if it lies outside the grid. */
  function controlRect(ctl: DeckControl, lay: DeckLayout, width: number, height: number): Rect | null {
    const cols = Math.max(1, lay.cols);
    const rows = Math.max(1, lay.rows);
    if (ctl.x >= cols || ctl.y >= rows) return null;
    const cw = (width - gap * 2 + gap) / cols;
    const ch = (height - gap * 2 + gap) / rows;
    const x0 = Math.round(gap + ctl.x * cw);
    const y0 = Math.round(gap + ctl.y * ch);
    const x1 = Math.round(gap + Math.min(cols, ctl.x + Math.max(1, ctl.w)) * cw) - gap;
    const y1 = Math.round(gap + Math.min(rows, ctl.y + Math.max(1, ctl.h)) * ch) - gap;
    return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
  }

  /** The cell containing `node` (clicks can target a label or icon inside it). */
  function cellOf(node: EventTarget | null): Cell | null {
    if (!(node instanceof Node)) return null;
    for (let i = 0; i < cells.length; i++) if (cells[i].el.contains(node)) return cells[i];
    return null;
  }

  function rebuild(): void {
    cancelGestures();
    // Keyboard focus survives a rebuild: back on the same control, else on the first one.
    const focused = cellOf(document.activeElement);
    clearChildren(stage);
    cells = [];
    byId = newMap<Cell>();
    if (!layout) return;
    const width = stage.clientWidth;
    const height = stage.clientHeight;
    if (width < 20 || height < 20) return;
    gap = clamp(Math.round(Math.min(width / Math.max(1, layout.cols), height / Math.max(1, layout.rows)) * 0.09), 6, 16);
    // DOM (and so Tab) order is reading order: rows top to bottom, then columns.
    const ordered = layout.controls.slice().sort(function (a, b) {
      return a.y - b.y || a.x - b.x;
    });
    for (let i = 0; i < ordered.length; i++) {
      const ctl = ordered[i];
      const rect = controlRect(ctl, layout, width, height);
      if (!rect || rect.w < 4 || rect.h < 4) continue;
      const state = states[ctl.id] || ctl.state;
      const radius = px(Math.min(layout.radius, Math.min(rect.w, rect.h) / 2));
      const kind = ctl.kind;
      let cell: Cell;
      switch (kind.type) {
        case "button":
          cell = buildButton(ctl, rect, state, layout.theme, radius);
          break;
        case "slider":
          cell = buildSlider(ctl, rect, state, layout.theme, radius, kind.vertical);
          break;
        default:
          unreachable(kind);
          continue;
      }
      cells.push(cell);
      byId[ctl.id] = cell;
    }
    if (ordered.length === 0) {
      const empty = el("div", "empty", stage);
      empty.textContent = EMPTY_TEXT;
      empty.style.color = layout.theme.textMuted;
    }
    if (focused) {
      const again = byId[focused.ctl.id] || cells[0];
      if (again) again.el.focus();
    }
  }

  /** A focusable cell root with the matching ARIA role (button / slider). */
  function buildBase(kind: Cell["kind"], rect: Rect, radius: string): { root: HTMLElement; iconEl: HTMLElement; labelEl: HTMLElement } {
    const root = el("div", kind === "button" ? "btn" : "slider", stage);
    root.setAttribute("role", kind);
    root.tabIndex = 0;
    place(root, rect);
    root.style.borderRadius = radius;
    return { root: root, iconEl: el("div", "ico", root), labelEl: el("div", "lbl", root) };
  }

  // ---- buttons ------------------------------------------------------------------------

  function buildButton(ctl: DeckControl, rect: Rect, state: ControlState, theme: ThemeColors, radius: string): ButtonCell {
    const base = buildBase("button", rect, radius);
    const cell: ButtonCell = {
      kind: "button",
      ctl: ctl,
      rect: rect,
      theme: theme,
      el: base.root,
      iconEl: base.iconEl,
      labelEl: base.labelEl,
      hl: el("div", "hl", base.root),
      state: state,
      drawnIcon: "\u0000",
      drawnLabel: "\u0000",
      pressed: false,
    };
    cell.hl.style.borderRadius = radius;
    paintButton(cell);
    return cell;
  }

  /** Active look: background from `active.bg`; keep the foreground readable on it. */
  function paintButton(c: ButtonCell): void {
    const ctl = c.ctl;
    const st = c.state;
    const bg = st.active ? ctl.active.bg || ctl.bg || c.theme.button : ctl.bg || c.theme.button;
    const fg = readableOn(bg, ctl.fg || c.theme.buttonText);
    c.el.style.backgroundColor = bg;
    c.el.style.color = fg;
    c.el.style.opacity = st.disabled ? DISABLED_OPACITY : "";
    c.el.setAttribute("aria-disabled", String(st.disabled));
    const icon = st.active && ctl.active.icon ? ctl.active.icon : ctl.icon;
    const label = st.label !== null ? st.label : st.active && ctl.active.label !== null ? ctl.active.label : ctl.label;
    const key = iconKey(icon);
    if (key !== c.drawnIcon || label !== c.drawnLabel) {
      if (key !== c.drawnIcon) drawIcon(c.iconEl, icon);
      c.drawnIcon = key;
      c.drawnLabel = label;
      // The full label, also when it is hidden or shortened on screen.
      c.el.setAttribute("aria-label", label);
      layoutButton(c, icon, label);
    }
  }

  /**
   * Icon + label geometry. Text-only buttons center a larger label; with an icon the
   * label sits at `labelPosition` (bottom / top / below the centered icon).
   */
  function layoutButton(c: ButtonCell, icon: DeckIcon | null, text: string): void {
    const w = c.rect.w;
    const h = c.rect.h;
    const base = Math.min(w, h);
    const pad = Math.max(4, Math.round(base * 0.09));
    const innerW = w - pad * 2;
    const innerH = h - pad * 2;
    const position = c.ctl.labelPosition;
    const showLabel = position !== "hidden" && text !== "";

    let lines: string[] = [];
    let fontSize = 0;
    let labelH = 0;
    if (showLabel) {
      const textOnly = icon === null;
      const fit = fitLabel(
        text,
        innerW,
        textOnly ? clamp(Math.round(base * 0.26), 13, 44) : clamp(Math.round(base * 0.18), 11, 28),
        9,
        textOnly ? 3 : 2,
      );
      lines = fit.lines;
      fontSize = fit.size;
      labelH = lineHeight(fontSize) * lines.length;
    }
    c.labelEl.style.display = showLabel ? "" : "none";
    if (showLabel) c.labelEl.textContent = lines.join("\n");

    const spacing = icon && showLabel ? Math.round(pad * 0.5) : 0;
    const areaH = Math.max(0, innerH - labelH - spacing);
    const iconSize = icon ? Math.floor(Math.min(innerW, areaH) * (showLabel ? 0.92 : 0.64)) : 0;
    let iconTop = (h - iconSize) / 2;
    let labelTop = (h - labelH) / 2;
    if (icon && showLabel) {
      if (position === "top") {
        labelTop = pad;
        iconTop = pad + labelH + spacing + (areaH - iconSize) / 2;
      } else if (position === "center") {
        const total = iconSize + spacing + labelH;
        iconTop = (h - total) / 2;
        labelTop = iconTop + iconSize + spacing;
      } else {
        iconTop = pad + (areaH - iconSize) / 2;
        labelTop = h - pad - labelH;
      }
    }
    sizeIcon(c.iconEl, icon, (w - iconSize) / 2, iconTop, iconSize);
    if (showLabel) placeText(c.labelEl, pad, labelTop, innerW, fontSize, lines.length);
  }

  function setPressed(c: ButtonCell, pressed: boolean): void {
    if (c.pressed === pressed) return;
    c.pressed = pressed;
    c.el.className = pressed ? "btn pressed" : "btn";
  }

  // ---- sliders ------------------------------------------------------------------------

  function buildSlider(ctl: DeckControl, rect: Rect, state: ControlState, theme: ThemeColors, radius: string, vertical: boolean): SliderCell {
    const base = buildBase("slider", rect, radius);
    base.root.setAttribute("aria-orientation", vertical ? "vertical" : "horizontal");
    base.root.setAttribute("aria-valuemin", "0");
    base.root.setAttribute("aria-valuemax", "100");
    // The fill goes underneath the icon and text.
    const fill = el("div", vertical ? "fill v" : "fill h");
    base.root.insertBefore(fill, base.root.firstChild);
    base.labelEl.className = vertical ? "lbl v" : "lbl h";
    const cell: SliderCell = {
      kind: "slider",
      vertical: vertical,
      ctl: ctl,
      rect: rect,
      theme: theme,
      el: base.root,
      iconEl: base.iconEl,
      labelEl: base.labelEl,
      fill: fill,
      valueEl: el("div", vertical ? "val v" : "val h", base.root),
      state: state,
      drawnIcon: "\u0000",
      drawnLabel: "\u0000",
      drawnValue: "",
      value: state.value !== null ? clamp(state.value, 0, 1) : 0,
      dragging: false,
      settleUntil: 0,
      held: null,
      settleTimer: 0,
      sendTimer: 0,
      lastSent: 0,
      lastSentValue: -1,
    };
    paintSlider(cell);
    setSliderValue(cell, cell.value);
    return cell;
  }

  function sliderLabel(c: SliderCell): string {
    return c.state.active && c.ctl.active.label !== null ? c.ctl.active.label : c.ctl.label;
  }

  /** Text next to the bar: the host's dynamic label when it sends one, else the live percentage. */
  function sliderValueText(c: SliderCell): string {
    if (c.state.label !== null && !c.dragging) return c.state.label;
    return Math.round(c.value * 100) + "%";
  }

  function paintSlider(c: SliderCell): void {
    const ctl = c.ctl;
    const st = c.state;
    c.el.style.backgroundColor = ctl.bg || c.theme.button;
    c.el.style.color = ctl.fg || c.theme.buttonText;
    c.el.style.opacity = st.disabled ? DISABLED_OPACITY : "";
    c.el.setAttribute("aria-disabled", String(st.disabled));
    c.fill.style.backgroundColor = st.active ? ctl.active.bg || c.theme.accent : c.theme.accent;
    const icon = st.active && ctl.active.icon ? ctl.active.icon : ctl.icon;
    const key = iconKey(icon);
    const label = sliderLabel(c);
    if (key !== c.drawnIcon || label !== c.drawnLabel) {
      if (key !== c.drawnIcon) drawIcon(c.iconEl, icon);
      c.drawnIcon = key;
      c.drawnLabel = label;
      c.el.setAttribute("aria-label", label);
      layoutSlider(c, icon, label);
    }
  }

  /** Vertical: icon, label, value at the bottom. Horizontal: icon left, label above the value (slim bars: one line). */
  function layoutSlider(c: SliderCell, icon: DeckIcon | null, text: string): void {
    const w = c.rect.w;
    const h = c.rect.h;
    const showLabel = c.ctl.labelPosition !== "hidden" && text !== "";
    c.labelEl.style.display = showLabel ? "" : "none";
    if (c.vertical) {
      const pad = Math.max(6, Math.round(Math.min(w, h) * 0.12));
      const innerW = w - pad * 2;
      const valueSize = clamp(Math.round(w * 0.22), 12, 24);
      const valueTop = h - pad - lineHeight(valueSize);
      placeText(c.valueEl, pad, valueTop, innerW, valueSize, 1);
      const iconSize = icon ? clamp(Math.round(w * 0.4), 18, 44) : 0;
      sizeIcon(c.iconEl, icon, (w - iconSize) / 2, pad, iconSize);
      if (showLabel) {
        const fit = fitLabel(text, innerW, clamp(Math.round(w * 0.17), 10, 18), 9, 2);
        const top = pad + (icon ? iconSize + 4 : 0);
        const labelH = lineHeight(fit.size) * fit.lines.length;
        // Hide the label if it would collide with the value.
        const fits = top + labelH <= valueTop - 4;
        c.labelEl.style.display = fits ? "" : "none";
        if (fits) {
          c.labelEl.textContent = fit.lines.join("\n");
          placeText(c.labelEl, pad, top, innerW, fit.size, fit.lines.length);
        }
      }
    } else if (h >= 64) {
      // Roomy bar: icon on the left, label above the value next to it.
      const pad = Math.max(8, Math.round(h * 0.16));
      const iconSize = icon ? clamp(Math.round(h * 0.4), 18, 40) : 0;
      const left = pad + (icon ? iconSize + Math.round(pad * 0.7) : 0);
      const room = w - left - pad;
      const valueSize = clamp(Math.round(h * 0.26), 14, 28);
      const fit = showLabel ? fitLabel(text, room, clamp(Math.round(h * 0.18), 11, 18), 9, 1) : null;
      const labelH = fit ? lineHeight(fit.size) : 0;
      let top = (h - labelH - lineHeight(valueSize)) / 2;
      sizeIcon(c.iconEl, icon, pad, (h - iconSize) / 2, iconSize);
      c.valueEl.style.textAlign = "left";
      if (fit) {
        c.labelEl.textContent = fit.lines[0];
        placeText(c.labelEl, left, top, room, fit.size, 1);
        top += labelH;
      }
      placeText(c.valueEl, left, top, room, valueSize, 1);
    } else {
      // Slim bar: icon, label and value on one line.
      const pad = Math.max(6, Math.round(h * 0.18));
      const iconSize = icon ? clamp(Math.round(h * 0.45), 16, 40) : 0;
      const size = clamp(Math.round(h * 0.34), 11, 24);
      const valueW = Math.ceil(textWidth("100%", size)) + 4;
      const lh = lineHeight(size);
      c.valueEl.style.textAlign = "right";
      placeText(c.valueEl, w - pad - valueW, (h - lh) / 2, valueW, size, 1);
      sizeIcon(c.iconEl, icon, pad, (h - iconSize) / 2, iconSize);
      if (showLabel) {
        const left = pad + (icon ? iconSize + Math.round(pad * 0.6) : 0);
        const room = w - left - valueW - pad * 1.5;
        c.labelEl.style.display = room > 20 ? "" : "none";
        if (room > 20) {
          const fit = fitLabel(text, room, size, 9, 1);
          c.labelEl.textContent = fit.lines[0];
          placeText(c.labelEl, left, (h - lineHeight(fit.size)) / 2, room, fit.size, 1);
        }
      }
    }
  }

  /** Redraws the value text (also read out by screen readers) when it changed. */
  function drawValueText(c: SliderCell): void {
    const text = sliderValueText(c);
    if (text === c.drawnValue) return;
    c.drawnValue = text;
    c.valueEl.textContent = text;
    c.el.setAttribute("aria-valuetext", text);
  }

  function setSliderValue(c: SliderCell, value: number): void {
    c.value = clamp(value, 0, 1);
    if (c.vertical) c.fill.style.height = px(c.rect.h * c.value);
    else c.fill.style.width = px(c.rect.w * c.value);
    c.el.setAttribute("aria-valuenow", String(Math.round(c.value * 100)));
    drawValueText(c);
  }

  /** Maps a stage point to 0..1; the outer 2% of the bar snaps to exactly 0 / 1. */
  function sliderValueAt(c: SliderCell, x: number, y: number): number {
    const raw = c.vertical ? 1 - (y - c.rect.y) / c.rect.h : (x - c.rect.x) / c.rect.w;
    return clamp((raw - 0.02) / 0.96, 0, 1);
  }

  /** Sends the current value; unchanged values are skipped unless `final` (the release always reports). */
  function sendSlide(c: SliderCell, final: boolean): void {
    c.sendTimer = 0;
    if (c.state.disabled) return;
    const value = Math.round(c.value * 1000) / 1000;
    if (!final && value === c.lastSentValue) return;
    c.lastSent = Date.now();
    c.lastSentValue = value;
    events.slide(c.ctl.id, value);
  }

  /** Throttled send while dragging: at most one message per SLIDE_INTERVAL_MS, trailing value included. */
  function scheduleSlide(c: SliderCell): void {
    const wait = SLIDE_INTERVAL_MS - (Date.now() - c.lastSent);
    if (wait <= 0) sendSlide(c, false);
    else if (!c.sendTimer) c.sendTimer = window.setTimeout(function () {
      sendSlide(c, false);
    }, wait);
  }

  /** Ignores host values for SLIDER_SETTLE_MS from now (they echo our own sends), then applies the newest. */
  function holdEchoes(c: SliderCell): void {
    c.settleUntil = Date.now() + SLIDER_SETTLE_MS;
    window.clearTimeout(c.settleTimer);
    c.settleTimer = window.setTimeout(function () {
      c.settleTimer = 0;
      const held = c.held;
      c.held = null;
      if (!c.dragging && held && held.value !== null) setSliderValue(c, held.value);
    }, SLIDER_SETTLE_MS);
  }

  function endSlide(c: SliderCell): void {
    window.clearTimeout(c.sendTimer);
    c.dragging = false;
    sendSlide(c, true);
    holdEchoes(c);
  }

  /** One arrow / Home / End press on a focused slider: sent like a short drag. */
  function keySlide(c: SliderCell, value: number): void {
    c.held = null; // anything held predates this step
    setSliderValue(c, value);
    scheduleSlide(c);
    holdEchoes(c);
  }

  function patchSlider(c: SliderCell, st: ControlState): void {
    c.state = st;
    paintSlider(c);
    if (st.value !== null) {
      if (c.dragging || Date.now() < c.settleUntil) c.held = st;
      else setSliderValue(c, st.value);
    }
    // The dynamic label may have changed even when the value didn't.
    if (!c.dragging) drawValueText(c);
  }

  // ---- gestures -----------------------------------------------------------------------

  function hit(x: number, y: number): Cell | null {
    const slack = gap / 2;
    // Later controls are drawn on top, so test them first.
    for (let i = cells.length - 1; i >= 0; i--) {
      const r = cells[i].rect;
      if (x >= r.x - slack && x < r.x + r.w + slack && y >= r.y - slack && y < r.y + r.h + slack) return cells[i];
    }
    return null;
  }

  function findGesture(id: number): Gesture | null {
    for (let i = 0; i < gestures.length; i++) if (gestures[i].id === id) return gestures[i];
    return null;
  }

  function pointDown(id: number, clientX: number, clientY: number): void {
    lastGestureAt = Date.now();
    if (!online || findGesture(id)) return;
    const origin = stage.getBoundingClientRect();
    let target = hit(clientX - origin.left, clientY - origin.top);
    // Disabled controls still allow page swipes, but cannot press or slide.
    if (target && target.state.disabled) target = null;
    // A slider follows one finger: a second touch on it is ignored until the first one lifts.
    if (target && target.kind === "slider" && target.dragging) return;
    const g: Gesture = {
      id: id,
      cell: target,
      startX: clientX,
      startY: clientY,
      originX: origin.left,
      originY: origin.top,
      startedAt: Date.now(),
      moved: false,
      fired: false,
      swiped: false,
    };
    gestures.push(g);
    const cell = g.cell;
    if (!cell) return;
    switch (cell.kind) {
      case "button":
        setPressed(cell, true);
        haptic();
        // With a single page there is nothing to swipe to, so fire right away. With several
        // pages the press waits for release so a swipe that starts on a button isn't a press.
        if (!canSwipe()) {
          g.fired = true;
          events.press(cell.ctl.id);
        }
        break;
      case "slider":
        cell.dragging = true;
        cell.held = null; // a value held from the previous drag is stale now
        slideTo(cell, g, clientX, clientY);
        break;
      default:
        unreachable(cell);
    }
  }

  function slideTo(c: SliderCell, g: Gesture, clientX: number, clientY: number): void {
    if (c.state.disabled) return;
    setSliderValue(c, sliderValueAt(c, clientX - g.originX, clientY - g.originY));
    scheduleSlide(c);
  }

  function pointMove(id: number, clientX: number, clientY: number): void {
    const g = findGesture(id);
    if (!g) return;
    const cell = g.cell;
    if (cell && cell.kind === "slider") {
      slideTo(cell, g, clientX, clientY);
      return;
    }
    const dx = clientX - g.startX;
    const dy = clientY - g.startY;
    if (!g.moved && (Math.abs(dx) > SLOP || Math.abs(dy) > SLOP)) {
      g.moved = true;
      if (cell && cell.kind === "button") setPressed(cell, false);
    }
    // Horizontal swipe changes page as soon as it is clearly one.
    const reach = clamp(stage.clientWidth * 0.12, 40, 90);
    if (g.moved && !g.swiped && canSwipe() && Math.abs(dx) >= reach && Math.abs(dx) > Math.abs(dy) * 1.5) {
      g.swiped = true;
      events.nav({ type: dx < 0 ? "nextPage" : "prevPage" });
    }
  }

  function pointUp(id: number, cancelled: boolean): void {
    const g = findGesture(id);
    if (!g) return;
    lastGestureAt = Date.now();
    gestures.splice(gestures.indexOf(g), 1);
    const cell = g.cell;
    if (!cell) return;
    switch (cell.kind) {
      case "button": {
        if (!cell.state.disabled && !cancelled && !g.moved && !g.fired) events.press(cell.ctl.id);
        const wait = MIN_PRESS_MS - (Date.now() - g.startedAt);
        if (wait > 0) window.setTimeout(function () {
          setPressed(cell, false);
        }, wait);
        else setPressed(cell, false);
        break;
      }
      case "slider":
        endSlide(cell);
        break;
      default:
        unreachable(cell);
    }
  }

  function cancelGestures(): void {
    const open = gestures.slice();
    for (let i = 0; i < open.length; i++) pointUp(open[i].id, true);
    gestures = [];
  }

  // Touch: preventDefault on start/move kills the 300 ms click delay, text selection,
  // the callout, double-tap zoom and scroll bounce.
  frame.addEventListener("touchstart", function (e) {
    e.preventDefault();
    lastTouchAt = Date.now();
    for (let i = 0; i < e.changedTouches.length; i++) {
      const t = e.changedTouches[i];
      pointDown(t.identifier, t.clientX, t.clientY);
    }
  }, false);
  frame.addEventListener("touchmove", function (e) {
    e.preventDefault();
    for (let i = 0; i < e.changedTouches.length; i++) {
      const t = e.changedTouches[i];
      pointMove(t.identifier, t.clientX, t.clientY);
    }
  }, false);
  function touchEnd(cancelled: boolean): (e: TouchEvent) => void {
    return function (e) {
      lastTouchAt = Date.now();
      for (let i = 0; i < e.changedTouches.length; i++) pointUp(e.changedTouches[i].identifier, cancelled);
    };
  }
  frame.addEventListener("touchend", touchEnd(false), false);
  frame.addEventListener("touchcancel", touchEnd(true), false);

  // Mouse (desktop browsers), ignoring the emulated events that follow touches.
  frame.addEventListener("mousedown", function (e) {
    if (e.button !== 0 || Date.now() - lastTouchAt < GHOST_MOUSE_MS) return;
    pointDown(MOUSE_ID, e.clientX, e.clientY);
  }, false);
  window.addEventListener("mousemove", function (e) {
    pointMove(MOUSE_ID, e.clientX, e.clientY);
  }, false);
  window.addEventListener("mouseup", function () {
    pointUp(MOUSE_ID, false);
  }, false);

  function activateButton(cell: ButtonCell): void {
    setPressed(cell, true);
    events.press(cell.ctl.id);
    window.setTimeout(function () {
      setPressed(cell, false);
    }, MIN_PRESS_MS);
  }

  // Screen readers and scripted activation can send only click. Physical clicks following
  // our mouse/touch gestures have a nonzero detail; suppress those, including after swipes.
  stage.addEventListener("click", function (e) {
    if (e.detail !== 0 && Date.now() - lastGestureAt < GHOST_MOUSE_MS) return;
    const cell = cellOf(e.target);
    if (!cell || !online || cell.state.disabled || cell.kind !== "button") return;
    activateButton(cell);
  }, false);

  // Keyboard: Enter / Space press the focused button; arrows, Home and End move the focused slider.
  stage.addEventListener("keydown", function (e) {
    const cell = cellOf(e.target);
    if (!cell || !online || cell.state.disabled) return;
    if (cell.kind === "button") {
      if (!isActivationKey(e)) return;
      e.preventDefault(); // Space would scroll
      if (e.repeat) return;
      activateButton(cell);
      return;
    }
    let next: number;
    switch (e.keyCode) {
      case 37: // left
      case 40: // down
        next = cell.value - SLIDER_KEY_STEP;
        break;
      case 38: // up
      case 39: // right
        next = cell.value + SLIDER_KEY_STEP;
        break;
      case 36: // Home
        next = 0;
        break;
      case 35: // End
        next = 1;
        break;
      default:
        return;
    }
    e.preventDefault();
    if (!cell.dragging) keySlide(cell, next);
  }, false);

  // ---- public -------------------------------------------------------------------------

  return {
    setLayout: function (next) {
      layout = next;
      states = newMap<ControlState>();
      for (let i = 0; i < next.controls.length; i++) states[next.controls[i].id] = next.controls[i].state;
      rebuild();
    },
    applyStates: function (updates) {
      for (let i = 0; i < updates.length; i++) {
        const st = updates[i];
        const cell = byId[st.id];
        if (!cell) continue;
        const prev = states[st.id];
        states[st.id] = st;
        if (prev && prev.active === st.active && prev.value === st.value && prev.label === st.label && prev.disabled === st.disabled) continue;
        switch (cell.kind) {
          case "button":
            cell.state = st;
            paintButton(cell);
            break;
          case "slider":
            patchSlider(cell, st);
            break;
          default:
            unreachable(cell);
        }
      }
    },
    relayout: rebuild,
    setOnline: function (value) {
      online = value;
      if (!value) cancelGestures();
    },
    cancelGestures: cancelGestures,
  };
}
