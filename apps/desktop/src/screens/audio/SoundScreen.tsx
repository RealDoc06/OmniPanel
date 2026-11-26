// Sound screen: the mixer (computer sound, microphone, per-app volume and devices)
// followed by the voice changer section (`sound-voice` anchor).
import type { AppAudio, AppRoute, AudioDirection, MixerState, VolumeState, VolumeTarget } from "@omni/protocol";
import { useRef, useState, type ReactNode } from "react";
import { DeviceSelect } from "../../components/DeviceSelect";
import { I } from "../../components/Icon";
import { Button, IconButton, Notice, ScreenHeader, Section } from "../../components/ui";
import { recordsOnly } from "../../lib/actions";
import type { IconName } from "../../lib/icons";
import { api } from "../../lib/backend";
import { usePolling, useThrottled } from "../../lib/hooks";
import { run, useAnchor, useIsMac } from "../../lib/store";
import { toastError } from "../../lib/toast";
import { cx, errorMessage } from "../../lib/util";
import { VoiceChangerSection } from "./VoiceChanger";

/** macOS privacy pane for "Screen & System Audio Recording", which per-app volume needs. */
const AUDIO_PERMISSION_PANE = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

export function SoundScreen() {
  useAnchor("sound");
  return (
    <>
      <ScreenHeader title="Sound" />
      <div className="flex-1 overflow-y-auto">
        <Section title="Volume" id="sound-volume">
          <Mixer />
        </Section>
        <VoiceChangerSection />
      </div>
    </>
  );
}

/** The device selects of an app row, in column order, and the capability each needs. */
const DIRECTIONS: readonly { direction: AudioDirection; label: string; icon: IconName }[] = [
  { direction: "output", label: "plays on", icon: "speaker" },
  { direction: "input", label: "microphone", icon: "mic" },
];
const CAPABILITY = { output: "perAppOutput", input: "perAppInput" } as const satisfies Record<AudioDirection, keyof MixerState["capabilities"]>;

/** Row grids by number of device columns: icon, name, slider, percentage, then the selects. */
const COLUMNS = [
  "grid-cols-[1.75rem_11rem_minmax(6rem,420px)_2.5rem]",
  "grid-cols-[1.75rem_11rem_minmax(6rem,420px)_2.5rem_minmax(8rem,13rem)]",
  "grid-cols-[1.75rem_11rem_minmax(6rem,420px)_2.5rem_minmax(8rem,13rem)_minmax(8rem,13rem)]",
] as const;

function patchMixer(mixer: MixerState, target: VolumeTarget, patch: Partial<VolumeState>): MixerState {
  switch (target.type) {
    case "master":
      return mixer.master ? { ...mixer, master: { ...mixer.master, ...patch } } : mixer;
    case "mic":
      return mixer.mic ? { ...mixer, mic: { ...mixer.mic, ...patch } } : mixer;
    case "app":
      return { ...mixer, apps: mixer.apps.map((a) => (a.id === target.app ? { ...a, ...patch } : a)) };
    case "obsInput":
      return mixer;
  }
}

