// Typed client for the engine. Every call is a `HostRequest` (generated from
// crates/omni-core/src/api.rs, so names and arguments are checked against
// Rust); this file pins each request's result type. `send` is the transport:
// Tauri's `host` command in the app, HTTP for the dev bridge (lib/bridge.ts).
import type { SaveOptions, SettingsPatch } from "./saver";

import type {
  Action,
  ActionOutcome,
  AppState,
  AudioDevices,
  DeckSettings,
  GenerateRequest,
  GeneratedControl,
  HaEntity,
  HostRequest,
  InstalledApp,
  MixerState,
  ObsInventory,
  PairingInfo,
  Profile,
  Settings,
  Theme,
  VoiceLevels,
  VoicePreset,
  VolumeTarget,
} from "@omni/protocol";

export type Send = (request: HostRequest, options?: SaveOptions) => Promise<unknown>;

export const makeApi = (send: Send, quit: () => Promise<void>) => {
  const client = crypto.randomUUID();
  const call = <T>(request: HostRequest, options?: SaveOptions) =>
    send(options ? { cmd: "commit", args: { client, revision: options.revision, request } } : request, options) as Promise<T>;
  return {
    // State & config. Every mutation persists and pushes a `config` host event.
    getState: () => call<AppState>({ cmd: "getState" }),
    setSettings: (settings: Settings) => call<void>({ cmd: "setSettings", args: { settings } }),
    patchSettings: (patch: SettingsPatch, options?: SaveOptions) =>
      call<void>({ cmd: "patchSettings", args: { patch: { ...patch.fields, ...patch.sections } } }, options),
    saveProfile: (profile: Profile, options?: SaveOptions) => call<void>({ cmd: "saveProfile", args: { profile } }, options),
    deleteProfile: (id: string) => call<void>({ cmd: "deleteProfile", args: { id } }),
    exportProfile: (id: string, path: string) => call<void>({ cmd: "exportProfile", args: { id, path } }),
    importProfile: (path: string) => call<Profile>({ cmd: "importProfile", args: { path } }),
    updateDevice: (id: string, name: string, profileId: string | null, deck: DeckSettings, options?: SaveOptions) =>
      call<void>({ cmd: "updateDevice", args: { id, name, profileId, deck } }, options),
    unpairDevice: (id: string) => call<void>({ cmd: "unpairDevice", args: { id } }),
    saveTheme: (theme: Theme, options?: SaveOptions) => call<void>({ cmd: "saveTheme", args: { theme } }, options),
    deleteTheme: (id: string) => call<void>({ cmd: "deleteTheme", args: { id } }),
    saveVoicePreset: (preset: VoicePreset, options?: SaveOptions) => call<void>({ cmd: "saveVoicePreset", args: { preset } }, options),
    deleteVoicePreset: (id: string) => call<void>({ cmd: "deleteVoicePreset", args: { id } }),

    // Pairing
    startPairing: () => call<PairingInfo>({ cmd: "startPairing" }),
    stopPairing: () => call<void>({ cmd: "stopPairing" }),
    qrSvg: (text: string) => call<string>({ cmd: "qrSvg", args: { text } }),

    // Actions
    runAction: (action: Action) => call<ActionOutcome>({ cmd: "runAction", args: { action } }),
    installedApps: () => call<InstalledApp[]>({ cmd: "installedApps" }),
    importImage: (path: string) => call<string>({ cmd: "importImage", args: { path } }),

    // Audio mixer
    getMixer: () => call<MixerState>({ cmd: "getMixer" }),
    setVolume: (target: VolumeTarget, value: number) => call<void>({ cmd: "setVolume", args: { target, value } }),
    setMuted: (target: VolumeTarget, muted: boolean) => call<void>({ cmd: "setMuted", args: { target, muted } }),

    // Voice changer (enable / preset / devices are part of `settings.voice`)
    voiceDevices: () => call<AudioDevices>({ cmd: "voiceDevices" }),
    voiceLevels: () => call<VoiceLevels>({ cmd: "voiceLevels" }),
    createVirtualMic: () => call<string>({ cmd: "createVirtualMic" }),

    // Integrations
    obsInventory: () => call<ObsInventory>({ cmd: "obsInventory" }),
    haEntities: () => call<HaEntity[]>({ cmd: "haEntities" }),

    // AI assistant (uses `settings.ai`)
    aiGenerate: (request: GenerateRequest) => call<GeneratedControl>({ cmd: "aiGenerate", args: { request } }),
    aiModels: () => call<string[]>({ cmd: "aiModels" }),
    aiTest: () => call<string>({ cmd: "aiTest" }),

    // App
    openDataDir: () => call<void>({ cmd: "openDataDir" }),
    quit,
  };
};

export type Api = ReturnType<typeof makeApi>;
