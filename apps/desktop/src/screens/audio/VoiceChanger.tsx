// Voice changer section of the Sound screen: status and switch in the title row, one-time
// setup per OS, the voice list, microphone and monitor rows, level meters, Advanced gains
// and the voice editor.
import type { AudioDeviceInfo, Effect, VoiceLevels, VoiceSettings, VoiceStatus } from "@omni/protocol";
import { useState, type ReactNode } from "react";
import { I } from "../../components/Icon";
import { Button, Disclosure, Field, IconButton, Notice, Range, Section, Select, Switch } from "../../components/ui";
import { EFFECT_LABELS } from "../../lib/actions";
import { api } from "../../lib/backend";
import { usePolling, useResource } from "../../lib/hooks";
import { patchSection, run, saveVoicePreset, useApp, useRuntime, useSettings, useVoicePresets } from "../../lib/store";
import { toast, toastError } from "../../lib/toast";
import { cx, newId, uniqueName } from "../../lib/util";
import { PresetEditor } from "./PresetEditor";

const db = (v: number) => `${v > 0 ? "+" : ""}${v.toFixed(0)} dB`;

function deviceOptions(devices: readonly AudioDeviceInfo[]) {
  return devices.map((d) => ({ value: d.id, label: `${d.name}${d.isVirtual ? " (virtual)" : ""}${d.isDefault ? " (default)" : ""}` }));
}

interface SetupCopy {
  install: string;
  installed: string;
  /** Matches the prerequisite's output name from the host device inventory. */
  outputName: string;
  /** Microphone name other apps pick. */
  mic: string;
  /** Download page; without one the host creates the virtual mic itself (Linux). */
  download?: { label: string; url: string };
}

const SETUP: Partial<Record<string, SetupCopy>> = {
  macos: {
    install: "Install BlackHole, a free virtual microphone.",
    installed: "BlackHole is installed.",
    outputName: "BlackHole",
    mic: "BlackHole 2ch",
    download: { label: "Get BlackHole", url: "https://existential.audio/blackhole/" },
  },
  windows: {
    install: "Install VB-CABLE, a free virtual microphone.",
    installed: "VB-CABLE is installed.",
    outputName: "CABLE Input",
    mic: "CABLE Output",
    download: { label: "Get VB-CABLE", url: "https://vb-audio.com/Cable/" },
  },
};
const LINUX_SETUP: SetupCopy = {
  install: "Create a virtual microphone.",
  installed: "The virtual microphone is ready.",
  outputName: "OmniPanel_Virtual_Sink",
  mic: "OmniPanel_Virtual_Mic",
};

/** Mirrors omni-voice `mic_name_for_output`: the microphone apps pick when we play into `output`. */
const micFor = (output: string) =>
  output.includes("OmniPanel_Virtual_Sink") ? "OmniPanel_Virtual_Mic" : output.replace("Input", "Output");

const STATE_TONES = { Off: "text-muted", On: "text-success", "Needs setup": "text-warning", Stopped: "text-danger" };

/** Setup takes priority until an output is chosen; running sessions can have monitor warnings. */
function voiceState(voice: VoiceSettings, status: VoiceStatus): keyof typeof STATE_TONES {
  if (!voice.enabled) return "Off";
  if (voice.outputDevice === null) return "Needs setup";
  return status.running ? "On" : "Stopped";
}

const openLink = (url: string) => void run(api.runAction({ type: "open", target: url }));

