// "Recipes": the task-named entries of the action picker ("Mute / unmute microphone").
// Each one makes a control kind plus the button face it starts with. Static recipes
// cover every action type; dynamic ones come from installed apps and the mixer.
import type { AppAudio, Control, Icon, InstalledApp, Style } from "@omni/protocol";
import { defaultAction, describeAction, type ActionGroup, type NameLookup } from "./actions.ts";
import type { IconName } from "./icons";

/** What picking an entry produces: the new kind and the button face to apply. */
export interface PickResult {
  kind: Control["kind"];
  style: Partial<Style>;
}

export interface Recipe {
  id: string;
  group: ActionGroup;
  /** Picker row text. */
  label: string;
  /** Extra search words. */
  keywords: string;
  /** Picker row icon, and the icon of the button it makes. */
  icon: IconName;
  /** Text on the button it makes. null keeps the current label and icon. */
  button: string | null;
  /** Look while the host lights the button up (muted, recording...). */
  active?: { label?: string; icon?: IconName; bg?: string };
  make: () => Control["kind"];
  /** App name of a dynamic entry; ranks it first when a query word equals it. */
  name?: string;
}

const MUTED = "#b91c1c";
const ON_AIR = "#dc2626";

export const RECIPES = [
  // Sound
  {
    id: "mic-mute",
    group: "Sound",
    label: "Mute / unmute microphone",
    keywords: "mute mic microphone silence talk",
    icon: "mic",
    button: "Mute mic",
    active: { label: "Mic muted", icon: "mic-off", bg: MUTED },
    make: () => ({ type: "button", action: { type: "volume", target: { type: "mic" }, op: { type: "toggleMute" } } }),
  },
  {
    id: "sound-mute",
    group: "Sound",
    label: "Mute / unmute computer sound",
    keywords: "speaker sound audio all",
    icon: "volume-2",
    button: "Mute sound",
    active: { label: "Sound muted", icon: "volume-x", bg: MUTED },
    make: () => ({ type: "button", action: { type: "volume", target: { type: "master" }, op: { type: "toggleMute" } } }),
  },
  {
    id: "app-mute",
    group: "Sound",
    label: "Mute / unmute an app",
    keywords: "app discord spotify game",
    icon: "volume-x",
    button: "Mute app",
    make: () => ({ type: "button", action: { type: "volume", target: { type: "app", app: "" }, op: { type: "toggleMute" } } }),
  },
  {
    id: "vol-up",
    group: "Sound",
    label: "Turn volume up",
    keywords: "louder",
    icon: "volume-2",
    button: "Volume up",
    make: () => ({ type: "button", action: { type: "volume", target: { type: "master" }, op: { type: "step", delta: 0.05 } } }),
  },
  {
    id: "vol-down",
    group: "Sound",
    label: "Turn volume down",
    keywords: "quieter",
    icon: "volume-1",
    button: "Volume down",
    make: () => ({ type: "button", action: { type: "volume", target: { type: "master" }, op: { type: "step", delta: -0.05 } } }),
  },
  {
    id: "vol-set",
    group: "Sound",
    label: "Set volume to a level",
    keywords: "level percent",
    icon: "volume-1",
    button: "Volume 50%",
    make: () => ({ type: "button", action: { type: "volume", target: { type: "master" }, op: { type: "set", value: 0.5 } } }),
  },
  {
    id: "slider",
    group: "Sound",
    label: "Volume slider",
    keywords: "slider fader mixer",
    icon: "volume-2",
    button: "Volume",
    make: () => ({ type: "slider", target: { type: "master" }, vertical: true }),
  },
  {
    id: "slider-mic",
    group: "Sound",
    label: "Microphone volume slider",
    keywords: "slider fader",
    icon: "mic",
    button: "Mic",
    make: () => ({ type: "slider", target: { type: "mic" }, vertical: true }),
  },
  {
    id: "slider-app",
    group: "Sound",
    label: "App volume slider",
    keywords: "slider fader discord spotify",
    icon: "app-window",
    button: "App",
    make: () => ({ type: "slider", target: { type: "app", app: "" }, vertical: true }),
  },
  // Music and video
  {
    id: "play-pause",
    group: "Music and video",
    label: "Play / pause",
    keywords: "music song pause resume",
    icon: "play",
    button: "Play / pause",
    make: () => ({ type: "button", action: { type: "media", command: "playPause" } }),
  },
  {
    id: "next-track",
    group: "Music and video",
    label: "Next track",
    keywords: "skip song",
    icon: "skip-forward",
    button: "Next",
    make: () => ({ type: "button", action: { type: "media", command: "next" } }),
  },
  {
    id: "prev-track",
    group: "Music and video",
    label: "Previous track",
    keywords: "back song",
    icon: "skip-back",
    button: "Previous",
    make: () => ({ type: "button", action: { type: "media", command: "previous" } }),
  },
  {
    id: "stop-media",
    group: "Music and video",
    label: "Stop playback",
    keywords: "music",
    icon: "square",
    button: "Stop",
    make: () => ({ type: "button", action: { type: "media", command: "stop" } }),
  },
  // Voice changer
  {
    id: "voice-toggle",
    group: "Voice changer",
    label: "Turn voice changer on / off",
    keywords: "voice effect fx changer",
    icon: "audio-waveform",
    button: "Voice effects",
    active: { label: "Effects on" },
    make: () => ({ type: "button", action: { type: "voice", op: { type: "toggle" } } }),
  },
  {
    id: "voice-next",
    group: "Voice changer",
    label: "Next voice",
    keywords: "cycle change",
    icon: "shuffle",
    button: "Next voice",
    make: () => ({ type: "button", action: { type: "voice", op: { type: "cycle", presetIds: [] } } }),
  },
  {
    id: "voice-set",
    group: "Voice changer",
    label: "Switch to a voice",
    keywords: "robot chipmunk deep demon",
    icon: "bot",
    button: "Robot",
    make: () => ({ type: "button", action: { type: "voice", op: { type: "setPreset", presetId: "robot" } } }),
  },
  // Open
  {
    id: "open-app",
    group: "Open",
    label: "Open an app",
    keywords: "launch start program run",
    icon: "app-window",
    button: "Open app",
    make: () => ({ type: "button", action: { type: "openApp", app: "", args: [] } }),
  },
  {
    id: "open-site",
    group: "Open",
    label: "Open a website",
    keywords: "url link browser web",
    icon: "globe",
    button: "Website",
    make: () => ({ type: "button", action: { type: "open", target: "https://" } }),
  },
  {
    id: "open-file",
    group: "Open",
    label: "Open a file or folder",
    keywords: "file folder document path",
    icon: "folder-open",
    button: "Folder",
    make: () => ({ type: "button", action: { type: "open", target: "" } }),
  },
  // Keyboard
  {
    id: "press-keys",
    group: "Keyboard",
    label: "Press keys",
    keywords: "shortcut hotkey keys combination macro",
    icon: "keyboard",
    button: "Keys",
    make: () => ({ type: "button", action: { type: "hotkey", keys: "" } }),
  },
  {
    id: "type-text",
    group: "Keyboard",
    label: "Type text",
    keywords: "paste snippet write",
    icon: "type",
    button: "Text",
    make: () => ({ type: "button", action: { type: "typeText", text: "" } }),
  },
  // Pages
  {
    id: "next-page",
    group: "Pages",
    label: "Next page",
    keywords: "page more",
    icon: "chevron-right",
    button: "More",
    make: () => ({ type: "button", action: { type: "navigate", to: { type: "nextPage" } } }),
  },
  {
    id: "prev-page",
    group: "Pages",
    label: "Previous page",
    keywords: "back return",
    icon: "chevron-left",
    button: "Back",
    make: () => ({ type: "button", action: { type: "navigate", to: { type: "prevPage" } } }),
  },
  {
    id: "go-page",
    group: "Pages",
    label: "Go to a page",
    keywords: "page jump folder",
    icon: "layers",
    button: "Page",
    make: () => ({ type: "button", action: { type: "navigate", to: { type: "page", pageId: "" } } }),
  },
  {
    id: "switch-layout",
    group: "Pages",
    label: "Switch layout",
    keywords: "layout profile",
    icon: "layout-grid",
    button: "Layout",
    make: () => ({ type: "button", action: { type: "navigate", to: { type: "profile", profileId: "" } } }),
  },
  // Computer
  {
    id: "lock",
    group: "Computer",
    label: "Lock computer",
    keywords: "screen",
    icon: "lock",
    button: "Lock",
    make: () => ({ type: "button", action: { type: "system", command: "lock" } }),
  },
  {
    id: "sleep",
    group: "Computer",
    label: "Put computer to sleep",
    keywords: "power",
    icon: "moon",
    button: "Sleep",
    make: () => ({ type: "button", action: { type: "system", command: "sleep" } }),
  },
  {
    id: "display-off",
    group: "Computer",
    label: "Turn display off",
    keywords: "screen monitor",
    icon: "monitor-off",
    button: "Display off",
    make: () => ({ type: "button", action: { type: "system", command: "displaySleep" } }),
  },
  // OBS Studio
  {
    id: "obs-scene",
    group: "OBS Studio",
    label: "Switch scene",
    keywords: "obs scene brb",
    icon: "clapperboard",
    button: "Scene",
    make: () => ({ type: "button", action: { type: "obs", op: { type: "setScene", scene: "" } } }),
  },
  {
    id: "obs-record",
    group: "OBS Studio",
    label: "Start / stop recording",
    keywords: "obs record rec",
    icon: "circle",
    button: "Record",
    active: { label: "REC", bg: ON_AIR },
    make: () => ({ type: "button", action: { type: "obs", op: { type: "toggleRecord" } } }),
  },
  {
    id: "obs-stream",
    group: "OBS Studio",
    label: "Go live / end stream",
    keywords: "obs stream live twitch youtube",
    icon: "radio",
    button: "Go live",
    active: { label: "LIVE", bg: ON_AIR },
    make: () => ({ type: "button", action: { type: "obs", op: { type: "toggleStream" } } }),
  },
  {
    id: "obs-replay",
    group: "OBS Studio",
    label: "Save replay",
    keywords: "obs clip replay",
    icon: "scissors",
    button: "Clip it",
    make: () => ({ type: "button", action: { type: "obs", op: { type: "saveReplay" } } }),
  },
  {
    id: "obs-mute",
    group: "OBS Studio",
    label: "Mute / unmute an OBS audio source",
    keywords: "obs mute audio source",
    icon: "mic-off",
    button: "OBS mic",
    make: () => ({ type: "button", action: { type: "obs", op: { type: "toggleMute", input: "" } } }),
  },
  {
    id: "obs-source",
    group: "OBS Studio",
    label: "Show / hide a source",
    keywords: "obs source camera overlay",
    icon: "eye",
    button: "Show / hide",
    make: () => ({ type: "button", action: { type: "obs", op: { type: "toggleSource", scene: "", source: "" } } }),
  },
  // Home Assistant
  {
    id: "ha-toggle",
    group: "Home Assistant",
    label: "Turn a device on / off",
    keywords: "home assistant light lamp switch plug smart",
    icon: "lightbulb",
    button: "Light",
    make: () => ({ type: "button", action: { type: "homeAssistant", op: { type: "toggle", entityId: "" } } }),
  },
  {
    id: "ha-service",
    group: "Home Assistant",
    label: "Run a Home Assistant service",
    keywords: "scene script automation",
    icon: "house",
    button: "Home",
    make: () => ({
      type: "button",
      action: { type: "homeAssistant", op: { type: "callService", domain: "", service: "", entityId: null, data: null } },
    }),
  },
  // Advanced
  {
    id: "multi",
    group: "Advanced",
    label: "Several actions in a row",
    keywords: "multi macro sequence",
    icon: "list-ordered",
    button: "Steps",
    make: () => ({ type: "button", action: { type: "multi", steps: [], delayMs: 0 } }),
  },
  {
    id: "toggle",
    group: "Advanced",
    label: "Alternate between two actions",
    keywords: "toggle alternate",
    icon: "toggle-right",
    button: "Toggle",
    make: () => ({ type: "button", action: defaultAction("toggle") }),
  },
  {
    id: "script",
    group: "Advanced",
    label: "Run a script",
    keywords: "script shell terminal command python applescript",
    icon: "terminal",
    button: "Script",
    make: () => ({ type: "button", action: defaultAction("runScript") }),
  },
  {
    id: "http",
    group: "Advanced",
    label: "Send a web request",
    keywords: "http webhook api url request",
    icon: "webhook",
    button: "Web request",
    make: () => ({ type: "button", action: defaultAction("http") }),
  },
  {
    id: "none",
    group: "Advanced",
    label: "Nothing (label only)",
    keywords: "nothing blank empty label",
    icon: "square-dashed",
    button: null,
    make: () => ({ type: "button", action: null }),
  },
] as const satisfies readonly Recipe[];

