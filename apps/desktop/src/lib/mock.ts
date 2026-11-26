// In-memory backend so the UI runs in a plain browser (`pnpm --filter @omni/desktop dev`).
// Mirrors the omni-core defaults (themes, voice presets, starter profile) and fakes a
// paired deck, mixer apps, OBS and Home Assistant. Loaded only outside Tauri (lib/backend.ts).
import type {
  Action,
  AppAudio,
  AppState,
  AudioDeviceInfo,
  Config,
  Control,
  DeckSettings,
  Device,
  Effect,
  GeneratedControl,
  HostEvent,
  Icon,
  LogEntry,
  MixerState,
  PairingInfo,
  Profile,
  RuntimeState,
  Style,
  Theme,
  VoicePreset,
  VolumeState,
  VolumeTarget,
} from "@omni/protocol";
import type { Api } from "./api";
import type { Backend, UpdateStatus } from "./backend";
import { emptyStyle } from "./controls";
import { MIDNIGHT } from "./theme";
import { rebase } from "./saver";
import { newId } from "./util";

type Colors = [string, string, string, string, string, string, string, string, string, string, string, string, string];

function theme(id: string, name: string, radius: number, c: Colors): Theme {
  const [background, surface, surfaceHover, border, text, textMuted, accent, accentText, success, warning, danger, button, buttonText] = c;
  return {
    id,
    name,
    radius,
    colors: { background, surface, surfaceHover, border, text, textMuted, accent, accentText, success, warning, danger, button, buttonText },
  };
}

// Same values as omni_core::defaults::builtin_themes.
const builtinThemes: Theme[] = [
  MIDNIGHT,
  theme("graphite", "Graphite", 10, ["#111214", "#1a1b1e", "#232428", "#2c2e33", "#ececec", "#9a9ca3", "#a78bfa", "#111214", "#4ade80", "#fbbf24", "#f87171", "#1f2024", "#ececec"]),
  theme("nord", "Nord", 8, ["#2e3440", "#3b4252", "#434c5e", "#4c566a", "#eceff4", "#a7b0c0", "#88c0d0", "#2e3440", "#a3be8c", "#ebcb8b", "#bf616a", "#3b4252", "#eceff4"]),
  theme("dracula", "Dracula", 10, ["#282a36", "#303241", "#3a3c4e", "#44475a", "#f8f8f2", "#9ea3c4", "#bd93f9", "#282a36", "#50fa7b", "#f1fa8c", "#ff5555", "#343746", "#f8f8f2"]),
  theme("synthwave", "Synthwave", 12, ["#0d0221", "#160a33", "#1f1147", "#2d1b5e", "#fdfdff", "#a99bd1", "#ff2a6d", "#ffffff", "#05d9e8", "#f9c80e", "#ff3864", "#1a0b3d", "#ffffff"]),
  theme("forest", "Forest", 10, ["#0b1210", "#111b18", "#182621", "#22332d", "#e8f2ec", "#8fa79a", "#34d399", "#04130d", "#34d399", "#facc15", "#f87171", "#13201c", "#e8f2ec"]),
  theme("paper", "Paper", 10, ["#f6f6f4", "#ffffff", "#ededea", "#dededa", "#111111", "#6b6b6b", "#2563eb", "#ffffff", "#16a34a", "#d97706", "#dc2626", "#ffffff", "#111111"]),
  theme("contrast", "High Contrast", 4, ["#000000", "#000000", "#1a1a1a", "#ffffff", "#ffffff", "#e6e6e6", "#ffff00", "#000000", "#00ff00", "#ffa500", "#ff3030", "#000000", "#ffffff"]),
];

const preset = (id: string, name: string, effects: Effect[]): VoicePreset => ({ id, name, effects });

