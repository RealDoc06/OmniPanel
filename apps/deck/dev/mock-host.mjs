// Mock OmniPanel host for developing the deck client without the desktop app.
//
//   pnpm --filter @omni/deck build && node dev/mock-host.mjs [port]
//
// Serves dist/ over HTTP and speaks the deck protocol on /ws. Pairing code: 123456.
// Dev controls (HTTP GET, or one-letter commands on stdin):
//   /__mock/unpair   send `unpaired` to every deck and forget all tokens
//   /__mock/drop     kill every socket (tests reconnect without restarting the server)
//   /__mock/toast    send a toast to every deck
//   /__mock/layout   resend layouts            /__mock/dim?v=0.5       set the dim level
//   /__mock/grid?cols=8&rows=10  force a grid   /__mock/pages?v=1  limit the page count   /__mock/reset          reset control state
//   /__mock/bar?v=0  hide the status bar       /__mock/page?v=2        jump every deck to a page
import { createServer } from "node:http";
import { randomBytes } from "node:crypto";
import { readFileSync, writeFileSync, existsSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";

const root = dirname(fileURLToPath(import.meta.url));
const dist = join(root, "../dist");
const port = Number(process.argv[2] || process.env.PORT || 8787);
const PAIR_CODE = "123456";
const icons = JSON.parse(readFileSync(join(root, "../../../packages/icons/icons.json"), "utf8"));

// ---- layout model -----------------------------------------------------------------------

// Same values as the built-in "midnight" theme (omni_core::defaults), so the mock looks like a real host.
const theme = {
  background: "#000000", surface: "#0b0b0b", surfaceHover: "#161616", border: "#1f1f1f",
  text: "#ffffff", textMuted: "#8c8c8c", accent: "#4f8cff", accentText: "#ffffff",
  success: "#22c55e", warning: "#f59e0b", danger: "#ef4444", button: "#121212", buttonText: "#ffffff",
};
const settings = { haptics: true, keepAwake: true, dim: 0, showStatusBar: true };
let pageLimit = Infinity; // /__mock/pages?v=1 pretends the profile has a single page

const svg = (name) => ({ type: "svg", markup: icons[name] ?? icons["circle"] });
const emoji = (value) => ({ type: "emoji", value });
const image = (url) => ({ type: "image", url });

/** Control specs; ids are stable so state survives layout changes. `w`/`h` are the preferred span. */
const pages = [
  {
    id: "main",
    name: "Stream",
    controls: [
      { id: "master", slider: "v", w: 1, h: 3, label: "Master", icon: svg("volume-2"), value: 0.62 },
      { id: "mute", toggle: true, label: "Mute Mic", icon: svg("mic"), active: { bg: "#b91c1c", label: "Muted", icon: svg("mic-off") } },
      { id: "cam", toggle: true, label: "Camera", icon: svg("video"), active: { bg: "#15803d", label: "Live", icon: svg("video") } },
      { id: "obs", toggle: true, label: "OBS link", icon: svg("plug"), active: { bg: "#2563eb", label: "Linked", icon: svg("plug-zap") } },
      { id: "rec", label: "Record", icon: svg("circle-dot"), bg: "#7f1d1d", disabled: true, needsObs: true },
      { id: "scene", label: "Scene 1", textOnly: true, bg: "#1e3a8a" },
      { id: "spotify", label: "Spotify", icon: emoji("\u{1F3B5}") },
      { id: "discord", label: "Discord", icon: image("/img/demo.svg"), position: "bottom" },
      { id: "stream", w: 2, label: "Start stream", icon: svg("radio"), bg: "#4c1d95" },
      { id: "long", label: "Toggle all living room lights", icon: svg("lightbulb"), position: "top" },
      { id: "mic", slider: "h", w: 3, h: 1, label: "Mic gain", icon: svg("mic"), value: 0.35 },
      { id: "next", label: "Next page", icon: svg("arrow-right"), nav: "next" },
    ],
  },
  {
    id: "lights",
    name: "Lights",
    controls: [
      { id: "l1", toggle: true, label: "Desk", icon: svg("lamp-desk"), active: { bg: "#ca8a04", icon: svg("lamp-desk") }, on: true },
      { id: "l2", toggle: true, label: "Ceiling", icon: svg("lightbulb"), active: { bg: "#ca8a04", icon: svg("lightbulb") } },
      { id: "l3", toggle: true, label: "Strip", icon: svg("sparkles"), active: { bg: "#a21caf", icon: svg("sparkles") } },
      { id: "l4", toggle: true, label: "Hallway", icon: svg("door-open"), active: { bg: "#ca8a04" } },
      { id: "bright", slider: "h", w: 2, h: 1, label: "Brightness", icon: svg("sun"), value: 0.8 },
      { id: "warm", slider: "h", w: 2, h: 1, label: "Warmth", icon: svg("thermometer-sun"), value: 0.4 },
      { id: "scenes", label: "Movie night", icon: emoji("\u{1F37F}"), position: "center" },
      { id: "off", label: "All off", icon: svg("power"), bg: "#7f1d1d" },
      { id: "prev", label: "Back", icon: svg("arrow-left"), nav: "prev" },
    ],
  },
  {
    id: "apps",
    name: "Apps",
    controls: [
      ["Chrome", "\u{1F310}"], ["Terminal", "\u{2328}\u{FE0F}"], ["Notes", "\u{1F4DD}"], ["Mail", "\u{2709}\u{FE0F}"],
      ["Music", "\u{1F3A7}"], ["Photos", "\u{1F5BC}\u{FE0F}"], ["Maps", "\u{1F5FA}\u{FE0F}"], ["Calendar", "\u{1F4C5}"],
      ["Slack", "\u{1F4AC}"], ["Files", "\u{1F4C1}"],
    ].map(([label, icon], i) => ({
      id: "app" + i, label, icon: emoji(icon), position: ["bottom", "top", "center", "hidden"][i % 4],
    })).concat([{ id: "back2", label: "Home", icon: svg("house"), nav: "first" }]),
  },
];

const specs = new Map();
for (const page of pages) for (const spec of page.controls) specs.set(spec.id, spec);

/** Live state shared by every connected deck. */
const live = new Map();
function resetState() {
  live.clear();
  for (const spec of specs.values()) {
    live.set(spec.id, {
      id: spec.id,
      active: !!spec.on,
      value: spec.slider ? spec.value : null,
      label: null,
      disabled: !!spec.disabled,
    });
  }
}
resetState();

/** First-fit packing so the same pages fill any cols x rows grid. */
function pack(specsOfPage, cols, rows) {
  const taken = Array.from({ length: rows }, () => Array(cols).fill(false));
  const placed = [];
  for (const spec of specsOfPage) {
    let w = Math.min(spec.w ?? 1, cols);
    let h = Math.min(spec.h ?? 1, rows);
    // A vertical slider in a short grid becomes as tall as it can; a wide one as wide.
    for (let y = 0; y + h <= rows; y++) {
      for (let x = 0; x + w <= cols; x++) {
        let free = true;
        for (let yy = y; yy < y + h && free; yy++) for (let xx = x; xx < x + w; xx++) if (taken[yy][xx]) { free = false; break; }
        if (!free) continue;
        for (let yy = y; yy < y + h; yy++) for (let xx = x; xx < x + w; xx++) taken[yy][xx] = true;
        placed.push({ spec, x, y, w, h });
        y = rows;
        break;
      }
    }
  }
  return placed;
}

function deckControl({ spec, x, y, w, h }) {
  const state = live.get(spec.id);
  const base = spec.bg ?? theme.button;
  return {
    id: spec.id, x, y, w, h,
    kind: spec.slider ? { type: "slider", vertical: spec.slider === "v" } : { type: "button" },
    label: spec.label,
    icon: spec.textOnly ? null : spec.icon ?? null,
    bg: base,
    fg: "#ffffff",
    labelPosition: spec.position ?? "bottom",
    active: { bg: spec.active?.bg ?? theme.accent, label: spec.active?.label ?? null, icon: spec.active?.icon ?? null },
    state,
  };
}

/** Phones in portrait get 3 columns, wider screens 5 (like the desktop app's default grids). */
let gridOverride = null; // /__mock/grid?cols=8&rows=10 forces a grid (stress-test tiny cells)
function gridFor(client) {
  if (gridOverride) return gridOverride;
  const portrait = client.height > client.width;
  return portrait ? { cols: 3, rows: 5 } : { cols: 5, rows: 3 };
}

function layoutFor(session) {
  const page = pages[Math.min(session.page, pageLimit - 1)];
  const { cols, rows } = gridFor(session.client);
  return {
    profileId: "demo", profileName: "Streaming", pageId: page.id, pageName: page.name,
    pageIndex: Math.min(session.page, pageLimit - 1), pageCount: Math.min(pages.length, pageLimit),
    pages: pages.slice(0, pageLimit).map((p) => ({ id: p.id, name: p.name })), rows, cols, theme, radius: 10, settings,
    controls: pack(page.controls, cols, rows).map(deckControl),
  };
}

// ---- protocol ---------------------------------------------------------------------------

// Tokens survive restarts of the mock (like the real host), so reconnect tests keep working.
const tokenFile = join(tmpdir(), "omnipanel-mock-tokens.json");
const tokens = new Set(existsSync(tokenFile) ? JSON.parse(readFileSync(tokenFile, "utf8")) : []);
const saveTokens = () => writeFileSync(tokenFile, JSON.stringify([...tokens]));
const sessions = new Set();
let toastCount = 0;

function send(session, msg) {
  if (session.ws.readyState === 1) session.ws.send(JSON.stringify(msg));
}
function broadcast(msg) {
  for (const s of sessions) if (s.authed) send(s, msg);
}
function toast(session, text, level) {
  send(session, { t: "toast", text, level });
}
function sendLayout(session) {
  send(session, { t: "layout", layout: layoutFor(session) });
}
function admit(session, name) {
  session.authed = true;
  if (name) session.deviceName = name;
  send(session, { t: "welcome", deviceId: session.id, deviceName: session.deviceName, hostName: "Doc's Mac mini", hostVersion: "1.0.0-mock" });
  sendLayout(session);
}
function patch(id, change) {
  const next = { ...live.get(id), ...change };
  live.set(id, next);
  broadcast({ t: "state", states: [next] });
}

function onPress(session, id) {
  const spec = specs.get(id);
  if (!spec) return;
  const state = live.get(id);
  toastCount++;
  if (spec.nav) {
    session.page = spec.nav === "next" ? (session.page + 1) % pages.length : spec.nav === "prev" ? Math.max(0, session.page - 1) : 0;
    sendLayout(session);
    return;
  }
  if (state.disabled) {
    toast(session, "OBS is offline", "error");
    return;
  }
  if (spec.toggle) {
    patch(id, { active: !state.active });
    if (id === "obs") {
      const on = !state.active;
      patch("rec", { disabled: !on });
      toast(session, on ? "OBS connected" : "OBS disconnected", on ? "success" : "info");
    } else {
      toast(session, spec.label + (!state.active ? " on" : " off"), "success");
    }
    return;
  }
  if (id === "rec") {
    patch("rec", { active: !state.active, label: state.active ? null : "REC 00:01" });
    return;
  }
  if (id === "off") {
    for (const s of specs.values()) if (s.toggle && s.id.startsWith("l")) patch(s.id, { active: false });
    toast(session, "All lights off", "success");
    return;
  }
  const levels = ["info", "success", "error"];
  toast(session, "Pressed " + spec.label, levels[toastCount % 3]);
}

function onMessage(session, msg) {
  switch (msg.t) {
    case "hello":
      if (msg.client) session.client = { ...session.client, ...msg.client };
      if (msg.token && tokens.has(msg.token)) {
        session.token = msg.token;
        admit(session);
      } else send(session, { t: "needPair", hostName: "Doc's Mac mini" });
      return;
    case "pair": {
      if (msg.code === PAIR_CODE) {
        const token = randomBytes(16).toString("hex");
        tokens.add(token);
        session.token = token;
        saveTokens();
        session.deviceName = msg.name || "Deck";
        send(session, { t: "pairResult", ok: true, token, error: null });
        admit(session, msg.name);
      } else {
        send(session, { t: "pairResult", ok: false, token: null, error: "Wrong code. Check the code on your computer." });
      }
      return;
    }
    case "ping":
      send(session, { t: "pong" });
      return;
  }
  if (!session.authed) return;
  switch (msg.t) {
    case "unpair":
      tokens.delete(session.token);
      saveTokens();
      send(session, { t: "unpaired" });
      session.ws.close();
      return;
    case "press":
      onPress(session, msg.control);
      return;
    case "slide": {
      const spec = specs.get(msg.control);
      if (!spec || !spec.slider) return;
      const value = Math.min(1, Math.max(0, msg.value));
      live.set(msg.control, { ...live.get(msg.control), value });
      // Echo like a real host would (after a short delay); other decks see it too.
      setTimeout(() => broadcast({ t: "state", states: [live.get(msg.control)] }), 40);
      return;
    }
    case "nav":
      if (msg.to.type === "nextPage") session.page = (session.page + 1) % pages.length;
      else if (msg.to.type === "prevPage") session.page = (session.page + pages.length - 1) % pages.length;
      else if (msg.to.type === "page") session.page = Math.max(0, pages.findIndex((p) => p.id === msg.to.pageId));
      sendLayout(session);
      return;
    case "resize":
      session.client = { ...session.client, width: msg.width, height: msg.height };
      console.log(`resize ${session.id}: ${msg.width}x${msg.height}`);
      sendLayout(session);
      return;
  }
}

// ---- servers ----------------------------------------------------------------------------

const types = {
  ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8",
  ".json": "application/json", ".webmanifest": "application/manifest+json", ".png": "image/png", ".svg": "image/svg+xml",
};

const demoImage = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#5865f2"/><stop offset="1" stop-color="#9b59ff"/></linearGradient></defs><rect x="8" y="8" width="112" height="112" rx="28" fill="url(#g)"/><circle cx="46" cy="62" r="10" fill="#fff"/><circle cx="82" cy="62" r="10" fill="#fff"/><path d="M40 90c14 8 34 8 48 0" stroke="#fff" stroke-width="8" stroke-linecap="round" fill="none"/></svg>`;

function control(path, query) {
  switch (path) {
    case "unpair":
      broadcast({ t: "unpaired" });
      tokens.clear();
      saveTokens();
      return "unpaired all decks";
    case "drop":
      for (const s of sessions) s.ws.terminate();
      return "dropped all sockets";
    case "toast":
      broadcast({ t: "toast", text: "Hello from the host", level: ["info", "success", "error"][toastCount++ % 3] });
      return "toast sent";
    case "layout":
      for (const s of sessions) if (s.authed) sendLayout(s);
      return "layouts resent";
    case "dim":
      settings.dim = Number(query.get("v") ?? 0);
      for (const s of sessions) if (s.authed) sendLayout(s);
      return "dim " + settings.dim;
    case "bar":
      settings.showStatusBar = query.get("v") !== "0";
      for (const s of sessions) if (s.authed) sendLayout(s);
      return "bar " + settings.showStatusBar;
    case "grid":
      gridOverride = query.get("cols") ? { cols: Number(query.get("cols")), rows: Number(query.get("rows")) } : null;
      for (const s of sessions) if (s.authed) sendLayout(s);
      return "grid " + JSON.stringify(gridOverride);
    case "pages":
      pageLimit = Number(query.get("v") ?? 99);
      for (const s of sessions) if (s.authed) sendLayout(s);
      return "page limit " + pageLimit;
    case "page":
      for (const s of sessions) if (s.authed) { s.page = Number(query.get("v") ?? 0) % pages.length; sendLayout(s); }
      return "page set";
    case "reset":
      resetState();
      for (const s of sessions) if (s.authed) sendLayout(s);
      return "state reset";
    default:
      return null;
  }
}

const server = createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  if (url.pathname.startsWith("/__mock/")) {
    const result = control(url.pathname.slice(8), url.searchParams);
    res.writeHead(result ? 200 : 404, { "content-type": "text/plain" });
    res.end(result ?? "unknown command");
    return;
  }
  if (url.pathname === "/img/demo.svg") {
    res.writeHead(200, { "content-type": "image/svg+xml" });
    res.end(demoImage);
    return;
  }
  if (url.pathname === "/api/info") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ name: "Doc's Mac mini", version: "1.0.0-mock", protocol: 1 }));
    return;
  }
  const rel = normalize(decodeURIComponent(url.pathname)).replace(/^(\.\.[/\\])+/, "");
  let file = join(dist, rel);
  if (existsSync(file) && statSync(file).isDirectory()) file = join(file, "index.html");
  if (!file.startsWith(dist) || !existsSync(file)) {
    res.writeHead(404, { "content-type": "text/plain" });
    res.end("not found");
    return;
  }
  res.writeHead(200, { "content-type": types[extname(file)] ?? "application/octet-stream", "cache-control": "no-store" });
  res.end(readFileSync(file));
});

