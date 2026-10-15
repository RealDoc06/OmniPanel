// Volume screen: computer sound, microphone and per-app mixer.
import type { AppAudio, MixerState, VolumeState, VolumeTarget } from "@omni/protocol";
import { useRef, useState } from "react";
import { I } from "../../components/Icon";
import { Button, IconButton, Notice, ScreenHeader } from "../../components/ui";
import type { IconName } from "../../lib/icons";
import { api } from "../../lib/backend";
import { usePolling, useThrottled } from "../../lib/hooks";
import { run, useIsMac } from "../../lib/store";
import { toastError } from "../../lib/toast";
import { cx, errorMessage } from "../../lib/util";

/** macOS privacy pane for "Screen & System Audio Recording", which per-app volume needs. */
const AUDIO_PERMISSION_PANE = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

export function AudioScreen() {
  return (
    <>
      <ScreenHeader title="Volume" />
      <div className="flex-1 overflow-y-auto px-6 py-4">
        <Mixer />
      </div>
    </>
  );
}

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

  const row = (target: VolumeTarget, name: string, icon: IconName, state: VolumeState, app?: AppAudio) => (
    <MixerRow
      key={target.type === "app" ? target.app : target.type}
      name={name}
      icon={icon}
      state={state}
      playing={app?.playing}
      disabled={target.type === "app" && !mixer?.capabilities.perAppVolume}
      onVolume={(v) => setVolume(target, v)}
      onMute={(m) => setMuted(target, m)}
    />
  );

  return (
    <>
      {error && <Notice tone="error">{error}</Notice>}
      {mixer && (
        <div className="flex flex-col">
          {mixer.master && row({ type: "master" }, "Computer sound", "volume-2", mixer.master)}
          {mixer.mic && row({ type: "mic" }, "Microphone", "mic", mixer.mic)}
          <div className="my-2 border-t border-line" />
          {!mixer.capabilities.perAppVolume && (
            <div className="mb-2 flex flex-wrap items-center gap-x-3 gap-y-1">
              <Notice tone="warn">
                {mac ? "Allow OmniPanel to control app volume." : (mixer.capabilities.note ?? "App volume controls are unavailable.")}
              </Notice>
              {mac && (
                <Button size="sm" onClick={() => void run(api.runAction({ type: "open", target: AUDIO_PERMISSION_PANE }))}>
                  Open System Settings
                </Button>
              )}
            </div>
          )}
          {mixer.apps.length === 0 ? (
            <p className="py-2">Apps show up here while they play sound.</p>
          ) : (
            mixer.apps.map((app) => row({ type: "app", app: app.id }, app.name, "app-window", app, app))
          )}
        </div>
      )}
    </>
  );
}

function MixerRow({
  name,
  icon,
  state,
  playing,
  disabled = false,
  onVolume,
  onMute,
}: {
  name: string;
  icon: IconName;
  state: VolumeState;
  playing?: boolean;
  disabled?: boolean;
  onVolume: (value: number) => void;
  onMute: (muted: boolean) => void;
}) {
  return (
    <div
      className={cx("flex h-10 items-center gap-3", disabled && "opacity-40")}
      title={disabled ? "App sliders and mute controls are unavailable until app volume control is allowed." : undefined}
    >
      <I name={icon} className="size-4 text-muted" />
      <span className="flex w-44 min-w-0 items-baseline gap-2">
        <span className="truncate">{name}</span>
        {playing && <span className="shrink-0 text-xs text-muted">playing</span>}
      </span>
      <IconButton
        icon={icon === "mic" ? (state.muted ? "mic-off" : "mic") : (state.muted ? "volume-x" : "volume-2")}
        label={state.muted ? `Unmute ${name}` : `Mute ${name}`}
        tone={state.muted ? "danger" : "default"}
        disabled={disabled}
        onClick={() => onMute(!state.muted)}
      />
      <input
        type="range"
        disabled={disabled}
        aria-label={`${name} volume`}
        min={0}
        max={1}
        step={0.01}
        value={state.volume}
        onChange={(e) => onVolume(Number(e.target.value))}
        className={cx("h-4 max-w-[420px] min-w-0 flex-1", state.muted && "opacity-40")}
      />
      <span className="w-10 text-right text-xs text-muted tabular-nums">{Math.round(state.volume * 100)}%</span>
    </div>
  );
}
