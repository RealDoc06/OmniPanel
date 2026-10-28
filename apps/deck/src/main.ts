// OmniPanel deck client: wires the connection, pairing, deck, status bar, toasts and settings.

import type { DeckLayout, HostToDeck, ThemeColors } from "@omni/protocol";
import { connect } from "./conn";
import type { ConnStatus } from "./conn";
import { createDeck } from "./deck";
import { clientInfo, viewportSize } from "./device";
import { el, isPageHidden, onVisibilityChange, setVisible, unreachable } from "./dom";
import { createPairScreen } from "./pair";
import { createPower } from "./power";
import { createSheet } from "./sheet";
import { createStatusBar } from "./statusbar";
import { clearToken, loadToken, saveToken } from "./storage";
import { createToasts } from "./toast";

/** After pairing the host should push the deck on its own; if it doesn't, ask with a new hello. */
const HELLO_FALLBACK_MS = 1500;
/** Quick reconnects shouldn't flash the "Reconnecting" overlay. */
const OFFLINE_OVERLAY_DELAY_MS = 600;
const SPLASH_HINT_AFTER_FAILURES = 2;
/** How long to wait for the host's `unpaired` reply before reconnecting ourselves. */
const UNPAIR_REPLY_MS = 1500;

type Screen = "splash" | "pair" | "deck";

const app = document.getElementById("app");
if (!app) throw new Error("#app missing");

// ---- DOM -----------------------------------------------------------------------------

/** Content area inside the device's safe insets (notch, home indicator). */
const frame = el("div", "frame", app);
const deckRoot = el("div", "deck", frame);
const stage = el("div", "stage", deckRoot);
const offlineBanner = el("div", "offline", deckRoot);
offlineBanner.textContent = "Reconnecting…";
const splash = el("div", "splash", frame);
const splashLogo = el("img", "splash-logo", splash);
splashLogo.src = "icons/icon-192.png";
splashLogo.alt = "";
const splashText = el("div", "splash-text", splash);
const splashHint = el("div", "splash-hint", splash);

// ---- state ---------------------------------------------------------------------------

let screen: Screen = "splash";
let layout: DeckLayout | null = null;
/** Raw text of the layout on screen; an identical re-send is ignored (it would only rebuild and cancel drags). */
let lastLayoutRaw = "";
let status: ConnStatus = "connecting";
let hostName = "";
let hostVersion = "";
let deviceName = "";
let overlayTimer = 0;
let overlayArmed = false;
let splashFailures = 0;
let helloTimer = 0;
let unpairTimer = 0;
let appliedSize = viewportSize();

const deck = createDeck(deckRoot, stage, {
  press: function (id) {
    conn.send({ t: "press", control: id });
  },
  slide: function (id, value) {
    conn.send({ t: "slide", control: id, value: value });
  },
  nav: function (to) {
    conn.send({ t: "nav", to: to });
  },
});

const toasts = createToasts(frame);
const power = createPower(app);

const pair = createPairScreen(frame, function (code, name) {
  return conn.send({ t: "pair", code: code, name: name });
});

const sheet = createSheet(frame, {
  info: function () {
    return { hostName: hostName, hostVersion: hostVersion, deviceName: deviceName, status: status, theme: layout ? layout.theme : null };
  },
  reload: function () {
    window.location.reload();
  },
  unpair: function () {
    const sent = conn.send({ t: "unpair" });
    forgetDevice();
    // The host removes the device, answers `unpaired` and closes the socket. If it can't
    // (socket already down, older host), reconnect ourselves without the token.
    window.clearTimeout(unpairTimer);
    unpairTimer = window.setTimeout(conn.restart, sent ? UNPAIR_REPLY_MS : 0);
  },
});

/** Jumps to the page behind a status bar dot. */
function goToPage(index: number): void {
  const page = layout ? layout.pages[index] : undefined;
  if (page) conn.send({ t: "nav", to: { type: "page", pageId: page.id } });
}

const statusBar = createStatusBar(deckRoot, {
  goToPage: goToPage,
  openSettings: function () {
    if (screen === "deck") sheet.open();
  },
});

// ---- screens -------------------------------------------------------------------------

