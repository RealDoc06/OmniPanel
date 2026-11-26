// Metadata, defaults and one-line summaries for every `Action` variant.
// Labels follow the plain-words vocabulary of the desktop UI (no "hotkey", "deck" or "profile").
import type {
  Action,
  AppAudio,
  AudioDeviceInfo,
  AudioDirection,
  Control,
  Effect,
  FilterKind,
  HttpMethod,
  Interpreter,
  MediaCommand,
  NavTarget,
  ObsOp,
  SystemCommand,
  VoiceOp,
  VolumeOp,
  VolumeTarget,
} from "@omni/protocol";
import { formatAccelerator } from "./accelerator.ts";
import type { IconName } from "./icons";
import { isMacPlatform } from "./util.ts";

export type ActionType = Action["type"];
export type ActionOf<T extends ActionType> = Extract<Action, { type: T }>;

/** Groups in display order, shared by the action picker and the step "Does" select. */
export const ACTION_GROUPS = [
  "Sound",
  "Music and video",
  "Voice changer",
  "Open",
  "Keyboard",
  "Pages",
  "Computer",
  "OBS Studio",
  "Home Assistant",
  "Advanced",
] as const;

export type ActionGroup = (typeof ACTION_GROUPS)[number];

interface ActionMeta {
  label: string;
  icon: IconName;
  group: ActionGroup;
}

export const ACTION_META = {
  volume: { label: "Volume and mute", icon: "volume-2", group: "Sound" },
  appDevice: { label: "Speaker or microphone of an app", icon: "speaker", group: "Sound" },
  media: { label: "Play, pause and skip", icon: "play", group: "Music and video" },
  voice: { label: "Voice changer", icon: "audio-waveform", group: "Voice changer" },
  openApp: { label: "Open an app", icon: "app-window", group: "Open" },
  open: { label: "Open a website, file or folder", icon: "external-link", group: "Open" },
  hotkey: { label: "Press keys", icon: "keyboard", group: "Keyboard" },
  typeText: { label: "Type text", icon: "type", group: "Keyboard" },
  navigate: { label: "Change page or layout", icon: "chevron-right", group: "Pages" },
  system: { label: "Computer", icon: "power", group: "Computer" },
  obs: { label: "OBS Studio", icon: "video", group: "OBS Studio" },
  homeAssistant: { label: "Home Assistant", icon: "house", group: "Home Assistant" },
  multi: { label: "Several actions in a row", icon: "list-ordered", group: "Advanced" },
  toggle: { label: "Alternate between two actions", icon: "toggle-right", group: "Advanced" },
  runScript: { label: "Run a script", icon: "terminal", group: "Advanced" },
  http: { label: "Web request", icon: "webhook", group: "Advanced" },
  delay: { label: "Wait", icon: "timer", group: "Advanced" },
} satisfies Record<ActionType, ActionMeta>;

export const isActionType = (value: string): value is ActionType => Object.hasOwn(ACTION_META, value);

/** Action types in display order. */
export const ACTION_TYPES: ActionType[] = Object.keys(ACTION_META).filter(isActionType);

export const MEDIA_LABELS: Record<MediaCommand, string> = {
  playPause: "Play / pause",
  next: "Next track",
  previous: "Previous track",
  stop: "Stop",
  volumeUp: "Volume up key",
  volumeDown: "Volume down key",
  mute: "Mute key",
};

export const SYSTEM_LABELS: Record<SystemCommand, string> = {
  lock: "Lock computer",
  sleep: "Put computer to sleep",
  displaySleep: "Turn display off",
};

export const INTERPRETER_LABELS: Record<Interpreter, string> = {
  shell: "Shell",
  bash: "Bash",
  powershell: "PowerShell",
  python: "Python",
  node: "Node.js",
  applescript: "AppleScript",
};

export const HTTP_METHODS: Record<HttpMethod, string> = {
  GET: "GET",
  POST: "POST",
  PUT: "PUT",
  PATCH: "PATCH",
  DELETE: "DELETE",
};

export const VOLUME_TARGET_LABELS: Record<VolumeTarget["type"], string> = {
  master: "Computer sound",
  mic: "Microphone",
  app: "An app",
  obsInput: "OBS audio source",
};

