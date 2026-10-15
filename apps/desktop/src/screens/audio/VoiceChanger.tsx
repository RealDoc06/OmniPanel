// Voice changer screen: status and switch, one-time setup per OS, the voice grid,
// microphone and monitor rows, level meters, Advanced gains and the voice editor.
import type { AudioDeviceInfo, Effect, VoiceLevels, VoiceSettings, VoiceStatus } from "@omni/protocol";
import { useState } from "react";
import { I } from "../../components/Icon";
import { Button, Disclosure, Field, Notice, Range, ScreenHeader, Section, Select, Switch } from "../../components/ui";
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

/** Setup takes priority until an output is chosen; runtime failures explain a stopped voice. */
function voiceState(voice: VoiceSettings, status: VoiceStatus): keyof typeof STATE_TONES {
  if (!voice.enabled) return "Off";
  if (voice.outputDevice === null) return "Needs setup";
  return status.error ? "Stopped" : "On";
}

const openLink = (url: string) => void run(api.runAction({ type: "open", target: url }));

export function VoiceChangerScreen() {
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
    <>
      <ScreenHeader title="Voice changer">
        <span className={cx("text-xs", STATE_TONES[state])}>{state}</span>
        <Switch label="Voice changer" checked={voice.enabled} onChange={(enabled) => set({ enabled })} />
      </ScreenHeader>
      <div className="flex-1 overflow-y-auto">
        <p className="px-6 pt-4">Changes how your microphone sounds in calls and recordings.</p>
        {state === "Stopped" && (
          <div className="px-6 pt-4">
            <Notice tone="error">The voice changer stopped: {status.error}</Notice>
          </div>
        )}

        <Setup voice={voice} status={status} outputs={devices.data?.outputs} reload={devices.reload} />

        <Section title="Voices">
          <div role="radiogroup" aria-label="Voice" className="grid grid-cols-[repeat(auto-fill,minmax(132px,1fr))] gap-x-2">
            {presets.map((p) => {
              const on = p.id === voice.presetId;
              return (
                <button
                  key={p.id}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  title={p.effects.map((e) => EFFECT_LABELS[e.type]).join(", ") || "No effects"}
                  onClick={() => set({ presetId: p.id })}
                  className={cx("flex h-8 min-w-0 items-center gap-1.5 rounded-ui-sm px-2 text-left hover:bg-hover", on && "font-medium text-accent")}
                >
                  <span className="w-3.5 shrink-0">{on && <I name="check" className="size-3.5" />}</span>
                  <span className="truncate">{p.name}</span>
                </button>
              );
            })}
            <Button variant="ghost" icon="plus" className="h-8 justify-self-stretch justify-start px-2" onClick={() => addVoice("My voice", [])}>
              New voice
            </Button>
          </div>
        </Section>

        <div className="px-6 py-4">
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

          <div className="mt-3">
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
        </div>
      </div>
    </>
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
      <div className="flex items-center gap-3 border-b border-line px-6 py-3">
        <p className="min-w-0 flex-1">
          Apps hear you through <span className="font-semibold">{output}</span>. In Discord, Zoom or OBS, choose{" "}
          <span className="font-semibold">{status.virtualMic ?? micFor(output)}</span> as your microphone.
        </p>
        <Button size="sm" onClick={() => setChanging(true)}>
          Change
        </Button>
      </div>
    );
  }

  return (
    <Section
      title="Setup"
      actions={
        changing && (
          <Button size="sm" variant="ghost" onClick={() => setChanging(false)}>
            Done
          </Button>
        )
      }
    >
      <ol className="flex flex-col gap-1">
        <li className="flex min-h-8 flex-wrap items-center gap-3">
          <span className="w-4 shrink-0 text-muted tabular-nums">
            {prerequisite ? <I name="check" className="size-4 text-success" /> : "1"}
          </span>
          <span>{!outputs ? "Checking audio devices…" : prerequisite ? installed : install}</span>
          {outputs && !prerequisite && (download ? (
            <Button size="sm" icon="external-link" onClick={() => openLink(download.url)}>
              {download.label}
            </Button>
          ) : (
            <Button size="sm" icon="plus" onClick={() => void createMic()}>
              Create virtual mic
            </Button>
          ))}
        </li>
        <li className="flex min-h-8 flex-wrap items-center gap-x-3 gap-y-1">
          <span className="w-4 shrink-0 text-muted tabular-nums">
            {selectedOutput ? <I name="check" className="size-4 text-success" /> : "2"}
          </span>
          <label className="flex max-w-full items-center gap-3">
            <span className="shrink-0">Send your changed voice to</span>
            <Select
              ariaLabel="Send your changed voice to"
              className="w-64 min-w-0"
              value={voice.outputDevice ?? ""}
              placeholder="Choose…"
              options={deviceOptions(outputs ?? [])}
              onChange={setOutput}
            />
          </label>
          <Button size="sm" onClick={reload}>
            Refresh
          </Button>
          {suggested && (
            <Button size="sm" variant="primary" onClick={() => setOutput(suggested.id)}>
              Use {suggested.name} for your changed voice
            </Button>
          )}
        </li>
        <li className="flex min-h-8 items-center gap-3">
          <span className="w-4 shrink-0 text-muted tabular-nums">3</span>
          <span>In Discord, Zoom or OBS, choose "{status.virtualMic ?? (selectedOutput ? micFor(selectedOutput.name) : mic)}" as your microphone.</span>
        </li>
      </ol>
    </Section>
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
