// App store: the host `AppState`, kept in sync by host events, plus a little UI state.
// Mutations are optimistic: the store updates immediately and saves go to the host
// (debounced for typing). Until a save is confirmed, incoming `config` events keep
// the local value so the host echo can't overwrite what the user is typing.
import type {
  AppState,
  Config,
  Device,
  HostEvent,
  LogEntry,
  Profile,
  Settings,
  Theme,
  VoicePreset,
} from "@omni/protocol";
import { useMemo } from "react";
import { create } from "zustand";
import { api, imageUrl, onHostEvent } from "./backend";
import { findTheme } from "./theme";
import { toast, toastError } from "./toast";
import { upsert } from "./util";

export type Screen = "decks" | "profiles" | "audio" | "integrations" | "settings" | "log";

export interface AppStore extends AppState {
  screen: Screen;
  /** Section to scroll to after navigating (e.g. "ai" in Settings). */
  anchor: string | null;
  logs: LogEntry[];
  unreadErrors: number;
  /** Last device that paired while the window was open. */
  lastPaired: Device | null;
}

let handleEvent: (event: HostEvent) => void = () => {};
await onHostEvent((event) => handleEvent(event));
const initial = await api.getState();

export const useApp = create<AppStore>()(() => ({
  ...initial,
  screen: initial.config.devices.length > 0 ? "profiles" : "decks",
  anchor: null,
  logs: [],
  unreadErrors: 0,
  lastPaired: null,
}));

// ---------------------------------------------------------------------------
// Saving

/** Per-key debounced saves, executed in order. `pending` holds values the host hasn't confirmed yet. */
function createSaver<T>(save: (value: T) => Promise<void>) {
  const pending = new Map<string, T>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  let chain: Promise<void> = Promise.resolve();

  const flush = (key: string) => {
    clearTimeout(timers.get(key));
    timers.delete(key);
    const value = pending.get(key);
    if (value === undefined) return;
    chain = chain
      .then(() => save(value))
      .then(
        () => {
          if (pending.get(key) === value) pending.delete(key);
        },
        (error: unknown) => {
          if (pending.get(key) === value) pending.delete(key);
          toastError(error);
          void resync();
        },
      );
  };

  return {
    pending,
    schedule(key: string, value: T, delay: number) {
      pending.set(key, value);
      clearTimeout(timers.get(key));
      if (delay > 0) timers.set(key, setTimeout(() => flush(key), delay));
      else flush(key);
    },
    cancel(key: string) {
      clearTimeout(timers.get(key));
      timers.delete(key);
      pending.delete(key);
    },
    flushAll() {
      for (const key of [...timers.keys()]) flush(key);
    },
  };
}

const savers = {
  settings: createSaver(api.setSettings),
  profiles: createSaver(api.saveProfile),
  themes: createSaver(api.saveTheme),
  voicePresets: createSaver(api.saveVoicePreset),
  devices: createSaver((d: Device) => api.updateDevice(d.id, d.name, d.profileId, d.deck)),
};

// Don't lose a debounced save when the window closes.
window.addEventListener("pagehide", () => Object.values(savers).forEach((s) => s.flushAll()));

/** Overlays unconfirmed local edits on a config pushed by the host. */
function withPending(config: Config): Config {
  const merge = <T extends { id: string }>(items: T[], pending: Map<string, T>, append: boolean) => {
    if (pending.size === 0) return items;
    const merged = items.map((item) => pending.get(item.id) ?? item);
    if (append) for (const [id, item] of pending) if (!items.some((i) => i.id === id)) merged.push(item);
    return merged;
  };
  return {
    ...config,
    settings: savers.settings.pending.get("settings") ?? config.settings,
    profiles: merge(config.profiles, savers.profiles.pending, true),
    themes: merge(config.themes, savers.themes.pending, true),
    voicePresets: merge(config.voicePresets, savers.voicePresets.pending, true),
    devices: merge(config.devices, savers.devices.pending, false),
  };
}

/** Reloads the host state, e.g. after a failed save. */
export async function resync() {
  try {
    const state = await api.getState();
    useApp.setState({ config: withPending(state.config), runtime: state.runtime });
  } catch (error) {
    toastError(error);
  }
}

const setConfig = (recipe: (config: Config) => Config) => useApp.setState((s) => ({ config: recipe(s.config) }));

/** Runs a one-shot command, toasting its error. Resolves false on failure. */
export async function run(command: Promise<unknown>): Promise<boolean> {
  try {
    await command;
    return true;
  } catch (error) {
    toastError(error);
    void resync();
    return false;
  }
}

// ---------------------------------------------------------------------------
// Mutations

/** Replaces the settings. Use `delay` (ms) for text inputs. */
export function saveSettings(settings: Settings, delay = 0) {
  setConfig((c) => ({ ...c, settings }));
  savers.settings.schedule("settings", settings, delay);
}

