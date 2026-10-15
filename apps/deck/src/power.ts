// Keep-awake (Screen Wake Lock where it exists), dim overlay and fullscreen.

import type { DeckSettings } from "@omni/protocol";
import { clamp, el, isPageHidden, onVisibilityChange } from "./dom";

interface FullscreenElement extends HTMLElement {
  webkitRequestFullscreen?: () => void;
  mozRequestFullScreen?: () => void;
  msRequestFullscreen?: () => void;
}

interface FullscreenDocument extends Document {
  webkitFullscreenElement?: Element | null;
  webkitExitFullscreen?: () => void;
  mozFullScreenElement?: Element | null;
  mozCancelFullScreen?: () => void;
  msFullscreenElement?: Element | null;
  msExitFullscreen?: () => void;
}

export function canFullscreen(): boolean {
  const root = document.documentElement as FullscreenElement;
  return !!(root.requestFullscreen || root.webkitRequestFullscreen || root.mozRequestFullScreen || root.msRequestFullscreen);
}

export function isFullscreen(): boolean {
  const doc = document as FullscreenDocument;
  return !!(doc.fullscreenElement || doc.webkitFullscreenElement || doc.mozFullScreenElement || doc.msFullscreenElement);
}

export function toggleFullscreen(): void {
  const doc = document as FullscreenDocument;
  const root = document.documentElement as FullscreenElement;
  try {
    if (isFullscreen()) {
      const exit = doc.exitFullscreen || doc.webkitExitFullscreen || doc.mozCancelFullScreen || doc.msExitFullscreen;
      if (exit) exit.call(doc);
    } else {
      const enter = root.requestFullscreen || root.webkitRequestFullscreen || root.mozRequestFullScreen || root.msRequestFullscreen;
      if (enter) enter.call(root);
    }
  } catch (_e) {
    // not allowed right now (needs a user gesture); ignore
  }
}

export interface Power {
  /** Applies the layout's dim + keep-awake settings. */
  apply(settings: DeckSettings): void;
}

export function createPower(parent: HTMLElement): Power {
  const dim = el("div", "dim", parent);
  dim.style.display = "none";
  let wanted = false;
  let sentinel: WakeLockSentinel | null = null;

  /** Screen Wake Lock is Promise-based and https-only; it simply doesn't exist on old WebViews. */
  function acquire(): void {
    if (!wanted || sentinel || isPageHidden() || !("wakeLock" in navigator)) return;
    try {
      navigator.wakeLock.request("screen").then(
        function (lock) {
          sentinel = lock;
          lock.addEventListener("release", function () {
            if (sentinel === lock) sentinel = null;
          });
          // Keep-awake was switched off while the request was in flight.
          if (!wanted) release();
        },
        function () {
          sentinel = null;
        },
      );
    } catch (_e) {
      sentinel = null;
    }
  }

  function release(): void {
    const lock = sentinel;
    sentinel = null;
    if (lock) {
      try {
        lock.release();
      } catch (_e) {
        // already released
      }
    }
  }

  // The lock is dropped automatically when the page is hidden: take it again on return.
  onVisibilityChange(acquire);

  return {
    apply: function (settings) {
      const opacity = clamp(settings.dim, 0, 0.8);
      dim.style.opacity = String(opacity);
      dim.style.display = opacity > 0 ? "" : "none";
      wanted = settings.keepAwake;
      if (wanted) acquire();
      else release();
    },
  };
}