export function VoiceChangerSection() {
  const { voice } = useSettings();
  const status = useRuntime().voice;
  const presets = useVoicePresets();
  const custom = useApp((s) => s.config.voicePresets);
  const devices = useResource(() => api.voiceDevices());
  // Bumped to remount the voice editor open (after New voice or Make a copy).
  const [editorKey, setEditorKey] = useState(0);
  const set = (patch: Partial<VoiceSettings>, delay = 0) => patchSection("voice", patch, delay);
  const selected = presets.find((p) => p.id === voice.presetId);
  const state = voiceState(voice, status);

  /** Creates a custom voice, selects it and opens its editor. */
  const addVoice = (name: string, effects: Effect[]) => {
    const preset = { id: newId(), name: uniqueName(name, presets.map((p) => p.name)), effects };
    saveVoicePreset(preset);
    set({ presetId: preset.id });
    setEditorKey((k) => k + 1);
  };

  return (
    <Section
      title="Voice changer"
      id="sound-voice"
      actions={
        <>
          <span className={cx("text-xs", STATE_TONES[state])}>{state}</span>
          <Switch label="Voice changer" checked={voice.enabled} onChange={(enabled) => set({ enabled })} />
        </>
      }
    >
      <p className="mb-2 text-muted">Changes how your microphone sounds in calls and recordings.</p>
      {state === "Stopped" && (
        <div className="py-1">
          <Notice tone="error">The voice changer stopped{status.error ? `: ${status.error}` : "."}</Notice>
        </div>
      )}
      {state === "On" && status.error && (
        <div className="py-1">
          <Notice tone="warn">Hear yourself is unavailable: {status.error.replace(/^monitor unavailable:\s*/, "")}</Notice>
        </div>
      )}

      <Setup voice={voice} status={status} outputs={devices.data?.outputs} reload={devices.reload} />

      <Field label="Voice" hint={selected && (selected.effects.map((e) => EFFECT_LABELS[e.type]).join(", ") || "No effects: your voice is unchanged.")}>
        <div className="flex items-center gap-2">
          <Select
            ariaLabel="Voice"
            className="max-w-xs"
            value={voice.presetId}
            options={presets.map((p) => ({ value: p.id, label: p.name }))}
            onChange={(presetId) => set({ presetId })}
          />
          <Button size="sm" variant="ghost" icon="plus" className="text-muted hover:text-fg" onClick={() => addVoice("My voice", [])}>
            New voice
          </Button>
        </div>
      </Field>
      <Field label="Your microphone">
        <Select
          ariaLabel="Your microphone"
          className="max-w-xs"
          value={voice.inputDevice ?? ""}
          options={[{ value: "", label: "System default" }, ...deviceOptions(devices.data?.inputs ?? [])]}
          onChange={(id) => set({ inputDevice: id || null })}
        />
      </Field>
      <Field label="Hear yourself">
        <div className="flex items-center gap-3">
          <Switch label="Hear yourself" checked={voice.monitor} onChange={(monitor) => set({ monitor })} />
          <Select
            ariaLabel="Hear yourself on"
            className="max-w-xs"
            value={voice.monitorDevice ?? ""}
            options={[{ value: "", label: "System default" }, ...deviceOptions(devices.data?.outputs ?? [])]}
            onChange={(id) => set({ monitorDevice: id || null })}
          />
        </div>
      </Field>
      {voice.enabled && status.running && <Meters />}
      {devices.error && <Notice tone="error">{devices.error}</Notice>}

      <div className="mt-2">
        <Disclosure label="Advanced">
          <Field label="Mic boost">
            <Range label="Mic boost" className="max-w-xs" min={-24} max={24} step={1} value={voice.inputGainDb} format={db} onChange={(inputGainDb) => set({ inputGainDb }, 150)} />
          </Field>
          <Field label="Changed voice volume">
            <Range
              label="Changed voice volume"
              className="max-w-xs"
              min={-24}
              max={24}
              step={1}
              value={voice.outputGainDb}
              format={db}
              onChange={(outputGainDb) => set({ outputGainDb }, 150)}
            />
          </Field>
          <Field label="Cut background noise">
            <Switch label="Cut background noise" checked={voice.noiseGate} onChange={(noiseGate) => set({ noiseGate })} />
          </Field>
          <Field label="Cut sounds below">
            <Range
              label="Cut sounds below"
              className="max-w-xs"
              min={-90}
              max={-20}
              step={1}
              disabled={!voice.noiseGate}
              value={voice.gateThresholdDb}
              format={db}
              onChange={(gateThresholdDb) => set({ gateThresholdDb }, 150)}
            />
          </Field>
        </Disclosure>
        {selected && (
          <Disclosure key={editorKey} label={`Customize ${selected.name}`} defaultOpen={editorKey > 0}>
            <PresetEditor
              key={selected.id}
              preset={selected}
              custom={custom.some((p) => p.id === selected.id)}
              onDuplicate={() => addVoice(`${selected.name} copy`, structuredClone(selected.effects))}
            />
          </Disclosure>
        )}
      </div>
    </Section>
  );
}

/**
 * Numbered setup steps while no output is chosen (or after Change), otherwise one
 * line saying where the changed voice goes.
 */