// ---------------------------------------------------------------------------
// Dynamic entries

const APP_ICONS: Record<string, IconName> = {
  spotify: "music",
  discord: "message-circle",
  chrome: "globe",
  safari: "globe",
  firefox: "globe",
  obs: "video",
  steam: "gamepad-2",
  terminal: "terminal",
};

const appIcon = (name: string): IconName =>
  name
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .map((word) => APP_ICONS[word])
    .find((icon) => icon !== undefined) ?? "app-window";

/** Picker entries for installed apps ("Open Spotify") and apps playing sound ("Mute / unmute Spotify"). */
export function dynamicRecipes(installed: readonly InstalledApp[], playing: readonly AppAudio[]): Recipe[] {
  const opens = installed.map(
    (app): Recipe => ({
      id: `open:${app.name}`,
      group: "Open",
      label: `Open ${app.name}`,
      keywords: "launch start app",
      icon: appIcon(app.name),
      button: app.name,
      name: app.name,
      make: () => ({ type: "button", action: { type: "openApp", app: app.launch, args: [] } }),
    }),
  );
  const mutes = playing.map(
    ({ name }): Recipe => ({
      id: `mute:${name}`,
      group: "Sound",
      label: `Mute / unmute ${name}`,
      keywords: "mute sound volume",
      icon: "volume-2",
      button: `Mute ${name}`,
      active: { label: `${name} muted`, icon: "volume-x", bg: MUTED },
      name,
      make: () => ({ type: "button", action: { type: "volume", target: { type: "app", app: name }, op: { type: "toggleMute" } } }),
    }),
  );
  const sliders = playing.map(
    ({ name }): Recipe => ({
      id: `slider:${name}`,
      group: "Sound",
      label: `${name} volume slider`,
      keywords: "slider fader",
      icon: appIcon(name),
      button: name,
      name,
      make: () => ({ type: "slider", target: { type: "app", app: name }, vertical: true }),
    }),
  );
  const recipes = [...opens, ...mutes, ...sliders];
  // Dynamic faces count as automatic labels when changing an existing button too.
  for (const recipe of recipes) {
    if (recipe.button !== null) AUTO_LABELS.add(recipe.button);
  }
  return recipes;
}