export function patchSettings(patch: Partial<Settings>, delay = 0) {
  saveSettings({ ...useApp.getState().config.settings, ...patch }, delay);
}

/** Settings fields that are nested objects (usb, ai, obs, homeAssistant, voice). */
export type SettingsSection = { [K in keyof Settings]: Settings[K] extends object ? K : never }[keyof Settings];

export function patchSection<K extends SettingsSection>(key: K, patch: Partial<Settings[K]>, delay = 0) {
  const settings = useApp.getState().config.settings;
  saveSettings({ ...settings, [key]: { ...settings[key], ...patch } }, delay);
}

export function saveProfile(profile: Profile, delay = 400) {
  setConfig((c) => ({ ...c, profiles: upsert(c.profiles, profile) }));
  savers.profiles.schedule(profile.id, profile, delay);
}

export function deleteProfile(id: string) {
  savers.profiles.cancel(id);
  setConfig((c) => ({ ...c, profiles: c.profiles.filter((p) => p.id !== id) }));
  return run(api.deleteProfile(id));
}

export function saveTheme(theme: Theme, delay = 0) {
  setConfig((c) => ({ ...c, themes: upsert(c.themes, theme) }));
  savers.themes.schedule(theme.id, theme, delay);
}

export function deleteTheme(id: string) {
  savers.themes.cancel(id);
  if (useApp.getState().config.settings.themeId === id) patchSettings({ themeId: "midnight" });
  setConfig((c) => ({ ...c, themes: c.themes.filter((t) => t.id !== id) }));
  return run(api.deleteTheme(id));
}

export function saveVoicePreset(preset: VoicePreset, delay = 0) {
  setConfig((c) => ({ ...c, voicePresets: upsert(c.voicePresets, preset) }));
  savers.voicePresets.schedule(preset.id, preset, delay);
}

export function deleteVoicePreset(id: string) {
  savers.voicePresets.cancel(id);
  if (useApp.getState().config.settings.voice.presetId === id) patchSection("voice", { presetId: "clean" });
  setConfig((c) => ({ ...c, voicePresets: c.voicePresets.filter((p) => p.id !== id) }));
  return run(api.deleteVoicePreset(id));
}

export function saveDevice(device: Device, delay = 0) {
  setConfig((c) => ({ ...c, devices: c.devices.map((d) => (d.id === device.id ? device : d)) }));
  savers.devices.schedule(device.id, device, delay);
}

export function unpairDevice(id: string) {
  savers.devices.cancel(id);
  setConfig((c) => ({ ...c, devices: c.devices.filter((d) => d.id !== id) }));
  return run(api.unpairDevice(id));
}

export function navigate(screen: Screen, anchor: string | null = null) {
  useApp.setState((s) => ({ screen, anchor, unreadErrors: screen === "log" ? 0 : s.unreadErrors }));
}

export const clearLogs = () => useApp.setState({ logs: [], unreadErrors: 0 });

// ---------------------------------------------------------------------------
// Host events

handleEvent = (event) => {
  switch (event.type) {
    case "config":
      useApp.setState({ config: withPending(event.config) });
      return;
    case "runtime":
      useApp.setState({ runtime: event.runtime });
      return;
    case "paired":
      useApp.setState({ lastPaired: event.device });
      toast(`Paired ${event.device.name}`, "success");
      return;
    case "log": {
      const { entry } = event;
      useApp.setState((s) => ({
        logs: [entry, ...s.logs].slice(0, 300),
        unreadErrors: entry.level === "error" && s.screen !== "log" ? s.unreadErrors + 1 : s.unreadErrors,
      }));
      return;
    }
    default:
      event satisfies never;
  }
};

// ---------------------------------------------------------------------------
// Selectors

export const useSettings = () => useApp((s) => s.config.settings);
export const useRuntime = () => useApp((s) => s.runtime);
export const useProfiles = () => useApp((s) => s.config.profiles);
export const useIsMac = () => useApp((s) => s.info.os === "macos");

/** Built-in + custom themes. */
export function useThemes(): Theme[] {
  const builtin = useApp((s) => s.builtinThemes);
  const custom = useApp((s) => s.config.themes);
  return useMemo(() => [...builtin, ...custom], [builtin, custom]);
}

/** Built-in + custom voice presets. */
export function useVoicePresets(): VoicePreset[] {
  const builtin = useApp((s) => s.builtinVoicePresets);
  const custom = useApp((s) => s.config.voicePresets);
  return useMemo(() => [...builtin, ...custom], [builtin, custom]);
}

/** Theme of the desktop app. */
export function useAppTheme(): Theme {
  const themes = useThemes();
  const id = useApp((s) => s.config.settings.themeId);
  return useMemo(() => findTheme(themes, id), [themes, id]);
}

/** Resolves an imported image id to a URL served by the deck server. */
export function useImageUrl(): (id: string) => string {
  const port = useApp((s) => s.runtime.server.port || s.config.settings.port);
  return useMemo(() => (id: string) => imageUrl(port, id), [port]);
}