// Same values as omni_core::defaults::builtin_voice_presets.
const builtinVoicePresets: VoicePreset[] = [
  preset("clean", "Clean", []),
  preset("robot", "Robot", [{ type: "robot", frequency: 110, mix: 1 }, { type: "reverb", roomSize: 0.2, damping: 0.5, mix: 0.1 }]),
  preset("chipmunk", "Chipmunk", [{ type: "pitch", semitones: 7 }]),
  preset("deep", "Deep", [{ type: "pitch", semitones: -5 }, { type: "filter", kind: "lowpass", frequency: 6000, q: 0.7 }]),
  preset("demon", "Demon", [
    { type: "pitch", semitones: -8 },
    { type: "distortion", drive: 0.35, mix: 0.5 },
    { type: "reverb", roomSize: 0.7, damping: 0.4, mix: 0.3 },
  ]),
  preset("radio", "Radio", [
    { type: "filter", kind: "bandpass", frequency: 1500, q: 0.9 },
    { type: "distortion", drive: 0.25, mix: 0.6 },
  ]),
  preset("telephone", "Telephone", [{ type: "filter", kind: "bandpass", frequency: 1700, q: 1.4 }, { type: "bitcrush", bits: 8, downsample: 2 }]),
  preset("echo", "Echo", [{ type: "echo", delayMs: 280, feedback: 0.45, mix: 0.4 }]),
  preset("cathedral", "Cathedral", [{ type: "reverb", roomSize: 0.92, damping: 0.25, mix: 0.45 }]),
  preset("alien", "Alien", [
    { type: "pitch", semitones: 4 },
    { type: "ringMod", frequency: 60, mix: 0.5 },
    { type: "flanger", rateHz: 0.3, depthMs: 3, feedback: 0.6, mix: 0.5 },
  ]),
  preset("megaphone", "Megaphone", [
    { type: "filter", kind: "highpass", frequency: 600, q: 0.7 },
    { type: "filter", kind: "lowpass", frequency: 4000, q: 0.7 },
    { type: "distortion", drive: 0.6, mix: 0.8 },
  ]),
  preset("underwater", "Underwater", [
    { type: "filter", kind: "lowpass", frequency: 700, q: 1.2 },
    { type: "chorus", rateHz: 0.8, depthMs: 8, mix: 0.6 },
  ]),
];

const builtin = (name: string): Icon => ({ type: "builtin", name });

function button(x: number, y: number, label: string, icon: string, action: Action, style: Partial<Style> = {}): Control {
  return {
    id: newId(),
    x,
    y,
    w: 1,
    h: 1,
    style: { ...emptyStyle(), label, icon: builtin(icon), ...style },
    kind: { type: "button", action },
    hotkey: null,
  };
}

/** Shows `activeLabel` and `activeIcon` on red while active (omni_core `toggle_button`). */
const toggleStyle = (activeLabel: string, activeIcon: string): Partial<Style> => ({
  activeLabel,
  activeIcon: builtin(activeIcon),
  activeBg: "#b91c1c",
});

// Same layout as omni_core::defaults::default_profile.
function defaultProfile(): Profile {
  const media = (command: "playPause" | "previous" | "next"): Action => ({ type: "media", command });
  const master = (op: Extract<Action, { type: "volume" }>["op"]): Action => ({ type: "volume", target: { type: "master" }, op });
  return {
    id: newId(),
    name: "Default",
    rows: 3,
    cols: 5,
    themeId: null,
    pages: [
      {
        id: newId(),
        name: "Main",
        controls: [
          button(0, 0, "Play / pause", "play", media("playPause")),
          button(1, 0, "Previous", "skip-back", media("previous")),
          button(2, 0, "Next", "skip-forward", media("next")),
          button(3, 0, "Open browser", "globe", { type: "open", target: "https://www.google.com" }),
          button(0, 1, "Volume down", "volume-1", master({ type: "step", delta: -0.05 })),
          button(1, 1, "Volume up", "volume-2", master({ type: "step", delta: 0.05 })),
          button(2, 1, "Mute sound", "volume-off", master({ type: "toggleMute" }), toggleStyle("Sound muted", "volume-x")),
          button(
            3,
            1,
            "Mute mic",
            "mic",
            { type: "volume", target: { type: "mic" }, op: { type: "toggleMute" } },
            toggleStyle("Mic muted", "mic-off"),
          ),
          button(0, 2, "Voice effects", "audio-waveform", { type: "voice", op: { type: "toggle" } }, { activeLabel: "Effects on" }),
          button(1, 2, "Next voice", "shuffle", { type: "voice", op: { type: "cycle", presetIds: [] } }),
          button(3, 2, "More", "chevron-right", { type: "navigate", to: { type: "nextPage" } }),
          {
            id: newId(),
            x: 4,
            y: 0,
            w: 1,
            h: 3,
            style: { ...emptyStyle(), label: "Volume", icon: builtin("volume-2") },
            kind: { type: "slider", target: { type: "master" }, vertical: true },
            hotkey: null,
          },
        ],
      },
      {
        id: newId(),
        name: "More",
        controls: [button(0, 0, "Back", "chevron-left", { type: "navigate", to: { type: "prevPage" } })],
      },
    ],
  };
}

