// Mixer (master, mic, per-app) and the voice changer.
import type { AppAudio, MixerState, VolumeState, VolumeTarget } from "@omni/protocol";
import { useRef, useState } from "react";
import { I } from "../../components/Icon";
import { IconButton, Notice, ScreenHeader, Section } from "../../components/ui";
import type { IconName } from "../../lib/icons";
import { api } from "../../lib/backend";
import { usePolling, useThrottled } from "../../lib/hooks";
import { toastError } from "../../lib/toast";
import { cx, errorMessage } from "../../lib/util";
import { VoiceChanger } from "./VoiceChanger";

export function AudioScreen() {
  return (
    <>
      <ScreenHeader title="Audio" />
      <div className="flex-1 overflow-y-auto">
        <Mixer />
        <VoiceChanger />
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
      onVolume={(v) => setVolume(target, v)}
      onMute={(m) => setMuted(target, m)}
    />
  );

  return (
    <Section title="Mixer">
      {error && <Notice tone="error">{error}</Notice>}
      {mixer && (
        <div className="flex flex-col">
          {mixer.master && row({ type: "master" }, "System output", "volume-2", mixer.master)}
          {mixer.mic && row({ type: "mic" }, "Microphone", "mic", mixer.mic)}
          <div className="mt-3 mb-1 text-xs font-semibold text-muted">Apps</div>
          {mixer.capabilities.note && (
            <div className="mb-2">
              <Notice tone={mixer.capabilities.perAppVolume ? "info" : "warn"}>{mixer.capabilities.note}</Notice>
            </div>
          )}
          {mixer.apps.length === 0 ? (
            <p className="py-2 text-muted">No app is playing audio.</p>
          ) : (
            mixer.apps.map((app) => row({ type: "app", app: app.id }, app.name, "app-window", app, app))
          )}
        </div>
      )}
    </Section>
  );
}

function MixerRow({
  name,
  icon,
  state,
  playing,
  onVolume,
  onMute,
}: {
  name: string;
  icon: IconName;
  state: VolumeState;
  playing?: boolean;
  onVolume: (value: number) => void;
  onMute: (muted: boolean) => void;
}) {
  return (
    <div className="flex h-10 items-center gap-3">
      <I name={icon} className="size-4 text-muted" />
      <span className="flex w-44 min-w-0 items-center gap-1.5 truncate">
        {name}
        {playing && <span title="Playing" className="size-1.5 shrink-0 rounded-full bg-success" />}
      </span>
      <IconButton
        icon={state.muted ? "volume-x" : "volume-2"}
        label={state.muted ? `Unmute ${name}` : `Mute ${name}`}
        tone={state.muted ? "danger" : "default"}
        onClick={() => onMute(!state.muted)}
      />
      <input
        type="range"
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