// ---------------------------------------------------------------------------
// Search

const STOPWORDS = new Set(["my", "the", "a", "an", "to", "on", "off", "button", "for", "make", "i", "want", "please"]);

const wordsOf = (text: string) => text.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);

/**
 * Picker search. Every query word (stopwords dropped) must prefix a word of the
 * label, keywords or app name. Ranked: dynamic entries named by a query word,
 * then labels starting with the query, then the rest. An empty query lists the
 * static recipes in their group order; dynamic entries only show while searching.
 */
export function searchRecipes(query: string, dynamic: readonly Recipe[]): readonly Recipe[] {
  const words = wordsOf(query).filter((w) => !STOPWORDS.has(w));
  if (query.trim() === "") return RECIPES;
  const recipes: readonly Recipe[] = [...RECIPES, ...dynamic];
  const phrase = words.join(" ");
  const rank = (r: Recipe) => {
    const name = r.name ? wordsOf(r.name) : [];
    if (name.some((n) => words.includes(n))) return 0;
    return r.label.toLowerCase().startsWith(phrase) ? 1 : 2;
  };
  return recipes
    .filter((r) => {
      const haystack = wordsOf(`${r.label} ${r.keywords} ${r.name ?? ""}`);
      return words.every((w) => haystack.some((h) => h.startsWith(w)));
    })
    .map((r) => ({ r, rank: rank(r) }))
    .sort((a, b) => a.rank - b.rank)
    .map(({ r }) => r);
}

