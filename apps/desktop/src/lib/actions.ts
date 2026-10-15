// Metadata, defaults and one-line summaries for every `Action` variant.
import type {
  Action,
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
import type { IconName } from "./icons";

export type ActionType = Action["type"];
export type ActionOf<T extends ActionType> = Extract<Action, { type: T }>;

interface ActionMeta {
  label: string;
  icon: IconName;
  group: string;
}

export const ACTION_META = {
  hotkey: { label: "Hotkey", icon: "keyboard", group: "Keyboard" },
  typeText: { label: "Type text", icon: "type", group: "Keyboard" },
  openApp: { label: "Open app", icon: "app-window", group: "Launch" },
  open: { label: "Open URL or file", icon: "external-link", group: "Launch" },
  runScript: { label: "Run script", icon: "terminal", group: "Launch" },
  media: { label: "Media key", icon: "play", group: "Audio" },
  volume: { label: "Volume", icon: "volume-2", group: "Audio" },
  voice: { label: "Voice changer", icon: "audio-waveform", group: "Audio" },
  obs: { label: "OBS", icon: "video", group: "Integrations" },
  homeAssistant: { label: "Home Assistant", icon: "house", group: "Integrations" },
  http: { label: "HTTP request", icon: "globe", group: "Integrations" },
  system: { label: "System", icon: "power", group: "System" },
  navigate: { label: "Navigate deck", icon: "chevron-right", group: "Deck" },
  multi: { label: "Multi action", icon: "list-ordered", group: "Flow" },
  toggle: { label: "Toggle", icon: "toggle-right", group: "Flow" },
  delay: { label: "Delay", icon: "timer", group: "Flow" },
} satisfies Record<ActionType, ActionMeta>;

export const isActionType = (value: string): value is ActionType => Object.hasOwn(ACTION_META, value);

/** Action types in display order. */
export const ACTION_TYPES: ActionType[] = Object.keys(ACTION_META).filter(isActionType);

export const MEDIA_LABELS: Record<MediaCommand, string> = {
  playPause: "Play / Pause",
  next: "Next track",
  previous: "Previous track",
  stop: "Stop",
  volumeUp: "Volume up",
  volumeDown: "Volume down",
  mute: "Mute",
};

export const SYSTEM_LABELS: Record<SystemCommand, string> = {
  lock: "Lock screen",
  sleep: "Sleep",
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
  master: "System output",
  mic: "Microphone",
  app: "Application",
  obsInput: "OBS input",
};

export const VOLUME_OP_LABELS: Record<VolumeOp["type"], string> = {
  toggleMute: "Toggle mute",
  mute: "Mute",
  unmute: "Unmute",
  set: "Set volume",
  step: "Change by",
};

export const VOICE_OP_LABELS: Record<VoiceOp["type"], string> = {
  toggle: "Toggle on / off",
  enable: "Turn on",
  disable: "Turn off",
  setPreset: "Switch to preset",
  cycle: "Cycle presets",
};

export const OBS_OP_LABELS: Record<ObsOp["type"], string> = {
  setScene: "Switch scene",
  toggleRecord: "Toggle recording",
  startRecord: "Start recording",
  stopRecord: "Stop recording",
  pauseRecord: "Pause recording",
  toggleStream: "Toggle stream",
  startStream: "Start stream",
  stopStream: "Stop stream",
  toggleReplayBuffer: "Toggle replay buffer",
  saveReplay: "Save replay",
  toggleVirtualCam: "Toggle virtual camera",
  toggleMute: "Toggle input mute",
  toggleSource: "Show / hide source",
  toggleStudioMode: "Toggle studio mode",
};

export const NAV_LABELS: Record<NavTarget["type"], string> = {
  nextPage: "Next page",
  prevPage: "Previous page",
  page: "Go to page",
  profile: "Switch profile",
};

export const FILTER_LABELS: Record<FilterKind, string> = {
  lowpass: "Low-pass",
  highpass: "High-pass",
  bandpass: "Band-pass",
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

export const volumeTargetLabel = (target: VolumeTarget): string => {
  switch (target.type) {
    case "master":
      return "System output";
    case "mic":
      return "Microphone";
    case "app":
      return target.app || "App";
    case "obsInput":
      return `OBS ${target.input || "input"}`;
  }
};

const volumeOpLabel = (op: VolumeOp): string => {
  switch (op.type) {
    case "set":
      return `set ${Math.round(op.value * 100)}%`;
    case "step":
      return `${op.delta >= 0 ? "+" : ""}${Math.round(op.delta * 100)}%`;
    case "toggleMute":
    case "mute":
    case "unmute":
      return VOLUME_OP_LABELS[op.type].toLowerCase();
  }
};

/** Resolves ids to names in summaries. */
export interface NameLookup {
  page?: (id: string) => string | undefined;
  profile?: (id: string) => string | undefined;
  preset?: (id: string) => string | undefined;
}

const short = (text: string, max = 32) => (text.length > max ? `${text.slice(0, max - 1)}…` : text);

/** One-line, human readable summary of an action. */
export function describeAction(action: Action, names: NameLookup = {}): string {
  switch (action.type) {
    case "hotkey":
      return action.keys ? `Press ${action.keys}` : "Press keys";
    case "typeText":
      return action.text ? `Type "${short(action.text, 24)}"` : "Type text";
    case "openApp":
      return action.app ? `Open ${short(action.app)}` : "Open app";
    case "open":
      return `Open ${short(action.target)}`;
    case "runScript":
      return `Run ${INTERPRETER_LABELS[action.interpreter]} script`;
    case "media":
      return MEDIA_LABELS[action.command];
    case "volume":
      return `${volumeTargetLabel(action.target)}: ${volumeOpLabel(action.op)}`;
    case "voice": {
      const op = action.op;
      switch (op.type) {
        case "setPreset":
          return `Voice: ${names.preset?.(op.presetId) ?? op.presetId}`;
        case "cycle":
          return op.presetIds.length ? `Cycle ${op.presetIds.length} voices` : "Cycle voices";
        case "toggle":
        case "enable":
        case "disable":
          return `Voice changer: ${VOICE_OP_LABELS[op.type].toLowerCase()}`;
      }
    }
    case "obs": {
      const op = action.op;
      switch (op.type) {
        case "setScene":
          return `OBS scene ${op.scene || "?"}`;
        case "toggleMute":
          return `OBS mute ${op.input || "?"}`;
        case "toggleSource":
          return `OBS ${op.source || "source"} in ${op.scene || "scene"}`;
        default:
          return `OBS: ${OBS_OP_LABELS[op.type].toLowerCase()}`;
      }
    }
    case "homeAssistant": {
      const op = action.op;
      switch (op.type) {
        case "toggle":
          return `Toggle ${op.entityId || "entity"}`;
        case "callService":
          return `${op.domain || "domain"}.${op.service || "service"}${op.entityId ? ` ${op.entityId}` : ""}`;
      }
    }
    case "http":
      return `${action.method} ${short(action.url.replace(/^https?:\/\//, ""))}`;
    case "system":
      return SYSTEM_LABELS[action.command];
    case "navigate": {
      const to = action.to;
      switch (to.type) {
        case "nextPage":
        case "prevPage":
          return NAV_LABELS[to.type];
        case "page":
          return `Go to ${names.page?.(to.pageId) ?? "page"}`;
        case "profile":
          return `Switch to ${names.profile?.(to.profileId) ?? "profile"}`;
      }
    }
    case "multi":
      return `${action.steps.length} step${action.steps.length === 1 ? "" : "s"}`;
    case "toggle":
      return `${describeAction(action.on, names)} / ${describeAction(action.off, names)}`;
    case "delay":
      return `Wait ${action.ms} ms`;
  }
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
  ringMod: "Ring modulator",
  echo: "Echo",
  reverb: "Reverb",
  distortion: "Distortion",
  filter: "Filter",
  chorus: "Chorus",
  flanger: "Flanger",
  tremolo: "Tremolo",
  bitcrush: "Bitcrusher",
  gain: "Gain",
};
