// App store: the host `AppState`, kept in sync by host events, plus a little UI state.
// Mutations are optimistic: the store updates immediately and saves go to the host
// (debounced for typing, lib/saver.ts). Until a save is confirmed, incoming `config`
// events keep the local value so the host echo can't overwrite what the user is typing.
// Settings are kept per field, so other settings changed meanwhile still come through.
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
import { createSaver, drainSavers, emptyPatch, quitAfterFlush, rebase, settlePatch, type SettingsPatch, type SettingsSection } from "./saver";
import { findTheme } from "./theme";
import { toast, toastError } from "./toast";
import { upsert } from "./util";

export type { SettingsSection };

export type Screen = "decks" | "profiles" | "audio" | "voice" | "settings" | "log";

export interface AppStore extends AppState {
  screen: Screen;
  /** Section to scroll to after navigating (e.g. "ai" in Settings). */
  anchor: string | null;
  logs: LogEntry[];
  /** Warnings and errors logged since the Activity log was last open. */
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
  logs: initial.logs,
  unreadErrors: initial.logs.filter((entry) => entry.level !== "info").length,
  lastPaired: null,
}));

// ---------------------------------------------------------------------------
// Saving

const onSaveError = (error: unknown) => {
  toastError(error);
  void resync();
};

const savers = {
  // The host applies each field-level patch under its config lock.
  settings: createSaver<SettingsPatch>(api.patchSettings, onSaveError, settlePatch),
  profiles: createSaver(api.saveProfile, onSaveError),
  themes: createSaver(api.saveTheme, onSaveError),
  voicePresets: createSaver(api.saveVoicePreset, onSaveError),
  devices: createSaver((d: Device, options) => api.updateDevice(d.id, d.name, d.profileId, d.deck, options), onSaveError),
};

// Don't lose a debounced save when the window reloads, closes or hides to the tray.
export async function flushAll() {
  await drainSavers(Object.values(savers));
}
export const flushSettings = () => savers.settings.flushAll();
const flushOnUnload = () => Object.values(savers).forEach((s) => s.flushOnUnload());
window.addEventListener("beforeunload", flushOnUnload);
window.addEventListener("pagehide", flushOnUnload);
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden") void flushAll().catch(() => {});
});

// Shell menus and the UI share a bounded flush, then always quit.
export const quit = () => quitAfterFlush(flushAll, api.quit);
window.addEventListener("omni-quit", () => {
  void run(quit());
});

/** Sends any pending save for a profile now. Resolves once the host has stored it, rejects if that failed. */
export const flushProfile = (id: string): Promise<void> => savers.profiles.flush(id);

/** Overlays unconfirmed local edits on a config pushed by the host. */
function withPending(config: Config): Config {
  const merge = <T extends { id: string }>(items: T[], pending: Map<string, T>, append: boolean) => {
    if (pending.size === 0) return items;
    const merged = items.map((item) => pending.get(item.id) ?? item);
    if (append) for (const [id, item] of pending) if (!items.some((i) => i.id === id)) merged.push(item);
    return merged;
  };
  const settings = savers.settings.pending.get("settings");
  return {
    ...config,
    settings: settings ? rebase(config.settings, settings) : config.settings,
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

/** Records a settings edit, shows it right away and saves it. Use `delay` (ms) for text inputs. */
function editSettings(edit: (patch: SettingsPatch) => SettingsPatch, delay: number) {
  const patch = edit(savers.settings.pending.get("settings") ?? emptyPatch());
  setConfig((c) => ({ ...c, settings: rebase(c.settings, patch) }));
  savers.settings.schedule("settings", patch, delay);
}

/** Changes top-level settings fields. Nested sections go through `patchSection`. */
export function patchSettings(patch: Partial<Omit<Settings, SettingsSection>>, delay = 0) {
  editSettings((p) => ({ ...p, fields: { ...p.fields, ...patch } }), delay);
}

/** Changes fields of one nested settings section (usb, ai, obs, homeAssistant, voice). */
export function patchSection<K extends SettingsSection>(key: K, patch: Partial<Settings[K]>, delay = 0) {
  editSettings((p) => ({ ...p, sections: { ...p.sections, [key]: { ...p.sections[key], ...patch } } }), delay);
}

export function saveProfile(profile: Profile, delay = 400) {
  setConfig((c) => ({ ...c, profiles: upsert(c.profiles, profile) }));
  savers.profiles.schedule(profile.id, profile, delay);
}

// Deletes cancel queued saves for the item and run on the same chain, after any save in flight,
// so a save can't recreate what was just deleted.

export function deleteProfile(id: string) {
  savers.profiles.cancel(id);
  setConfig((c) => ({ ...c, profiles: c.profiles.filter((p) => p.id !== id) }));
  return run(savers.profiles.enqueue(() => api.deleteProfile(id)));
}

export function saveTheme(theme: Theme, delay = 0) {
  setConfig((c) => ({ ...c, themes: upsert(c.themes, theme) }));
  savers.themes.schedule(theme.id, theme, delay);
}

export function deleteTheme(id: string) {
  savers.themes.cancel(id);
  if (useApp.getState().config.settings.themeId === id) patchSettings({ themeId: "midnight" });
  setConfig((c) => ({ ...c, themes: c.themes.filter((t) => t.id !== id) }));
  return run(savers.themes.enqueue(() => api.deleteTheme(id)));
}

export function saveVoicePreset(preset: VoicePreset, delay = 0) {
  setConfig((c) => ({ ...c, voicePresets: upsert(c.voicePresets, preset) }));
  savers.voicePresets.schedule(preset.id, preset, delay);
}

export function deleteVoicePreset(id: string) {
  savers.voicePresets.cancel(id);
  if (useApp.getState().config.settings.voice.presetId === id) patchSection("voice", { presetId: "clean" });
  setConfig((c) => ({ ...c, voicePresets: c.voicePresets.filter((p) => p.id !== id) }));
  return run(savers.voicePresets.enqueue(() => api.deleteVoicePreset(id)));
}

export function saveDevice(device: Device, delay = 0) {
  setConfig((c) => ({ ...c, devices: c.devices.map((d) => (d.id === device.id ? device : d)) }));
  savers.devices.schedule(device.id, device, delay);
}

export function unpairDevice(id: string) {
  savers.devices.cancel(id);
  setConfig((c) => ({ ...c, devices: c.devices.filter((d) => d.id !== id) }));
  return run(savers.devices.enqueue(() => api.unpairDevice(id)));
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
      toast(`${event.device.name} connected`, "success");
      return;
    case "log": {
      const { entry } = event;
      useApp.setState((s) => ({
        logs: [entry, ...s.logs].slice(0, 300),
        unreadErrors: entry.level !== "info" && s.screen !== "log" ? s.unreadErrors + 1 : s.unreadErrors,
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
