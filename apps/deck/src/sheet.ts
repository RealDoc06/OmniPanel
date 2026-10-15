// Settings sheet: fullscreen, reload, unpair, about. Built fresh each time it opens.
// A modal dialog for keyboards and screen readers: focus moves in and stays in, Escape closes.

import type { ThemeColors } from "@omni/protocol";
import type { ConnStatus } from "./conn";
import { PROTOCOL_VERSION, iosVersion, isAndroid, isIos, isIosStandalone } from "./device";
import { clamp, clearChildren, el, setVisible } from "./dom";
import { ICONS } from "./icons";
import { canFullscreen, isFullscreen, toggleFullscreen } from "./power";
import { onTap, tapButton } from "./tap";

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
  refreshInfo(): void;
  /** Re-fits the open panel to the viewport (rotation) without rebuilding its rows. */
  relayout(): void;
}

const STATUS_TEXT: { [K in ConnStatus]: string } = {
  online: "Connected",
  connecting: "Connecting",
  offline: "Reconnecting",
};

/** How to keep this device's screen on; old decks have no Wake Lock, so the user has to set it. */
function keepAwakeTip(): string {
  if (isIos()) {
    // iOS 10 moved Auto-Lock from General to Display & Brightness.
    const version = iosVersion();
    const path = version > 0 && version < 10 ? "Settings > General > Auto-Lock" : "Settings > Display & Brightness > Auto-Lock";
    return "To keep the screen on, set " + path + " to Never." + (isIosStandalone() ? "" : " Add this page to the Home Screen for a fullscreen deck.");
  }
  if (isAndroid()) return "To keep the screen on while charging, turn on Stay awake in Developer options.";
  return "To keep the screen on, set a long screen timeout in this device's settings.";
}

export function createSheet(parent: HTMLElement, actions: SheetActions): Sheet {
  const root = el("div", "sheet", parent);
  const back = el("div", "sheet-back", root);
  const panel = el("div", "sheet-panel scroll", root);
  panel.setAttribute("role", "dialog");
  panel.setAttribute("aria-modal", "true");
  panel.setAttribute("aria-label", "Settings");
  setVisible(root, false);
  let open = false;
  let confirmTimer = 0;
  /** Focused before opening; gets focus back on close. */
  let opener: HTMLElement | null = null;
  let closeButton: HTMLElement | null = null;
  let deviceNameNode: HTMLElement | null = null;
  let hostNameNode: HTMLElement | null = null;

  function row(text: string, icon: string, fn: () => void): { node: HTMLElement; label: HTMLElement } {
    const node = el("div", "sheet-row", panel);
    const ic = el("span", "sheet-row-icon", node);
    ic.innerHTML = icon;
    const label = el("span", "sheet-row-text", node);
    label.textContent = text;
    tapButton(node, fn);
    return { node: node, label: label };
  }

  /** Sizes and centers the panel for the current viewport. */
  function fit(): void {
    const width = Math.min(440, root.clientWidth);
    panel.style.width = width + "px";
    panel.style.left = Math.round((root.clientWidth - width) / 2) + "px";
    panel.style.maxHeight = Math.round(root.clientHeight * 0.94) + "px";
    panel.style.borderTopLeftRadius = panel.style.borderTopRightRadius = clamp(Math.round(root.clientHeight * 0.03), 8, 18) + "px";
  }

  function build(): void {
    const info = actions.info();
    const theme = info.theme;
    clearChildren(panel);
    panel.style.backgroundColor = theme ? theme.surface : "#141414";
    panel.style.color = theme ? theme.text : "#ffffff";
    fit();

    const head = el("div", "sheet-head", panel);
    const title = el("span", "sheet-title", head);
    title.textContent = "Settings";
    closeButton = el("div", "sheet-close", head);
    closeButton.innerHTML = ICONS.close;
    tapButton(closeButton, closeSheet, "Close");

    if (canFullscreen()) {
      const fs = isFullscreen();
      row(fs ? "Exit fullscreen" : "Fullscreen", fs ? ICONS.minimize : ICONS.maximize, function () {
        toggleFullscreen();
        closeSheet();
      });
    }
    row("Reload", ICONS.reload, actions.reload);
    // Two taps: the first arms the row, the second unpairs.
    const unpair = row("Disconnect this device", ICONS.unlink, function () {
      if (!confirmTimer) {
        unpair.label.textContent = "Tap again to disconnect";
        unpair.node.className = "sheet-row danger";
        confirmTimer = window.setTimeout(function () {
          confirmTimer = 0;
          unpair.label.textContent = "Disconnect this device";
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
    const add = function (label: string, value: string): HTMLElement {
      const node = el("div", "sheet-line", about);
      const l = el("span", "sheet-line-label", node);
      l.textContent = label;
      const v = el("span", "sheet-line-value", node);
      v.textContent = value;
      return v;
    };
    hostNameNode = add("Connected to", info.hostName || "Unknown");
    deviceNameNode = add("This device", info.deviceName || "Unknown");
    add("Status", STATUS_TEXT[info.status]);
    const tip = el("div", "sheet-tip", about);
    tip.textContent = keepAwakeTip();
    const version = el("div", "sheet-version", about);
    version.textContent =
      (info.hostVersion ? "OmniPanel " + info.hostVersion + " \u00b7 deck " : "Deck ") + __DECK_VERSION__ + " \u00b7 protocol " + PROTOCOL_VERSION;
  }

  function closeSheet(): void {
    if (!open) return;
    open = false;
    window.clearTimeout(confirmTimer);
    confirmTimer = 0;
    const hadFocus = root.contains(document.activeElement);
    setVisible(root, false);
    if (hadFocus && opener) opener.focus();
    opener = null;
  }

  onTap(back, closeSheet);
  document.addEventListener("keydown", function (e) {
    if (open && e.keyCode === 27) closeSheet();
  }, false);
  // Modal: focus that wanders behind the open sheet comes back to it, wrapping around like Tab does:
  // past the last row to Close, Shift+Tab past Close to the last row.
  let lastFocused: EventTarget | null = null;
  document.addEventListener("focus", function (e) {
    if (!open || !closeButton) return;
    if (root.contains(e.target as Node)) {
      lastFocused = e.target;
      return;
    }
    const rows = panel.querySelectorAll(".sheet-row");
    const lastRow = rows.length ? (rows[rows.length - 1] as HTMLElement) : closeButton;
    (lastFocused === closeButton ? lastRow : closeButton).focus();
  }, true);

  return {
    open: function () {
      if (open) return;
      open = true;
      opener = document.activeElement as HTMLElement | null;
      setVisible(root, true);
      build();
      if (closeButton) closeButton.focus();
    },
    close: closeSheet,
    refreshInfo: function () {
      const info = actions.info();
      if (deviceNameNode) deviceNameNode.textContent = info.deviceName || "Unknown";
      if (hostNameNode) hostNameNode.textContent = info.hostName || "Unknown";
    },
    isOpen: function () {
      return open;
    },
    relayout: function () {
      if (open) fit();
    },
  };
}