/** A second profile so the editor has something realistic to switch to. */
function streamingProfile(): Profile {
  const obs = (op: Extract<Action, { type: "obs" }>["op"]): Action => ({ type: "obs", op });
  return {
    id: newId(),
    name: "Streaming",
    rows: 3,
    cols: 4,
    themeId: "synthwave",
    pages: [
      {
        id: newId(),
        name: "Live",
        controls: [
          button(0, 0, "Starting", "clapperboard", obs({ type: "setScene", scene: "Starting Soon" })),
          button(1, 0, "Main", "monitor", obs({ type: "setScene", scene: "Main" })),
          button(2, 0, "BRB", "coffee", obs({ type: "setScene", scene: "BRB" })),
          button(3, 0, "Record", "circle", obs({ type: "toggleRecord" }), { activeBg: "#dc2626", activeLabel: "REC" }),
          button(0, 1, "Go live", "radio", obs({ type: "toggleStream" }), { activeBg: "#dc2626", activeLabel: "LIVE" }),
          button(1, 1, "Mute mic", "mic-off", obs({ type: "toggleMute", input: "Mic/Aux" })),
          button(2, 1, "Clip it", "scissors", obs({ type: "saveReplay" })),
          button(3, 1, "Discord", "volume-x", { type: "volume", target: { type: "app", app: "Discord" }, op: { type: "toggleMute" } }),
          {
            ...button(0, 2, "Desk lamp", "lamp-desk", { type: "homeAssistant", op: { type: "toggle", entityId: "light.desk" } }),
            w: 2,
          },
          button(2, 2, "Hype", "sparkles", {
            type: "multi",
            delayMs: 150,
            steps: [
              { type: "hotkey", keys: "CmdOrCtrl+Shift+H" },
              { type: "typeText", text: "Let's go!" },
            ],
          }),
          button(3, 2, "Demon", "skull", { type: "voice", op: { type: "setPreset", presetId: "demon" } }),
        ],
      },
    ],
  };
}

const deckDefaults: DeckSettings = { haptics: true, keepAwake: true, dim: 0, showStatusBar: true, autoRotate: true };

function initialConfig(): Config {
  const main = defaultProfile();
  const streaming = streamingProfile();
  const now = Date.now();
  const devices: Device[] = [
    {
      id: "dev-pixel",
      name: "Pixel 4a",
      tokenHash: "",
      profileId: null,
      platform: "Android 13 · Chrome",
      pairedAt: now - 9 * 86400e3,
      lastSeen: now,
      deck: deckDefaults,
    },
    {
      id: "dev-ipad",
      name: "Kitchen iPad",
      tokenHash: "",
      profileId: streaming.id,
      platform: "iPadOS 12 · Safari",
      pairedAt: now - 30 * 86400e3,
      lastSeen: now - 5 * 3600e3,
      deck: { ...deckDefaults, dim: 0.3, haptics: false },
    },
  ];
  return {
    schema: 1,
    settings: {
      hostName: "",
      port: 7710,
      launchAtLogin: false,
      startHidden: false,
      checkForUpdates: true,
      themeId: "midnight",
      defaultProfileId: main.id,
      usb: { enabled: true, adbPath: null },
      ai: { provider: "openai", model: "gpt-6-luna", baseUrl: null, apiKey: "" },
      obs: { enabled: false, host: "127.0.0.1", port: 4455, password: "" },
      homeAssistant: { enabled: false, url: "http://homeassistant.local:8123", token: "" },
      voice: {
        enabled: false,
        presetId: "robot",
        inputDevice: null,
        outputDevice: null,
        monitor: false,
        monitorDevice: null,
        inputGainDb: 0,
        outputGainDb: 0,
        noiseGate: true,
        gateThresholdDb: -50,
      },
    },
    profiles: [main, streaming],
    devices,
    themes: [],
    voicePresets: [],
    appLevels: [],
  };
}

// `?fresh` starts with no devices paired, like a first launch.
let config = initialConfig();
if (new URLSearchParams(location.search).has("fresh")) config = { ...config, devices: [] };
let pairing: PairingInfo | null = null;
let pairTimer: ReturnType<typeof setTimeout> | undefined;
const images = new Map<string, string>();

