// Thin status bar: profile / page name, page dots, connection dot + host name, settings gear.
// Long-pressing the bar also opens settings. With the bar hidden, the gear floats in the corner.

import type { DeckLayout, ThemeColors } from "@omni/protocol";
import type { ConnStatus } from "./conn";
import { clearChildren, el, setVisible } from "./dom";
import { ICONS } from "./icons";
import { onTap } from "./tap";

const LONG_PRESS_MS = 600;
const LONG_PRESS_SLOP = 10;
const DOT_PITCH = 22;
const MIN_DOT_PITCH = 14;

export interface StatusBarActions {
  goToPage(index: number): void;
  openSettings(): void;
}

export interface StatusBar {
  /** Current bar height in px (0 while hidden); the deck sits below it. */
  height(): number;
  setBarVisible(visible: boolean): void;
  setLayout(layout: DeckLayout): void;
  setHost(hostName: string): void;
  setConn(status: ConnStatus): void;
  relayout(): void;
}

export function createStatusBar(parent: HTMLElement, actions: StatusBarActions): StatusBar {
  const bar = el("div", "bar", parent);
  const left = el("div", "bar-left", bar);
  const profileEl = el("span", "bar-profile", left);
  const sepEl = el("span", "bar-sep", left);
  sepEl.textContent = " / ";
  const pageEl = el("span", "bar-page", left);
  const dots = el("div", "bar-dots", bar);
  const right = el("div", "bar-right", bar);
  const connDot = el("span", "bar-conn", right);
  const hostEl = document.createTextNode("");
  right.appendChild(hostEl);
  const gear = el("div", "gear", parent);
  gear.innerHTML = ICONS.gear;

  let layout: DeckLayout | null = null;
  let theme: ThemeColors | null = null;
  let status: ConnStatus = "connecting";
  let barVisible = true;
  let barHeight = 28;
  let dotEls: HTMLElement[] = [];
  let dotCount = -1;

  onTap(gear, actions.openSettings);

  // Long press on the bar (not on the dots / gear, which own their touches).
  let pressTimer = 0;
  let pressX = 0;
  let pressY = 0;
  function pressStart(x: number, y: number): void {
    pressX = x;
    pressY = y;
    window.clearTimeout(pressTimer);
    pressTimer = window.setTimeout(actions.openSettings, LONG_PRESS_MS);
  }
  function pressMove(x: number, y: number): void {
    if (Math.abs(x - pressX) > LONG_PRESS_SLOP || Math.abs(y - pressY) > LONG_PRESS_SLOP) window.clearTimeout(pressTimer);
  }
  function pressEnd(): void {
    window.clearTimeout(pressTimer);
  }
  bar.addEventListener("touchstart", function (e) {
    pressStart(e.changedTouches[0].clientX, e.changedTouches[0].clientY);
  }, false);
  bar.addEventListener("touchmove", function (e) {
    pressMove(e.changedTouches[0].clientX, e.changedTouches[0].clientY);
  }, false);
  bar.addEventListener("touchend", pressEnd, false);
  bar.addEventListener("touchcancel", pressEnd, false);
  bar.addEventListener("mousedown", function (e) {
    pressStart(e.clientX, e.clientY);
  }, false);
  bar.addEventListener("mousemove", function (e) {
    pressMove(e.clientX, e.clientY);
  }, false);
  bar.addEventListener("mouseup", pressEnd, false);
  bar.addEventListener("mouseleave", pressEnd, false);

  function paintConn(): void {
    if (!theme) return;
    connDot.style.backgroundColor = status === "online" ? theme.success : status === "connecting" ? theme.warning : theme.danger;
  }

  function paintDots(): void {
    if (!layout || !theme) return;
    for (let i = 0; i < dotEls.length; i++) {
      const dot = dotEls[i].firstChild as HTMLElement;
      const current = i === layout.pageIndex;
      dot.style.backgroundColor = current ? theme.text : theme.textMuted;
      dot.style.opacity = current ? "1" : "0.4";
      dot.className = current ? "bar-dot current" : "bar-dot";
    }
  }

  /** Rebuilds the dot buttons when the page count changes. */
  function buildDots(count: number): void {
    clearChildren(dots);
    dotEls = [];
    dotCount = count;
    for (let i = 0; i < count; i++) {
      const hit = el("div", "bar-dot-hit", dots);
      el("span", "bar-dot", hit);
      const index = i;
      onTap(hit, function () {
        if (layout && index !== layout.pageIndex) actions.goToPage(index);
      });
      dotEls.push(hit);
    }
  }

  function relayout(): void {
    const width = parent.clientWidth;
    if (!width || !layout) return;
    barHeight = Math.min(window.innerWidth, window.innerHeight) >= 600 ? 34 : 28;
    bar.style.height = barHeight + "px";
    bar.style.lineHeight = barHeight + "px";
    bar.style.fontSize = (barHeight >= 34 ? 15 : 13) + "px";
    setVisible(bar, barVisible);
    // The gear's tap area is wider than the bar is tall (fingers); the icon itself stays small.
    const iconSize = barHeight >= 34 ? 20 : 18;
    const gearWidth = 44;
    const gearHeight = barVisible ? barHeight : 44;
    gear.className = barVisible ? "gear in-bar" : "gear floating";
    gear.style.width = gearWidth + "px";
    gear.style.height = gearHeight + "px";
    gear.style.padding = Math.round((gearHeight - iconSize) / 2) + "px " + Math.round((gearWidth - iconSize) / 2) + "px";

    // Dots: shrink the pitch for many pages; beyond that fall back to "3 / 12" text.
    const pages = layout.pageCount;
    let pitch = DOT_PITCH;
    const room = width * 0.4;
    if (pages * pitch > room) pitch = Math.max(MIN_DOT_PITCH, Math.floor(room / pages));
    const compact = pages * pitch > room;
    let dotsWidth = 0;
    if (pages > 1 && compact) {
      clearChildren(dots);
      dotEls = [];
      dotCount = -1;
      dots.textContent = layout.pageIndex + 1 + " / " + pages;
      dotsWidth = 56;
    } else if (pages > 1) {
      if (dotCount !== pages) buildDots(pages);
      for (let i = 0; i < dotEls.length; i++) {
        dotEls[i].style.width = pitch + "px";
        dotEls[i].style.height = barHeight + "px";
      }
      paintDots();
      dotsWidth = pages * pitch;
    } else {
      clearChildren(dots);
      dotEls = [];
      dotCount = -1;
    }
    dots.style.width = dotsWidth + "px";
    dots.style.left = Math.round((width - dotsWidth) / 2) + "px";
    dots.style.height = barHeight + "px";

    // Each side gets half the remaining width; hide the text when it would be unreadable.
    const side = Math.floor((width - dotsWidth) / 2);
    const leftMax = side - 10 - 8;
    left.style.maxWidth = Math.max(0, leftMax) + "px";
    setVisible(left, leftMax >= 40);
    const rightMax = side - gearWidth - 8;
    right.style.right = gearWidth + 4 + "px";
    right.style.maxWidth = Math.max(0, rightMax) + "px";
    setVisible(right, rightMax >= 40);
  }

  return {
    height: function () {
      return barVisible ? barHeight : 0;
    },
    setBarVisible: function (visible) {
      barVisible = visible;
      relayout();
    },
    setLayout: function (next) {
      layout = next;
      theme = next.theme;
      bar.style.color = theme.textMuted;
      profileEl.textContent = next.profileName;
      pageEl.textContent = next.pageName;
      pageEl.style.color = theme.text;
      gear.style.color = theme.textMuted;
      gear.style.opacity = barVisible ? "1" : "0.35";
      paintConn();
      relayout();
    },
    setHost: function (hostName) {
      hostEl.nodeValue = hostName;
    },
    setConn: function (next) {
      status = next;
      paintConn();
    },
    relayout: relayout,
  };
}