/** Polls the mixer every 1.5 s while visible. Local changes win over polls for a moment. */
function Mixer() {
  const [mixer, setMixer] = useState<MixerState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const touchedAt = useRef(0);
  const mac = useIsMac();
  const capabilities = mixer?.capabilities;
  const columns = COLUMNS[Number(capabilities?.perAppOutput ?? false) + Number(capabilities?.perAppInput ?? false)] ?? COLUMNS[0];
  const note = capabilities?.note ?? (capabilities && !capabilities.perAppVolume ? "App volume controls are unavailable." : null);
  // Match omni-audio's macOS PERMISSION_HINT, not unsupported-OS or tap failures.
  const needsPermission = mac && capabilities?.perAppVolume === false &&
    capabilities.note?.startsWith("Per-app volume and speakers: allow OmniPanel under System Settings > Privacy & Security > Screen & System Audio Recording");
  // Why a speaker or microphone cannot be chosen here (once, and not what the notice
  // above already says), or when a microphone select shows up.
  const deviceNotes = capabilities
    ? [...new Set([capabilities.outputNote, capabilities.inputNote])].filter((n) => n !== null && n !== capabilities.note)
    : [];
  if (capabilities?.perAppInput) deviceNotes.push("An app's microphone can be chosen while it is recording.");

  usePolling(async () => {
    try {
      const next = await api.getMixer();
      if (Date.now() - touchedAt.current > 1500) setMixer(next);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, 1500);

  const sendVolume = useThrottled((target: VolumeTarget, value: number) => {
    api.setVolume(target, value).catch(toastError);
  }, 50);

  const setVolume = (target: VolumeTarget, volume: number) => {
    touchedAt.current = Date.now();
    setMixer((m) => m && patchMixer(m, target, { volume }));
    sendVolume(target, volume);
  };

  const setMuted = (target: VolumeTarget, muted: boolean) => {
    touchedAt.current = Date.now();
    setMixer((m) => m && patchMixer(m, target, { muted }));
    api.setMuted(target, muted).catch(toastError);
  };

  /** Sends an app to a device. A failure is toasted and the next poll shows where the app really is. */
  const setDevice = (app: AppAudio, direction: AudioDirection, device: string | null) => {
    const route: AppRoute = device === null ? { type: "default" } : { type: "device", id: device };
    touchedAt.current = Date.now();
    setMixer((m) => m && { ...m, apps: m.apps.map((a) => (a.id === app.id ? { ...a, [direction]: route } : a)) });
    api.runAction({ type: "appDevice", app: app.id, direction, device }).catch(toastError);
  };

  // One select per direction this computer can change, shown while the app has such a stream.
  const deviceSelects = (app: AppAudio) =>
    mixer &&
    DIRECTIONS.filter(({ direction }) => mixer.capabilities[CAPABILITY[direction]]).map(({ direction, label, icon }) => {
      const route = app[direction];
      return route === null ? (
        <span key={direction} />
      ) : (
        <span key={direction} className="flex min-w-0 items-center gap-1.5" title={`${app.name} ${label}`}>
          <I name={icon} className="size-3.5 shrink-0 text-muted" />
          <DeviceSelect
            ariaLabel={`${app.name} ${label}`}
            className="min-w-0"
            devices={direction === "output" ? mixer.devices.outputs : mixer.devices.inputs}
            value={route}
            onChange={(device) => setDevice(app, direction, device)}
          />
        </span>
      );
    });

  const row = (target: VolumeTarget, name: string, icon: IconName, state: VolumeState, app?: AppAudio) => (
    <MixerRow
      key={target.type === "app" ? target.app : target.type}
      name={name}
      icon={icon}
      state={state}
      playing={app?.playing}
      disabled={target.type === "app" && !mixer?.capabilities.perAppVolume}
      silent={app !== undefined && recordsOnly(app)}
      columns={columns}
      onVolume={(v) => setVolume(target, v)}
      onMute={(m) => setMuted(target, m)}
    >
      {app && deviceSelects(app)}
    </MixerRow>
  );

  return (
    <>
      {error && <Notice tone="error">{error}</Notice>}
      {mixer && (
        <div className="flex flex-col">
          {mixer.master && row({ type: "master" }, "Computer sound", "volume-2", mixer.master)}
          {mixer.mic && row({ type: "mic" }, "Microphone", "mic", mixer.mic)}
          <div className="my-2 border-t border-line" />
          {note && (
            <div className="mb-2 flex flex-wrap items-center gap-x-3 gap-y-1">
              <Notice tone="warn">{note}</Notice>
              {needsPermission && (
                <Button size="sm" onClick={() => void run(api.runAction({ type: "open", target: AUDIO_PERMISSION_PANE }))}>
                  Open System Settings
                </Button>
              )}
            </div>
          )}
          {mixer.apps.length === 0 ? (
            <p className="py-2 text-muted">Apps show up here while they play sound.</p>
          ) : (
            mixer.apps.map((app) => row({ type: "app", app: app.id }, app.name, "app-window", app, app))
          )}
          {deviceNotes.map((text) => (
            <p key={text} className="mt-2 text-xs text-muted">
              {text}
            </p>
          ))}
        </div>
      )}
    </>
  );
}

/**
 * One line: mute toggle (doubles as the row's icon), name, slider, percentage, then
 * the app's device selects (`children`). Every row uses the same `columns`, so the
 * sliders line up whether or not a row has selects.
 */
function MixerRow({
  name,
  icon,
  state,
  playing,
  disabled = false,
  silent = false,
  columns,
  onVolume,
  onMute,
  children,
}: {
  name: string;
  icon: IconName;
  state: VolumeState;
  playing?: boolean;
  disabled?: boolean;
  /** The app only records: it has no volume to change. */
  silent?: boolean;
  columns: string;
  onVolume: (value: number) => void;
  onMute: (muted: boolean) => void;
  children?: ReactNode;
}) {
  const muteIcon: IconName = icon === "mic" ? (state.muted ? "mic-off" : "mic") : state.muted ? "volume-x" : icon;
  const fixed = disabled || silent;
  const faded = fixed && "opacity-40";
  return (
    <div
      className={cx("grid h-8 items-center gap-x-3", columns)}
      title={disabled ? "App sliders and mute controls are unavailable." : silent ? `${name} is not playing anything.` : undefined}
    >
      <IconButton
        icon={muteIcon}
        label={state.muted ? `Unmute ${name}` : `Mute ${name}`}
        tone={state.muted ? "danger" : "default"}
        disabled={fixed}
        onClick={() => onMute(!state.muted)}
        className={cx(!state.muted && "text-muted", faded)}
      />
      <span className={cx("flex min-w-0 items-baseline gap-2", disabled && "opacity-40")}>
        <span className="truncate">{name}</span>
        {playing && (
          <span role="img" aria-label="Playing" title="Playing" className="shrink-0 text-muted">
            <I name="activity" className="inline size-3 align-[-2px]" />
          </span>
        )}
      </span>
      <input
        type="range"
        disabled={fixed}
        aria-label={`${name} volume`}
        min={0}
        max={1}
        step={0.01}
        value={state.volume}
        onChange={(e) => onVolume(Number(e.target.value))}
        className={cx("w-full min-w-0", (state.muted || fixed) && "opacity-40")}
      />
      <span className={cx("text-right text-xs text-muted tabular-nums", faded)}>{Math.round(state.volume * 100)}%</span>
      {children}
    </div>
  );
}