const mixer: { master: VolumeState; mic: VolumeState; apps: AppAudio[] } = {
  master: { volume: 0.62, muted: false },
  mic: { volume: 0.8, muted: false },
  apps: [
    { id: "com.spotify.client", name: "Spotify", pid: 812, volume: 0.7, muted: false, playing: true },
    { id: "com.hnc.Discord", name: "Discord", pid: 944, volume: 0.55, muted: false, playing: false },
    { id: "com.google.Chrome", name: "Google Chrome", pid: 1201, volume: 1, muted: false, playing: true },
    { id: "us.zoom.xos", name: "zoom.us", pid: 1550, volume: 0.9, muted: true, playing: false },
  ],
};

const audioDevices: { inputs: AudioDeviceInfo[]; outputs: AudioDeviceInfo[] } = {
  inputs: [
    { id: "builtin-mic", name: "MacBook Pro Microphone", isDefault: true, isVirtual: false },
    { id: "mv7", name: "Shure MV7", isDefault: false, isVirtual: false },
    { id: "blackhole-in", name: "BlackHole 2ch", isDefault: false, isVirtual: true },
  ],
  outputs: [
    { id: "builtin-speakers", name: "MacBook Pro Speakers", isDefault: true, isVirtual: false },
    { id: "airpods", name: "AirPods Pro", isDefault: false, isVirtual: false },
    { id: "blackhole-out", name: "BlackHole 2ch", isDefault: false, isVirtual: true },
  ],
};

const os = /mac/i.test(navigator.platform) ? "macos" : /win/i.test(navigator.platform) ? "windows" : "linux";

function runtime(): RuntimeState {
  const { settings } = config;
  const voice = settings.voice;
  return {
    server: {
      running: true,
      port: settings.port,
      urls: [`http://192.168.1.24:${settings.port}`, `http://10.10.10.182:${settings.port}`],
      error: settings.port < 1024 ? `Port ${settings.port} needs admin rights. Pick a port above 1024.` : null,
    },
    sessions: config.devices.length === 0 ? [] : [
      {
        deviceId: "dev-pixel",
        transport: "wifi",
        address: "192.168.1.57",
        profileId: config.profiles[0]?.id ?? "",
        pageId: config.profiles[0]?.pages[0]?.id ?? "",
        connectedAt: Date.now() - 42 * 60e3,
      },
    ],
    pairing,
    usb: {
      enabled: settings.usb.enabled,
      adbPath: settings.usb.enabled ? (settings.usb.adbPath ?? "/opt/homebrew/bin/adb") : null,
      devices: settings.usb.enabled ? [{ serial: "R52N40ABC1", model: "Galaxy Tab A7", state: "device", forwarded: true }] : [],
      error: null,
    },
    obs: settings.obs.enabled
      ? unavailableObs() ? { state: "error", message: "Couldn't connect to OBS. Check the address and port, and make sure OBS is running." } : { state: "connected", message: "OBS 31.1.2, obs-websocket 5.6" }
      : { state: "disabled", message: null },
    homeAssistant: settings.homeAssistant.enabled
      ? unavailableUrl(settings.homeAssistant.url) ? { state: "error", message: "Couldn't connect to Home Assistant. Check the address and make sure it is running." } : settings.homeAssistant.token
        ? { state: "connected", message: "Home Assistant 2026.9" }
        : { state: "error", message: "401 Unauthorized: check the access token" }
      : { state: "disabled", message: null },
    voice: {
      running: voice.enabled && voice.outputDevice !== null,
      presetId: voice.presetId,
      error: voice.enabled && voice.outputDevice === null ? "Pick an output device for the processed voice." : null,
      virtualMic: voice.outputDevice?.startsWith("blackhole") ? "BlackHole 2ch" : null,
      hint:
        os === "macos"
          ? "Install BlackHole (brew install blackhole-2ch), select it as output here and as the microphone in your apps."
          : os === "windows"
            ? "Install VB-CABLE, select CABLE Input as output here and CABLE Output as the microphone in your apps."
            : null,
    },
    audio: {
      perAppVolume: true,
      note: os === "macos" ? "Per-app volume needs System Settings > Privacy & Security > Screen & System Audio Recording." : null,
    },
  };
}

const listeners = new Set<(event: HostEvent) => void>();
const emit = (event: HostEvent) => {
  for (const listener of listeners) listener(structuredClone(event));
};
const emitConfig = () => {
  emit({ type: "config", config });
  emit({ type: "runtime", runtime: runtime() });
};
const log = (level: LogEntry["level"], message: string) => emit({ type: "log", entry: { at: Date.now(), level, message } });