export const VOLUME_OP_LABELS: Record<VolumeOp["type"], string> = {
  toggleMute: "Mute / unmute",
  mute: "Mute",
  unmute: "Unmute",
  set: "Set to",
  step: "Turn up or down by",
};

export const APP_DEVICE_LABELS: Record<AudioDirection, string> = {
  output: "Speaker",
  input: "Microphone",
};

export const VOICE_OP_LABELS: Record<VoiceOp["type"], string> = {
  toggle: "Turn on / off",
  enable: "Turn on",
  disable: "Turn off",
  setPreset: "Switch to a voice",
  cycle: "Next voice",
};

export const OBS_OP_LABELS: Record<ObsOp["type"], string> = {
  setScene: "Switch scene",
  toggleRecord: "Start / stop recording",
  startRecord: "Start recording",
  stopRecord: "Stop recording",
  pauseRecord: "Pause recording",
  toggleStream: "Go live / end stream",
  startStream: "Go live",
  stopStream: "End stream",
  toggleReplayBuffer: "Replay buffer on / off",
  saveReplay: "Save replay",
  toggleVirtualCam: "Virtual camera on / off",
  toggleMute: "Mute / unmute an audio source",
  toggleSource: "Show / hide a source",
  toggleStudioMode: "Studio mode on / off",
};

export const NAV_LABELS: Record<NavTarget["type"], string> = {
  nextPage: "Next page",
  prevPage: "Previous page",
  page: "A page",
  profile: "Another layout",
};

export const FILTER_LABELS: Record<FilterKind, string> = {
  lowpass: "Muffled (low-pass)",
  highpass: "Thin (high-pass)",
  bandpass: "Narrow (band-pass)",
};

/** A fresh action of the given type with sensible defaults. */
export function defaultAction(type: ActionType): Action {
  switch (type) {
    case "hotkey":
      return { type, keys: "" };
    case "typeText":
      return { type, text: "" };
    case "openApp":
      return { type, app: "", args: [] };
    case "open":
      return { type, target: "https://" };
    case "runScript":
      return { type, interpreter: "shell", script: "", showOutput: false, timeoutSecs: 30 };
    case "media":
      return { type, command: "playPause" };
    case "volume":
      return { type, target: { type: "master" }, op: { type: "toggleMute" } };
    case "appDevice":
      return { type, app: "", direction: "output", device: null };
    case "voice":
      return { type, op: { type: "toggle" } };
    case "obs":
      return { type, op: { type: "toggleRecord" } };
    case "homeAssistant":
      return { type, op: { type: "toggle", entityId: "" } };
    case "http":
      return { type, method: "POST", url: "https://", headers: [], body: null };
    case "system":
      return { type, command: "lock" };
    case "navigate":
      return { type, to: { type: "nextPage" } };
    case "multi":
      return { type, steps: [], delayMs: 0 };
    case "toggle":
      return { type, on: defaultAction("media"), off: defaultAction("media") };
    case "delay":
      return { type, ms: 500 };
  }
}

/** Lowercase name of a volume target for summaries ("computer sound", "Spotify"). */
const volumeTargetPhrase = (target: VolumeTarget): string => {
  switch (target.type) {
    case "master":
      return "computer sound";
    case "mic":
      return "microphone";
    case "app":
      return target.app || "an app";
    case "obsInput":
      return `${target.input || "an audio source"} in OBS`;
  }
};

const volumeSummary = (target: VolumeTarget, op: VolumeOp): string => {
  const what = volumeTargetPhrase(target);
  switch (op.type) {
    case "toggleMute":
      return `Mute / unmute ${what}`;
    case "mute":
      return `Mute ${what}`;
    case "unmute":
      return `Unmute ${what}`;
    case "set":
      return `Set ${what} to ${Math.round(op.value * 100)}%`;
    case "step":
      return `Turn ${what} ${op.delta >= 0 ? "up" : "down"} ${Math.abs(Math.round(op.delta * 100))}%`;
  }
};

/** Resolves ids to names in summaries. */
export interface NameLookup {
  page?: (id: string) => string | undefined;
  profile?: (id: string) => string | undefined;
  preset?: (id: string) => string | undefined;
  entity?: (id: string) => string | undefined;
  /** Speaker (`output`) or microphone (`input`) by id, or as it was written. */
  device?: (device: string, direction: AudioDirection) => string | undefined;
}