function showScreen(next: Screen): void {
  screen = next;
  setVisible(deckRoot, next === "deck");
  setVisible(splash, next === "splash");
  if (next !== "pair") pair.hide();
  if (next !== "deck") sheet.close();
  paintSplash();
}

function paintSplash(failures?: number): void {
  if (failures !== undefined) splashFailures = failures;
  if (status === "online") {
    splashText.textContent = "Loading…";
    splashHint.textContent = "";
  } else if (splashFailures >= SPLASH_HINT_AFTER_FAILURES) {
    splashText.textContent = "Can't reach OmniPanel";
    splashHint.textContent = "Make sure the desktop app is running and this device is on the same network.";
  } else {
    splashText.textContent = "Connecting…";
    splashHint.textContent = "";
  }
}

/** Forgets the pairing token and goes back to the connecting screen. */
function forgetDevice(): void {
  clearToken();
  layout = null;
  lastLayoutRaw = "";
  applyTheme(null);
  showScreen("splash");
}

/** Theme colors drive the page background; null resets to the default black. */
function applyTheme(theme: ThemeColors | null): void {
  const color = theme ? theme.background : "#000000";
  document.documentElement.style.backgroundColor = color;
  document.body.style.backgroundColor = color;
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.setAttribute("content", color);
}

/** Recomputes everything that depends on the viewport size. */
function relayoutAll(): void {
  statusBar.relayout();
  stage.style.top = statusBar.height() + "px";
  deck.relayout();
  pair.relayout();
  sheet.relayout();
}

function applyLayout(next: DeckLayout): void {
  window.clearTimeout(helloTimer);
  layout = next;
  applyTheme(next.theme);
  showScreen("deck");
  power.apply(next.settings);
  statusBar.setBarVisible(next.settings.showStatusBar);
  statusBar.setLayout(next);
  stage.style.top = statusBar.height() + "px";
  deck.setLayout(next);
  setOfflineOverlay(status !== "online" && overlayArmed);
}

// ---- connection state ----------------------------------------------------------------

function setOfflineOverlay(show: boolean): void {
  setVisible(offlineBanner, show && screen === "deck");
  stage.style.opacity = show ? "0.35" : "";
}

function onStatus(next: ConnStatus, failures: number): void {
  status = next;
  statusBar.setConn(next);
  deck.setOnline(next === "online");
  window.clearTimeout(overlayTimer);
  if (next === "online") {
    overlayArmed = false;
    setOfflineOverlay(false);
  } else {
    lastLayoutRaw = ""; // after a reconnect always redraw from the host's fresh layout
    overlayTimer = window.setTimeout(function () {
      overlayArmed = true;
      setOfflineOverlay(true);
    }, OFFLINE_OVERLAY_DELAY_MS);
  }
  paintSplash(failures);
}

function onMessage(msg: HostToDeck, raw: string): void {
  switch (msg.t) {
    case "needPair":
      layout = null;
      lastLayoutRaw = "";
      hostName = msg.hostName;
      statusBar.setHost(hostName);
      applyTheme(null);
      showScreen("pair");
      pair.show(msg.hostName);
      break;
    case "pairResult":
      if (msg.ok && msg.token) {
        saveToken(msg.token);
        pair.accepted();
        window.clearTimeout(helloTimer);
        helloTimer = window.setTimeout(function () {
          if (screen !== "deck") conn.send({ t: "hello", token: loadToken(), client: clientInfo() });
        }, HELLO_FALLBACK_MS);
      } else {
        pair.fail(msg.error || "Connection failed. Check the code and try again.");
      }
      break;
    case "welcome":
      hostName = msg.hostName;
      hostVersion = msg.hostVersion;
      deviceName = msg.deviceName;
      sheet.refreshInfo();
      statusBar.setHost(hostName);
      break;
    case "layout":
      if (raw === lastLayoutRaw && screen === "deck") break;
      lastLayoutRaw = raw;
      applyLayout(msg.layout);
      break;
    case "state":
      deck.applyStates(msg.states);
      break;
    case "toast":
      toasts.show(msg.text, msg.level, layout ? layout.theme : null);
      break;
    case "unpaired":
      window.clearTimeout(unpairTimer);
      forgetDevice();
      conn.restart(); // a token-less hello gets needPair
      break;
    case "pong":
      break;
    default:
      // A newer host may send messages this client doesn't know; ignore them.
      unreachable(msg);
  }
}