/** Simulates IPC: async, and nothing is shared by reference with the caller. */
async function reply<T>(value: T, ms = 20): Promise<T> {
  await new Promise((resolve) => setTimeout(resolve, ms));
  return structuredClone(value);
}

function mutate(recipe: (c: Config) => Config) {
  config = recipe(structuredClone(config));
  emitConfig();
}

function volumeOf(target: VolumeTarget): VolumeState | AppAudio | undefined {
  switch (target.type) {
    case "master":
      return mixer.master;
    case "mic":
      return mixer.mic;
    case "app":
      return mixer.apps.find((a) => a.id === target.app || a.name.toLowerCase() === target.app.toLowerCase());
    case "obsInput":
      return undefined;
  }
}

const isProfile = (value: unknown): value is Profile =>
  typeof value === "object" && value !== null && "name" in value && "pages" in value && Array.isArray(value.pages);

function freshIds(profile: Profile): Profile {
  return {
    ...profile,
    id: newId(),
    pages: profile.pages.map((page) => ({ ...page, id: newId(), controls: page.controls.map((c) => ({ ...c, id: newId() })) })),
  };
}

function fakeQr(text: string): string {
  const n = 25;
  let seed = [...text].reduce((h, ch) => (h * 31 + ch.charCodeAt(0)) >>> 0, 7);
  const rand = () => ((seed = (seed * 1103515245 + 12345) >>> 0) >>> 16) & 1;
  const finder = (x: number, y: number) =>
    `<path d="M${x} ${y}h7v7h-7z M${x + 1} ${y + 1}v5h5v-5z" fill-rule="evenodd"/><rect x="${x + 2}" y="${y + 2}" width="3" height="3"/>`;
  const inFinder = (x: number, y: number) => (x < 8 && y < 8) || (x > n - 9 && y < 8) || (x < 8 && y > n - 9);
  let cells = "";
  for (let y = 0; y < n; y++) for (let x = 0; x < n; x++) if (!inFinder(x, y) && rand()) cells += `M${x} ${y}h1v1h-1z`;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="-2 -2 ${n + 4} ${n + 4}" shape-rendering="crispEdges"><rect x="-2" y="-2" width="${n + 4}" height="${n + 4}" fill="#fff"/><g fill="#000">${finder(0, 0)}${finder(n - 7, 0)}${finder(0, n - 7)}<path d="${cells}"/></g></svg>`;
}

function fakeGenerate(prompt: string): GeneratedControl {
  const p = prompt.toLowerCase();
  if (p.includes("discord")) {
    return {
      label: "Mute Discord",
      icon: "volume-x",
      action: { type: "volume", target: { type: "app", app: "Discord" }, op: { type: "toggleMute" } },
      explanation: "Toggles mute on Discord's audio output.",
    };
  }
  if (p.includes("scene") || p.includes("obs") || p.includes("brb")) {
    return {
      label: "BRB",
      icon: "coffee",
      action: {
        type: "multi",
        delayMs: 0,
        steps: [
          { type: "obs", op: { type: "setScene", scene: "BRB" } },
          { type: "obs", op: { type: "toggleMute", input: "Mic/Aux" } },
        ],
      },
      explanation: "Switches OBS to the BRB scene and mutes your mic input. Needs the OBS integration.",
    };
  }
  if (p.includes("light") || p.includes("lamp")) {
    return {
      label: "Desk lamp",
      icon: "lamp-desk",
      action: { type: "homeAssistant", op: { type: "toggle", entityId: "light.desk" } },
      explanation: "Toggles light.desk in Home Assistant.",
    };
  }
  if (p.includes("git") || p.includes("script") || p.includes("terminal")) {
    return {
      label: "Git pull",
      icon: "git-pull-request",
      action: { type: "runScript", interpreter: "bash", script: "cd ~/code/project && git pull --ff-only", showOutput: true, timeoutSecs: 30 },
      explanation: "Pulls the latest changes in ~/code/project and shows the result on the deck.",
    };
  }
  if (p.includes("spotify") || p.includes("music")) {
    return {
      label: "Focus music",
      icon: "music",
      action: {
        type: "multi",
        delayMs: 0,
        steps: [
          { type: "openApp", app: "Spotify", args: [] },
          { type: "delay", ms: 1500 },
          { type: "media", command: "playPause" },
        ],
      },
      explanation: "Opens Spotify, waits for it to start and presses play.",
    };
  }
  return {
    label: "Search",
    icon: "search",
    action: { type: "open", target: `https://duckduckgo.com/?q=${encodeURIComponent(prompt)}` },
    explanation: "Opens a web search for your description.",
  };
}