/** An app that records without playing anything: it has a microphone but no volume or speaker. */
export const recordsOnly = (app: AppAudio): boolean => app.output === null && app.input !== null;

/**
 * The device an `appDevice` action means, by id or by name like the host matches it
 * (`best_match` in crates/omni-audio/src/lib.rs: id, then exact name, then part of a name).
 */
export function findDevice(devices: readonly AudioDeviceInfo[], query: string): AudioDeviceInfo | undefined {
  const needle = query.trim().toLowerCase();
  if (needle === "") return undefined;
  return (
    devices.find((d) => d.id.toLowerCase() === needle) ??
    devices.find((d) => d.name.toLowerCase() === needle) ??
    devices.find((d) => d.name.toLowerCase().includes(needle))
  );
}

/** Whether the action, or one nested in it, chooses an app's device. */
export function usesAppDevice(action: Action | null): boolean {
  switch (action?.type) {
    case "appDevice":
      return true;
    case "multi":
      return action.steps.some(usesAppDevice);
    case "toggle":
      return usesAppDevice(action.on) || usesAppDevice(action.off);
    default:
      return false;
  }
}

const short = (text: string, max = 32) => (text.length > max ? `${text.slice(0, max - 1)}…` : text);

/** Seconds for display: 500 -> "0.5". */
export const seconds = (ms: number) => String(Math.round(ms / 10) / 100);

const hostOf = (url: string): string | null => {
  try {
    return new URL(url).host || null;
  } catch {
    return null;
  }
};

