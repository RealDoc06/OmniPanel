// Settings sheet: fullscreen, reload, unpair, about. Built fresh each time it opens.

import type { ThemeColors } from "@omni/protocol";
import type { ConnStatus } from "./conn";
import { PROTOCOL_VERSION, isIos, isIosStandalone } from "./device";
import { clamp, clearChildren, el, setVisible } from "./dom";
import { ICONS } from "./icons";
import { canFullscreen, isFullscreen, toggleFullscreen } from "./power";
import { onTap } from "./tap";

const CONFIRM_MS = 3000;

export interface SheetInfo {
  hostName: string;
  hostVersion: string;
  deviceName: string;
  status: ConnStatus;
  /** Null before the first layout; the sheet then uses a neutral dark look. */
  theme: ThemeColors | null;
}

export interface SheetActions {
  info(): SheetInfo;
  reload(): void;
  unpair(): void;
}

export interface Sheet {
  open(): void;
  close(): void;
  isOpen(): boolean;
}

const STATUS_TEXT: { [K in ConnStatus]: string } = {
  online: "Connected",
  connecting: "Connecting",
  offline: "Reconnecting",
};

export function createSheet(parent: HTMLElement, actions: SheetActions): Sheet {
  const root = el("div", "sheet", parent);
  const back = el("div", "sheet-back", root);
  const panel = el("div", "sheet-panel scroll", root);
  setVisible(root, false);
  let open = false;
  let confirmTimer = 0;

  function row(text: string, icon: string, fn: () => void): { node: HTMLElement; label: HTMLElement } {
    const node = el("div", "sheet-row", panel);
    const ic = el("span", "sheet-row-icon", node);
    ic.innerHTML = icon;
    const label = el("span", "sheet-row-text", node);
    label.textContent = text;
    onTap(node, fn);
    return { node: node, label: label };
  }

  function build(): void {
    const info = actions.info();
    const theme = info.theme;
    clearChildren(panel);
    panel.style.backgroundColor = theme ? theme.surface : "#141414";
    panel.style.color = theme ? theme.text : "#ffffff";
    const width = Math.min(440, root.clientWidth);
    panel.style.width = width + "px";
    panel.style.left = Math.round((root.clientWidth - width) / 2) + "px";
    panel.style.maxHeight = Math.round(root.clientHeight * 0.94) + "px";
    panel.style.borderTopLeftRadius = panel.style.borderTopRightRadius = clamp(Math.round(root.clientHeight * 0.03), 8, 18) + "px";

    const head = el("div", "sheet-head", panel);
    const title = el("span", "sheet-title", head);
    title.textContent = "Settings";
    const close = el("div", "sheet-close", head);
    close.innerHTML = ICONS.close;
    onTap(close, closeSheet);

    if (canFullscreen()) {
      const fs = isFullscreen();
      row(fs ? "Exit fullscreen" : "Fullscreen", fs ? ICONS.minimize : ICONS.maximize, function () {
        toggleFullscreen();
        closeSheet();
      });
    }
    row("Reload", ICONS.reload, actions.reload);
    // Two taps: the first arms the row, the second unpairs.
    const unpair = row("Unpair this device", ICONS.unlink, function () {
      if (!confirmTimer) {
        unpair.label.textContent = "Tap again to unpair";
        unpair.node.className = "sheet-row danger";
        confirmTimer = window.setTimeout(function () {
          confirmTimer = 0;
          unpair.label.textContent = "Unpair this device";
          unpair.node.className = "sheet-row";
        }, CONFIRM_MS);
        return;
      }
      window.clearTimeout(confirmTimer);
      confirmTimer = 0;
      closeSheet();
      actions.unpair();
    });

    const about = el("div", "sheet-about", panel);
    const add = function (label: string, value: string): void {
      const node = el("div", "sheet-line", about);
      const l = el("span", "sheet-line-label", node);
      l.textContent = label;
      const v = el("span", "sheet-line-value", node);
      v.textContent = value;
    };
    add("Host", info.hostName || "Unknown");
    add("Host version", info.hostVersion || "Unknown");
    add("This device", info.deviceName || "Unknown");
    add("Status", STATUS_TEXT[info.status]);
    add("Deck", __DECK_VERSION__ + " (protocol " + PROTOCOL_VERSION + ")");
    const tip = el("div", "sheet-tip", about);
    tip.textContent = isIos()
      ? "To keep the screen on, set Settings > Display & Brightness > Auto-Lock to Never." +
        (isIosStandalone() ? "" : " Add this page to the Home Screen for a fullscreen deck.")
      : "To keep the screen on, enable Stay awake in Android's Developer options (or use a charger and a long screen timeout).";
  }

  function closeSheet(): void {
    if (!open) return;
    open = false;
    window.clearTimeout(confirmTimer);
    confirmTimer = 0;
    setVisible(root, false);
  }

  onTap(back, closeSheet);

  return {
    open: function () {
      if (open) return;
      open = true;
      setVisible(root, true);
      build();
    },
    close: closeSheet,
    isOpen: function () {
      return open;
    },
  };
}