/** Deterministic refused-connection fixtures; no network or real credentials are used. */
function unavailableUrl(address: string | null): boolean {
  if (!address) return false;
  try {
    return new URL(address.includes("://") ? address : `http://${address}`).port === "9";
  } catch {
    return false;
  }
}
const unavailableObs = () => config.settings.obs.port === 9 || unavailableUrl(config.settings.obs.host);
const checkAiConnection = () => {
  if (unavailableUrl(config.settings.ai.baseUrl)) throw "request failed: error sending request";
};

const aiReady = () => {
  const ai = config.settings.ai;
  return ai.apiKey.trim() !== "" || ai.provider === "ollama" || ai.provider === "custom";
};

const api: Api = {
  getState: (): Promise<AppState> =>
    reply({
      config,
      logs: [],
      runtime: runtime(),
      builtinThemes,
      builtinVoicePresets,
      info: {
        version: "1.0.0",
        os,
        arch: "aarch64",
        machineName: "Docs-Mac-mini",
        dataDir: "/Users/you/Library/Application Support/dev.omnipanel.desktop",
      },
    }),
  setSettings: async (settings) => {
    const portChanged = settings.port !== config.settings.port;
    mutate((c) => ({ ...c, settings: structuredClone(settings) }));
    if (portChanged) log("info", `Ready for devices on port ${settings.port}`);
    return reply(undefined);
  },
  patchSettings: async (patch) => {
    const port = config.settings.port;
    mutate((c) => ({ ...c, settings: rebase(c.settings, patch) }));
    if (port !== config.settings.port) log("info", `Ready for devices on port ${config.settings.port}`);
    return reply(undefined);
  },
  saveProfile: async (profile) => {
    mutate((c) => {
      const exists = c.profiles.some((p) => p.id === profile.id);
      const copy = structuredClone(profile);
      return { ...c, profiles: exists ? c.profiles.map((p) => (p.id === profile.id ? copy : p)) : [...c.profiles, copy] };
    });
    return reply(undefined);
  },
  deleteProfile: async (id) => {
    if (config.profiles.length <= 1) throw "Can't delete the last profile";
    mutate((c) => ({ ...c, profiles: c.profiles.filter((p) => p.id !== id) }));
    return reply(undefined);
  },
  exportProfile: async (id, path) => {
    const profile = config.profiles.find((p) => p.id === id);
    if (!profile) throw `Profile ${id} not found`;
    const link = document.createElement("a");
    link.href = URL.createObjectURL(new Blob([JSON.stringify(profile, null, 2)], { type: "application/json" }));
    link.download = path.split(/[\\/]/).pop() ?? "profile.json";
    link.click();
    URL.revokeObjectURL(link.href);
    return reply(undefined);
  },
  importProfile: async (path) => {
    const text = await (await fetch(path)).text();
    const parsed: unknown = JSON.parse(text);
    if (!isProfile(parsed)) throw "Not an OmniPanel profile";
    const profile = freshIds(parsed);
    mutate((c) => ({ ...c, profiles: [...c.profiles, profile] }));
    return reply(profile);
  },
  updateDevice: async (id, name, profileId, deck) => {
    mutate((c) => ({ ...c, devices: c.devices.map((d) => (d.id === id ? { ...d, name, profileId, deck } : d)) }));
    return reply(undefined);
  },
  unpairDevice: async (id) => {
    mutate((c) => ({ ...c, devices: c.devices.filter((d) => d.id !== id) }));
    return reply(undefined);
  },
  saveTheme: async (theme) => {
    if (builtinThemes.some((t) => t.id === theme.id)) throw "Built-in themes can't be changed";
    mutate((c) => ({
      ...c,
      themes: c.themes.some((t) => t.id === theme.id) ? c.themes.map((t) => (t.id === theme.id ? theme : t)) : [...c.themes, theme],
    }));
    return reply(undefined);
  },
  deleteTheme: async (id) => {
    mutate((c) => ({ ...c, themes: c.themes.filter((t) => t.id !== id) }));
    return reply(undefined);
  },
  saveVoicePreset: async (preset) => {
    mutate((c) => ({
      ...c,
      voicePresets: c.voicePresets.some((p) => p.id === preset.id)
        ? c.voicePresets.map((p) => (p.id === preset.id ? preset : p))
        : [...c.voicePresets, preset],
    }));
    return reply(undefined);
  },
  deleteVoicePreset: async (id) => {
    mutate((c) => ({ ...c, voicePresets: c.voicePresets.filter((p) => p.id !== id) }));
    return reply(undefined);
  },

  startPairing: async () => {
    const info = { code: String(Math.floor(100000 + Math.random() * 900000)), expiresAt: Date.now() + 10 * 60e3 };
    pairing = info;
    emit({ type: "runtime", runtime: runtime() });
    clearTimeout(pairTimer);
    // Pretend a phone enters the code after a while.
    pairTimer = setTimeout(() => {
      const device: Device = {
        id: newId(),
        name: "Galaxy Tab A7",
        tokenHash: "",
        profileId: null,
        platform: "Android 11 · Chrome",
        pairedAt: Date.now(),
        lastSeen: Date.now(),
        deck: deckDefaults,
      };
      pairing = null;
      mutate((c) => ({ ...c, devices: [...c.devices, device] }));
      emit({ type: "paired", device });
      log("info", `Connected ${device.name}`);
    }, 20000);
    return reply(info);
  },
  stopPairing: async () => {
    clearTimeout(pairTimer);
    pairing = null;
    emit({ type: "runtime", runtime: runtime() });
    return reply(undefined);
  },
  qrSvg: (text) => reply(fakeQr(text)),

  runAction: async (action) => {
    await reply(undefined, 150);
    switch (action.type) {
      case "obs":
        if (!config.settings.obs.enabled || unavailableObs()) throw "OBS is not connected";
        break;
      case "homeAssistant":
        if (!config.settings.homeAssistant.enabled) throw "Home Assistant is not connected";
    if (unavailableUrl(config.settings.homeAssistant.url)) throw "error sending request";
        break;
      case "runScript":
        return { message: action.showOutput ? "Already up to date." : null };
      case "http":
        return { message: `${action.method} ${action.url}: 200 OK` };
      default:
        break;
    }
    log("info", `Ran ${action.type}`);
    return { message: null };
  },
  installedApps: () =>
    reply(
      ["Spotify", "Discord", "Google Chrome", "Safari", "Visual Studio Code", "OBS", "Slack", "Terminal", "Finder", "Zoom", "Notion", "Figma"].map(
        (name) => ({ name, launch: name }),
      ),
    ),
  importImage: async (path) => {
    const id = `${newId()}.png`;
    images.set(id, path);
    return reply(id);
  },

  getMixer: (): Promise<MixerState> =>
    reply({
      master: mixer.master,
      mic: mixer.mic,
      apps: mixer.apps.map((a) => ({ ...a, playing: a.playing && !a.muted })),
      capabilities: runtime().audio,
    }),
  setVolume: async (target, value) => {
    const state = volumeOf(target);
    if (!state) throw "Unknown audio target";
    state.volume = value;
    return reply(undefined, 5);
  },
  setMuted: async (target, muted) => {
    const state = volumeOf(target);
    if (!state) throw "Unknown audio target";
    state.muted = muted;
    return reply(undefined, 5);
  },

  voiceDevices: () => reply(audioDevices),
  voiceLevels: () => {
    const on = config.settings.voice.enabled;
    const t = Date.now() / 1000;
    const input = on ? Math.abs(Math.sin(t * 2.1) * Math.sin(t * 0.7)) * 0.8 : 0;
    return reply({ input, output: on ? input * 0.85 : 0 }, 2);
  },
  createVirtualMic: async () => {
    const id = "omnipanel-virtual-mic";
    if (!audioDevices.outputs.some((d) => d.id === id)) {
      audioDevices.outputs.push({ id, name: "OmniPanel Virtual Mic", isDefault: false, isVirtual: true });
    }
    return reply(id, 300);
  },

  obsInventory: async () => {
    if (!config.settings.obs.enabled || unavailableObs()) throw "OBS is not connected";
    return reply({
      scenes: ["Starting Soon", "Main", "BRB", "Ending"],
      inputs: ["Mic/Aux", "Desktop Audio", "Webcam", "Music"],
      sceneItems: [
        { scene: "Starting Soon", sources: ["Countdown", "Music"] },
        { scene: "Main", sources: ["Webcam", "Display Capture", "Chat", "Alerts"] },
        { scene: "BRB", sources: ["BRB Loop", "Music"] },
        { scene: "Ending", sources: ["Credits"] },
      ],
    });
  },
  haEntities: async () => {
    if (!config.settings.homeAssistant.enabled) throw "Home Assistant is not connected";
    if (unavailableUrl(config.settings.homeAssistant.url)) throw "error sending request";
    return reply([
      { entityId: "light.desk", name: "Desk lamp", state: "on" },
      { entityId: "light.living_room", name: "Living room", state: "off" },
      { entityId: "switch.coffee_machine", name: "Coffee machine", state: "off" },
      { entityId: "scene.movie_time", name: "Movie time", state: "scening" },
      { entityId: "media_player.kitchen", name: "Kitchen speaker", state: "playing" },
    ]);
  },

  aiGenerate: async (request) => {
    await reply(undefined, 900);
    checkAiConnection();
    if (!aiReady()) throw "AI is not configured: add an API key in Settings";
    const base = fakeGenerate(request.prompt);
    return request.history.length ? { ...base, explanation: `${base.explanation} Refined: ${request.prompt}` } : base;
  },
  aiModels: async () => {
    checkAiConnection();
    const models: Record<string, string[]> = {
      openai: ["gpt-6-luna", "gpt-6", "gpt-6-mini"],
      anthropic: ["claude-sonnet", "claude-haiku", "claude-opus"],
      ollama: ["llama3.3", "qwen3", "mistral-small"],
      openrouter: ["openai/gpt-6-luna", "anthropic/claude-sonnet", "meta-llama/llama-4"],
      custom: [],
    };
    return reply(models[config.settings.ai.provider] ?? []);
  },
  aiTest: async () => {
    await reply(undefined, 600);
    checkAiConnection();
    if (!aiReady()) throw "Missing API key";
    return `Connected. ${config.settings.ai.model} answered in 412 ms.`;
  },

  openDataDir: () => reply(undefined),
  quit: () => reply(undefined),
};