// ---------------------------------------------------------------------------
// Picking

const builtin = (name: IconName): Icon => ({ type: "builtin", name });

/** The kind and full button face an entry makes. "Nothing" carries no face. */
export function pickOf(recipe: Recipe): PickResult {
  const kind = recipe.make();
  if (recipe.button === null) return { kind, style: {} };
  return {
    kind,
    style: {
      label: recipe.button,
      icon: builtin(recipe.icon),
      activeLabel: recipe.active?.label ?? null,
      activeIcon: recipe.active?.icon ? builtin(recipe.active.icon) : null,
      activeBg: recipe.active?.bg ?? null,
    },
  };
}

/** Labels the v1.0 palette gave new buttons, so those still count as untouched. */
const OLD_LABELS = [
  "Hotkey",
  "Type text",
  "Open app",
  "Open URL or file",
  "Run script",
  "Media key",
  "Volume",
  "Voice changer",
  "OBS",
  "Home Assistant",
  "HTTP request",
  "System",
  "Navigate deck",
  "Multi action",
  "Toggle",
  "Delay",
  "Slider",
];

const AUTO_LABELS = new Set<string>([
  ...RECIPES.flatMap((r) => (r.button === null ? [] : [r.button])),
  ...OLD_LABELS,
]);

/** True when a label wasn't typed by the user (empty, or one a recipe or the old palette set). */
export const isAutoLabel = (label: string): boolean => label.trim() === "" || AUTO_LABELS.has(label.trim());