const wss = new WebSocketServer({ noServer: true });
server.on("upgrade", (req, socket, head) => {
  if (new URL(req.url ?? "/", "http://localhost").pathname !== "/ws") return socket.destroy();
  wss.handleUpgrade(req, socket, head, (ws) => wss.emit("connection", ws, req));
});

let nextId = 1;
wss.on("connection", (ws) => {
  const session = {
    ws, id: "dev-" + nextId++, authed: false, page: 0, deviceName: "Deck",
    client: { width: 1280, height: 800, userAgent: "", pixelRatio: 1, protocol: 1 },
  };
  sessions.add(session);
  console.log(`+ ${session.id} connected (${sessions.size})`);
  ws.on("message", (data) => {
    let msg;
    try {
      msg = JSON.parse(String(data));
    } catch {
      return;
    }
    if (msg.t !== "ping" && msg.t !== "slide") console.log(`< ${session.id}`, JSON.stringify(msg).slice(0, 160));
    onMessage(session, msg);
  });
  ws.on("close", () => {
    sessions.delete(session);
    console.log(`- ${session.id} gone (${sessions.size})`);
  });
});

server.listen(port, () => {
  console.log(`mock host: http://localhost:${port}  (pair code ${PAIR_CODE})`);
  console.log("stdin: u = unpair all, d = drop sockets, t = toast, l = resend layouts, r = reset state");
});

process.stdin.on("data", (chunk) => {
  const map = { u: "unpair", d: "drop", t: "toast", l: "layout", r: "reset" };
  for (const key of String(chunk).trim()) if (map[key]) console.log(control(map[key], new URLSearchParams()));
});
