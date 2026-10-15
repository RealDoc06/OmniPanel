// Toasts: top-center, auto-hide, colored by level. Not interactive.

import type { ThemeColors, ToastLevel } from "@omni/protocol";
import { readableOn } from "./color";
import { el } from "./dom";

const SHOW_MS = 2500;
const FADE_MS = 200;
const MAX_VISIBLE = 3;

export interface Toasts {
  show(text: string, level: ToastLevel, theme: ThemeColors | null): void;
}

export function createToasts(parent: HTMLElement): Toasts {
  const host = el("div", "toasts", parent);
  const live: HTMLElement[] = [];

  function dismiss(node: HTMLElement): void {
    const at = live.indexOf(node);
    if (at === -1) return;
    live.splice(at, 1);
    node.className = "toast";
    window.setTimeout(function () {
      const row = node.parentNode;
      if (row && row.parentNode) row.parentNode.removeChild(row);
    }, FADE_MS);
  }

  return {
    show: function (text, level, theme) {
      while (live.length >= MAX_VISIBLE) dismiss(live[0]);
      const node = el("div", "toast", el("div", "toast-row", host));
      node.textContent = text;
      let bg = theme ? theme.surface : "#1d1d1d";
      let fg = theme ? theme.text : "#ffffff";
      switch (level) {
        case "success":
          bg = theme ? theme.success : "#22c55e";
          fg = readableOn(bg, "#ffffff");
          break;
        case "error":
          bg = theme ? theme.danger : "#ef4444";
          fg = readableOn(bg, "#ffffff");
          break;
        case "info":
          node.style.border = "1px solid " + (theme ? theme.border : "#333333");
          break;
      }
      node.style.backgroundColor = bg;
      node.style.color = fg;
      live.push(node);
      // Force a reflow so the entry transition runs.
      node.getBoundingClientRect();
      node.className = "toast in";
      window.setTimeout(function () {
        dismiss(node);
      }, SHOW_MS);
    },
  };
}