/**
 * Applies a pick to an existing control's style. An auto label gets the new face
 * (label, icon and live look); a label the user typed is kept, and only a missing
 * icon is filled in.
 */
export function applyPick(style: Style, pick: PickResult, previous?: Control["kind"], names: LabelNames = {}): Style {
  if (isAutoLabel(style.label) || (previous && style.label === labelFor(previous, names))) return { ...style, ...pick.style };
  return { ...style, icon: style.icon ?? pick.style.icon ?? null };
}

type LabelNames = NameLookup & { app?: (launch: string) => string | undefined };

/** The target-specific face of a recipe, also used to recognize its previous automatic label. */
function labelFor(kind: Control["kind"], names: LabelNames): string | null {
  const soundName = (target: Extract<Control["kind"], { type: "slider" }>["target"]) =>
    target.type === "app" ? target.app || "app" : target.type === "obsInput" ? target.input || "OBS audio" : target.type === "mic" ? "mic" : "sound";
  if (kind.type === "slider") {
    return kind.target.type === "master" ? "Volume" : kind.target.type === "mic" ? "Mic" :
      kind.target.type === "app" ? kind.target.app || "App" : kind.target.input || "OBS audio";
  }
  const action = kind.action;
  if (!action) return null;
  switch (action.type) {
    case "openApp":
      return names.app?.(action.app) ?? action.app.split(/[/\\]/).filter(Boolean).at(-1)?.replace(/\.(app|exe)$/i, "") ?? "Open app";
    case "open":
      try {
        return new URL(action.target).hostname || action.target.split(/[/\\]/).filter(Boolean).at(-1) || "Website";
      } catch {
        return action.target === "https://" ? "Website" : action.target.split(/[/\\]/).filter(Boolean).at(-1) || "Folder";
      }
    case "navigate":
      return action.to.type === "page" ? names.page?.(action.to.pageId) ?? "Page" :
        action.to.type === "profile" ? names.profile?.(action.to.profileId) ?? "Layout" : action.to.type === "nextPage" ? "More" : "Back";
    case "volume":
      return action.op.type === "set" ? `Volume ${Math.round(action.op.value * 100)}%` :
        action.op.type === "step" ? (action.op.delta < 0 ? "Volume down" : "Volume up") :
          `${action.op.type === "unmute" ? "Unmute" : "Mute"} ${soundName(action.target)}`;
    case "voice":
      return action.op.type === "setPreset" ? names.preset?.(action.op.presetId) ?? action.op.presetId : null;
    case "obs":
      return action.op.type === "setScene" ? action.op.scene || "Scene" :
        action.op.type === "toggleSource" ? action.op.source || "Show / hide" :
          action.op.type === "toggleMute" ? `Mute ${action.op.input || "OBS mic"}` : describeAction(action);
    case "homeAssistant":
      return action.op.type === "toggle" ? names.entity?.(action.op.entityId) ?? (action.op.entityId || "Light") :
        action.op.domain && action.op.service ? `${action.op.domain}.${action.op.service}` : "Home";
    case "http":
      try { return new URL(action.url).hostname || "Web request"; } catch { return "Web request"; }
    case "media":
    case "system":
      return describeAction(action);
    default:
      return null;
  }
}

/** Follow form edits while the label is automatic; a user-written label stays intact. */
export function applyAutoLabel(style: Style, previous: Control["kind"], next: Control["kind"], names: LabelNames = {}): Style {
  if (!isAutoLabel(style.label) && style.label !== labelFor(previous, names)) return style;
  const label = labelFor(next, names);
  return label === null || label === style.label ? style : { ...style, label };
}