/** File picker stand-in: returns a blob URL that `importImage` / `importProfile` understand. */
function pickFile(accept: string): Promise<string | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    input.addEventListener("change", () => {
      const file = input.files?.[0];
      resolve(file ? URL.createObjectURL(file) : null);
    });
    input.addEventListener("cancel", () => resolve(null));
    input.click();
  });
}

// Pretends a newer version exists once "Check now" is clicked, so the update UI can be tried.
let updateStatus: UpdateStatus = { state: "idle" };
const updateListeners = new Set<(status: UpdateStatus) => void>();
const setUpdateStatus = (status: UpdateStatus) => {
  updateStatus = status;
  updateListeners.forEach((listener) => listener(status));
};
const MOCK_UPDATE = "9.9.9";

export const backend: Backend = {
  api,
  onHostEvent: async (handler) => {
    listeners.add(handler);
    return () => listeners.delete(handler);
  },
  dialog: {
    open: async ({ filters, directory }) => {
      if (directory) return window.prompt("Folder path (mock)", "/Users/doc/Downloads");
      return pickFile((filters ?? []).flatMap((f) => f.extensions.map((e) => `.${e}`)).join(","));
    },
    save: async (defaultPath) => defaultPath,
  },
  imageUrl: (_port, id) => images.get(id) ?? "",
  updater: {
    status: async () => updateStatus,
    onStatus: async (handler) => {
      updateListeners.add(handler);
      return () => updateListeners.delete(handler);
    },
    check: async () => {
      setUpdateStatus({ state: "checking" });
      await reply(null, 600);
      setUpdateStatus({ state: "available", version: MOCK_UPDATE, canInstall: true });
    },
    download: async () => {
      for (const percent of [0, 35, 70, 100]) {
        setUpdateStatus({ state: "downloading", version: MOCK_UPDATE, percent });
        await reply(null, 400);
      }
      setUpdateStatus({ state: "ready", version: MOCK_UPDATE });
    },
    install: async () => setUpdateStatus({ state: "upToDate" }),
  },
};

// A few log lines so the log view isn't empty.
setTimeout(() => {
  log("info", "Ready for devices on port 7710");
  log("warn", `The keyboard shortcut ${os === "macos" ? "Cmd" : "Ctrl"}+Alt+1 for "Play / pause" is used by another app. Choose a different one.`);
}, 300);