// ---- viewport ------------------------------------------------------------------------

/** Re-lays out and tells the host when the viewport size actually changed (rotation, resize). */
function checkViewport(): void {
  const size = viewportSize();
  if (size.width === appliedSize.width && size.height === appliedSize.height) return;
  appliedSize = size;
  relayoutAll();
  conn.send({ t: "resize", width: size.width, height: size.height });
}

let viewportTimer = 0;
let viewportLateTimer = 0;
/** iOS reports the old size right after `orientationchange`, so check again a bit later. */
function scheduleViewportCheck(): void {
  window.clearTimeout(viewportTimer);
  window.clearTimeout(viewportLateTimer);
  viewportTimer = window.setTimeout(checkViewport, 120);
  viewportLateTimer = window.setTimeout(checkViewport, 500);
}
window.addEventListener("resize", scheduleViewportCheck, false);
window.addEventListener("orientationchange", scheduleViewportCheck, false);

// ---- page-level behavior -------------------------------------------------------------

onVisibilityChange(function () {
  if (isPageHidden()) deck.cancelGestures();
});
window.addEventListener("blur", function () {
  deck.cancelGestures();
}, false);

// Focus rings only while the keyboard is in use (old WebViews have no :focus-visible); see `.kbd` in app.css.
// Capture phase: tap targets stop their touches and mouse presses from bubbling. Typing into the name
// field doesn't count (on-screen keyboards send keys too), except Tab out of it.
const docClasses = document.documentElement.classList;
document.addEventListener("keydown", function (e) {
  if ((e.target as HTMLElement).tagName !== "INPUT" || e.keyCode === 9) docClasses.add("kbd");
}, true);
document.addEventListener("touchstart", function () {
  docClasses.remove("kbd");
}, true);
document.addEventListener("mousedown", function () {
  docClasses.remove("kbd");
}, true);

// No context menu, pinch zoom (iOS 10+ ignores user-scalable=no) or rubber-banding.
document.addEventListener("contextmenu", function (e) {
  e.preventDefault();
}, false);
document.addEventListener("gesturestart", function (e) {
  e.preventDefault();
}, false);

let passiveSupported = false;
try {
  const probe = Object.defineProperty({}, "passive", {
    get: function () {
      passiveSupported = true;
      return false;
    },
  }) as AddEventListenerOptions;
  const noop = function (): void {};
  window.addEventListener("omni-probe", noop, probe);
  window.removeEventListener("omni-probe", noop, probe);
} catch (_e) {
  // old browser: third argument is the capture flag
}

/** Elements that may scroll or hold a text caret keep their touch moves. */
function keepsTouchMove(node: EventTarget | null): boolean {
  let cur = node as HTMLElement | null;
  while (cur && cur !== document.body) {
    if (cur.tagName === "INPUT" || (cur.className && String(cur.className).indexOf("scroll") !== -1)) return true;
    cur = cur.parentNode as HTMLElement | null;
  }
  return false;
}
document.addEventListener("touchmove", function (e) {
  if (!keepsTouchMove(e.target)) e.preventDefault();
}, passiveSupported ? { passive: false } : false);

// Pre-iOS-11 home-screen apps draw under the translucent status bar and have no safe-area insets.
if (
  (navigator as Navigator & { standalone?: boolean }).standalone === true &&
  !(window.CSS && CSS.supports && (CSS.supports("padding-top", "env(safe-area-inset-top)") || CSS.supports("padding-top", "constant(safe-area-inset-top)")))
) {
  document.documentElement.className += " legacy-ios-standalone";
}

// ---- start ---------------------------------------------------------------------------

showScreen("splash");

/** Created last: connecting reports status synchronously, so everything above must exist. */
const conn = connect(
  {
    onStatus: onStatus,
    onMessage: onMessage,
  },
  function () {
    return { t: "hello", token: loadToken(), client: clientInfo() };
  },
);