function Setup({
  voice,
  status,
  outputs,
  reload,
}: {
  voice: VoiceSettings;
  status: VoiceStatus;
  outputs: readonly AudioDeviceInfo[] | undefined;
  reload: () => void;
}) {
  const os = useApp((s) => s.info.os);
  const [changing, setChanging] = useState(false);
  const { install, installed, outputName, mic, download } = SETUP[os] ?? LINUX_SETUP;
  const virtual = outputs?.filter((d) => d.isVirtual) ?? [];
  const prerequisite = virtual.find((d) => d.name.toLowerCase().includes(outputName.toLowerCase()));
  const selectedOutput = outputs?.find((d) => d.id === voice.outputDevice);
  const suggested = voice.outputDevice === null && virtual.length === 1 ? virtual[0] : undefined;
  const setOutput = (outputDevice: string) => {
    patchSection("voice", { outputDevice });
    setChanging(false);
  };

  const createMic = async () => {
    try {
      const id = await api.createVirtualMic();
      reload();
      setOutput(id);
      toast("Virtual mic created. In Discord, Zoom or OBS, choose it as your microphone.", "success");
    } catch (error) {
      toastError(error);
    }
  };

  if (voice.outputDevice !== null && !changing) {
    const output = selectedOutput?.name ?? voice.outputDevice;
    return (
      <Field label="Output">
        <div className="flex items-center gap-3">
          <p className="min-w-0 flex-1">
            <span className="font-medium">{output}</span>
            <span className="text-muted">
              {" "}
              · pick <span className="text-fg">{status.virtualMic ?? micFor(output)}</span> as the microphone in Discord, Zoom or OBS
            </span>
          </p>
          <Button size="sm" variant="ghost" className="text-muted hover:text-fg" onClick={() => setChanging(true)}>
            Change
          </Button>
        </div>
      </Field>
    );
  }

  return (
    <Field label="Setup">
      <ol className="flex flex-col py-0.5">
        <SetupStep n={1} done={!!prerequisite}>
          <span>{!outputs ? "Checking audio devices…" : prerequisite ? installed : install}</span>
          {outputs && !prerequisite && (download ? (
            <Button size="sm" variant={suggested ? "default" : "primary"} icon="external-link" onClick={() => openLink(download.url)}>
              {download.label}
            </Button>
          ) : (
            <Button size="sm" variant={suggested ? "default" : "primary"} icon="plus" onClick={() => void createMic()}>
              Create virtual mic
            </Button>
          ))}
        </SetupStep>
        <SetupStep n={2} done={!!selectedOutput}>
          <span className="shrink-0">Send your changed voice to</span>
          <Select
            ariaLabel="Send your changed voice to"
            className="w-56 min-w-0"
            value={voice.outputDevice ?? ""}
            placeholder="Choose…"
            options={deviceOptions(outputs ?? [])}
            onChange={setOutput}
          />
          {suggested && (
            <Button size="sm" variant="primary" onClick={() => setOutput(suggested.id)}>
              Use {suggested.name}
            </Button>
          )}
          <IconButton icon="refresh-cw" label="Refresh devices" onClick={reload} />
          {changing && (
            <Button size="sm" variant="ghost" onClick={() => setChanging(false)}>
              Done
            </Button>
          )}
        </SetupStep>
        <SetupStep n={3} done={false}>
          <span>In Discord, Zoom or OBS, choose "{status.virtualMic ?? (selectedOutput ? micFor(selectedOutput.name) : mic)}" as your microphone.</span>
        </SetupStep>
      </ol>
    </Field>
  );
}

/** Numbered (or checked) setup step; its content wraps while the number stays put. */
function SetupStep({ n, done, children }: { n: number; done: boolean; children: ReactNode }) {
  return (
    <li className="flex gap-3">
      <span className="flex h-8 w-4 shrink-0 items-center text-muted tabular-nums">{done ? <I name="check" className="size-4 text-success" /> : n}</span>
      <div className="flex min-h-8 min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1">{children}</div>
    </li>
  );
}

/** Peak meters, polled at 10 Hz while mounted (only while the voice changer runs) and visible. */
function Meters() {
  const [levels, setLevels] = useState<VoiceLevels>({ input: 0, output: 0 });
  usePolling(async () => setLevels(await api.voiceLevels()), 100);
  return (
    <>
      <Field label="Your mic">
        <Meter level={levels.input} />
      </Field>
      <Field label="Changed voice">
        <Meter level={levels.output} />
      </Field>
    </>
  );
}

/** -60..0 dBFS bar. Scaled with a transform (no layout, no animation). */
function Meter({ level }: { level: number }) {
  const dbfs = level > 0 ? 20 * Math.log10(level) : -60;
  const fill = Math.min(1, Math.max(0, (dbfs + 60) / 60));
  return (
    <div className="h-1.5 max-w-xs overflow-hidden rounded-full bg-hover">
      <div
        className={cx("h-full w-full origin-left", dbfs > -3 ? "bg-danger" : dbfs > -12 ? "bg-warning" : "bg-success")}
        style={{ transform: `scaleX(${fill})` }}
      />
    </div>
  );
}