/** One-line, plain-words summary of an action ("Mute / unmute microphone"). */
export function describeAction(action: Action | null, names: NameLookup = {}): string {
  if (!action) return "Does nothing yet";
  switch (action.type) {
    case "hotkey":
      return action.keys ? `Press ${formatAccelerator(action.keys, isMacPlatform())}` : "Press keys";
    case "typeText":
      return action.text ? `Type "${short(action.text, 24)}"` : "Type text";
    case "openApp":
      return action.app ? `Open ${short(action.app)}` : "Open an app";
    case "open": {
      const target = action.target.trim().replace(/^https?:\/\//, "");
      return target ? `Open ${short(target)}` : "Open a website, file or folder";
    }
    case "runScript": {
      const language = INTERPRETER_LABELS[action.interpreter];
      return `Run ${/^[AEIOU]/.test(language) ? "an" : "a"} ${language} script`;
    }
    case "media":
      return MEDIA_LABELS[action.command];
    case "volume":
      return volumeSummary(action.target, action.op);
    case "appDevice": {
      const app = action.app || "an app";
      const device = action.device === null ? null : short(names.device?.(action.device, action.direction) ?? action.device);
      return action.direction === "output"
        ? `Play ${app} on ${device ?? "the default speaker"}`
        : `Use ${device ?? "the default microphone"} for ${app}`;
    }
    case "voice": {
      const op = action.op;
      switch (op.type) {
        case "setPreset":
          return `Switch to ${names.preset?.(op.presetId) ?? op.presetId} voice`;
        case "cycle":
          return op.presetIds.length ? `Next voice (${op.presetIds.length} voices)` : "Next voice";
        case "toggle":
          return "Turn voice changer on / off";
        case "enable":
          return "Turn voice changer on";
        case "disable":
          return "Turn voice changer off";
      }
    }
    case "obs": {
      const op = action.op;
      switch (op.type) {
        case "setScene":
          return op.scene ? `Switch OBS to ${op.scene}` : "Switch OBS scene";
        case "toggleMute":
          return `Mute / unmute ${op.input || "an audio source"} in OBS`;
        case "toggleSource":
          return op.source ? `Show / hide ${op.source} in ${op.scene || "OBS"}` : "Show / hide an OBS source";
        default:
          return `OBS: ${OBS_OP_LABELS[op.type].toLowerCase()}`;
      }
    }
    case "homeAssistant": {
      const op = action.op;
      switch (op.type) {
        case "toggle":
          return op.entityId ? `Turn ${names.entity?.(op.entityId) ?? op.entityId} on / off` : "Turn a device on / off";
        case "callService":
          return op.domain && op.service ? `Run ${op.domain}.${op.service}` : "Run a Home Assistant service";
      }
    }
    case "http": {
      const host = hostOf(action.url);
      return host ? `Send a web request to ${short(host)}` : "Send a web request";
    }
    case "system":
      return SYSTEM_LABELS[action.command];
    case "navigate": {
      const to = action.to;
      switch (to.type) {
        case "nextPage":
        case "prevPage":
          return NAV_LABELS[to.type];
        case "page": {
          const page = names.page?.(to.pageId);
          return page ? `Go to ${page}` : "Go to a page";
        }
        case "profile": {
          const layout = names.profile?.(to.profileId);
          return layout ? `Switch to ${layout} layout` : "Switch layout";
        }
      }
    }
    case "multi":
      return `${action.steps.length} step${action.steps.length === 1 ? "" : "s"}`;
    case "toggle":
      return `${describeAction(action.on, names)} / ${describeAction(action.off, names)}`;
    case "delay":
      return `Wait ${seconds(action.ms)} s`;
  }
}

const OBS_LIVE_TITLES: Record<ObsOp["type"], string> = {
  setScene: "While on this scene",
  toggleRecord: "While recording",
  startRecord: "While recording",
  stopRecord: "While recording",
  pauseRecord: "While paused",
  toggleStream: "While live",
  startStream: "While live",
  stopStream: "While live",
  toggleReplayBuffer: "While the replay buffer is on",
  saveReplay: "While the replay buffer is on",
  toggleVirtualCam: "While the virtual camera is on",
  toggleMute: "While muted",
  toggleSource: "While shown",
  toggleStudioMode: "While in studio mode",
};

function actionLiveTitle(action: Action): string | null {
  switch (action.type) {
    case "volume":
      return action.op.type === "set" || action.op.type === "step" ? null : "While muted";
    case "appDevice":
      // The system default is not a device the app can be seen on.
      return action.device === null ? null : action.direction === "output" ? "While it plays there" : "While it uses this microphone";
    case "toggle":
      return "While on";
    case "voice":
      return action.op.type === "setPreset" ? "While this voice is on" : "While the voice changer is on";
    case "obs":
      return OBS_LIVE_TITLES[action.op.type];
    case "navigate":
      return action.to.type === "page" ? "While on this page" : action.to.type === "profile" ? "While this layout is shown" : null;
    case "multi":
      return action.steps[0] ? actionLiveTitle(action.steps[0]) : null;
    default:
      return null;
  }
}

/**
 * Title of the inspector section for a control's lit-up look ("While muted"), or null
 * when the host never lights it up. Mirrors `action_state` in
 * crates/omni-host/src/feedback.rs: keep both in sync.
 */
export function liveStateTitle(kind: Control["kind"]): string | null {
  if (kind.type === "slider") return "While muted";
  return kind.action ? actionLiveTitle(kind.action) : null;
}

/** Default parameters for a new voice effect. */
export function defaultEffect(type: Effect["type"]): Effect {
  switch (type) {
    case "pitch":
      return { type, semitones: 0 };
    case "robot":
      return { type, frequency: 110, mix: 1 };
    case "ringMod":
      return { type, frequency: 60, mix: 0.5 };
    case "echo":
      return { type, delayMs: 250, feedback: 0.4, mix: 0.35 };
    case "reverb":
      return { type, roomSize: 0.5, damping: 0.5, mix: 0.3 };
    case "distortion":
      return { type, drive: 0.3, mix: 0.5 };
    case "filter":
      return { type, kind: "lowpass", frequency: 4000, q: 0.7 };
    case "chorus":
      return { type, rateHz: 0.8, depthMs: 6, mix: 0.5 };
    case "flanger":
      return { type, rateHz: 0.3, depthMs: 3, feedback: 0.5, mix: 0.5 };
    case "tremolo":
      return { type, rateHz: 5, depth: 0.5 };
    case "bitcrush":
      return { type, bits: 8, downsample: 2 };
    case "gain":
      return { type, db: 0 };
  }
}

export const EFFECT_LABELS: Record<Effect["type"], string> = {
  pitch: "Pitch",
  robot: "Robot",
  ringMod: "Metallic",
  echo: "Echo",
  reverb: "Room echo",
  distortion: "Distortion",
  filter: "Tone filter",
  chorus: "Layered",
  flanger: "Sweep",
  tremolo: "Wobble",
  bitcrush: "Lo-fi",
  gain: "Volume",
};
