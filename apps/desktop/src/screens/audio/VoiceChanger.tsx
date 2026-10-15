// Voice changer: enable, presets, devices, gains, noise gate, level meter, status.
import type { AudioDeviceInfo, VoiceLevels, VoiceSettings } from "@omni/protocol";
import { useState } from "react";
import { Button, Field, IconButton, Notice, Range, Section, Select, StatusDot, Switch } from "../../components/ui";
import { EFFECT_LABELS } from "../../lib/actions";
import { api } from "../../lib/backend";
import { usePolling, useResource } from "../../lib/hooks";
import { patchSection, saveVoicePreset, useApp, useRuntime, useSettings, useVoicePresets } from "../../lib/store";
import { toast, toastError } from "../../lib/toast";
import { cx, newId, uniqueName } from "../../lib/util";
import { PresetEditor } from "./PresetEditor";

const db = (v: number) => `${v > 0 ? "+" : ""}${v.toFixed(0)} dB`;

function deviceOptions(devices: AudioDeviceInfo[], none: string) {
  return [
    { value: "", label: none },
    ...devices.map((d) => ({ value: d.id, label: `${d.name}${d.isVirtual ? " · virtual" : ""}${d.isDefault ? " · default" : ""}` })),
  ];
}

export function VoiceChanger() {
  const { voice } = useSettings();
  const status = useRuntime().voice;
  const presets = useVoicePresets();
  const custom = useApp((s) => s.config.voicePresets);
  const linux = useApp((s) => s.info.os === "linux");
  const devices = useResource(() => api.voiceDevices());
  const set = (patch: Partial<VoiceSettings>, delay = 0) => patchSection("voice", patch, delay);
  const selected = presets.find((p) => p.id === voice.presetId);

  const createMic = async () => {
    try {
      const id = await api.createVirtualMic();
      devices.reload();
      set({ outputDevice: id });
      toast("Virtual mic created and selected as output. Pick it as the microphone in your apps.", "success");
    } catch (error) {
      toastError(error);
    }
  };

  const duplicate = () => {
    if (!selected) return;
    const copy = {
      id: newId(),
      name: uniqueName(`${selected.name} copy`, presets.map((p) => p.name)),
      effects: structuredClone(selected.effects),
    };
    saveVoicePreset(copy);
    set({ presetId: copy.id });
  };

  const tone = status.error ? "error" : status.running ? "ok" : "off";

  return (
    <Section
      title="Voice changer"
      actions={
        <>
          <StatusDot tone={tone} />
          <span className="text-xs text-muted">{status.error ? "Error" : status.running ? "Running" : "Off"}</span>
          <Switch label="Voice changer" checked={voice.enabled} onChange={(enabled) => set({ enabled })} />
        </>
      }
    >
      <div className="flex flex-col gap-3">
        {status.error && <Notice tone="error">{status.error}</Notice>}
        {status.virtualMic && status.running && (
          <Notice>
            In Discord, OBS, Zoom..., pick <b>{status.virtualMic}</b> as the microphone.
          </Notice>
        )}
        {status.hint && !status.virtualMic && <Notice tone="warn">{status.hint}</Notice>}

        <div role="radiogroup" aria-label="Voice preset" className="grid grid-cols-[repeat(auto-fill,minmax(128px,1fr))] gap-1.5">
          {presets.map((p) => (
            <button
              key={p.id}
              type="button"
              role="radio"
              aria-checked={p.id === voice.presetId}
              onClick={() => set({ presetId: p.id })}
              className={cx(
                "flex h-14 flex-col justify-center rounded-ui-sm border px-3 text-left",
                p.id === voice.presetId ? "border-accent bg-hover" : "border-line bg-bg hover:bg-hover",
              )}
            >
              <span className="truncate font-medium">{p.name}</span>
              <span className="truncate text-xs text-muted">
                {p.effects.length ? p.effects.map((e) => EFFECT_LABELS[e.type]).join(" + ") : "No effects"}
              </span>
            </button>
          ))}
        </div>

        <Meters enabled={voice.enabled} />

        <div className="grid grid-cols-1 gap-x-8 xl:grid-cols-2">
          <div>
            <Field compact label="Microphone">
              <Select
                ariaLabel="Input device"
                value={voice.inputDevice ?? ""}
                options={deviceOptions(devices.data?.inputs ?? [], "System default")}
                onChange={(id) => set({ inputDevice: id || null })}
              />
            </Field>
            <Field
              compact
              label="Output"
              hint={linux ? undefined : "Usually a virtual cable that apps use as their microphone."}
            >
              <div className="flex gap-1.5">
                <Select
                  ariaLabel="Output device"
                  value={voice.outputDevice ?? ""}
                  options={deviceOptions(devices.data?.outputs ?? [], "Not set")}
                  onChange={(id) => set({ outputDevice: id || null })}
                />
                <IconButton icon="refresh-cw" label="Refresh devices" className="size-8" onClick={devices.reload} />
              </div>
              {linux && (
                <Button size="sm" icon="plus" className="mt-1.5" onClick={() => void createMic()}>
                  Create virtual mic
                </Button>
              )}
            </Field>
            <Field compact label="Monitor">
              <div className="flex items-center gap-2">
                <Switch label="Monitor" checked={voice.monitor} onChange={(monitor) => set({ monitor })} />
                <Select
                  ariaLabel="Monitor device"
                  value={voice.monitorDevice ?? ""}
                  options={deviceOptions(devices.data?.outputs ?? [], "System default")}
                  onChange={(id) => set({ monitorDevice: id || null })}
                />
              </div>
            </Field>
            {devices.error && <Notice tone="error">{devices.error}</Notice>}
          </div>
          <div>
            <Field compact label="Input gain">
              <Range label="Input gain" min={-24} max={24} step={1} value={voice.inputGainDb} format={db} onChange={(inputGainDb) => set({ inputGainDb }, 150)} />
            </Field>
            <Field compact label="Output gain">
              <Range label="Output gain" min={-24} max={24} step={1} value={voice.outputGainDb} format={db} onChange={(outputGainDb) => set({ outputGainDb }, 150)} />
            </Field>
            <Field compact label="Noise gate">
              <div className="flex items-center gap-3">
                <Switch label="Noise gate" checked={voice.noiseGate} onChange={(noiseGate) => set({ noiseGate })} />
                <Range
                  label="Gate threshold"
                  className="flex-1"
                  min={-90}
                  max={-20}
                  step={1}
                  disabled={!voice.noiseGate}
                  value={voice.gateThresholdDb}
                  format={db}
                  onChange={(gateThresholdDb) => set({ gateThresholdDb }, 150)}
                />
              </div>
            </Field>
          </div>
        </div>

        {selected && (
          <PresetEditor
            key={selected.id}
            preset={selected}
            custom={custom.some((p) => p.id === selected.id)}
            onDuplicate={duplicate}
          />
        )}
      </div>
    </Section>
  );
}

/** Peak meters, polled at 10 Hz only while the voice changer is on and visible. */
function Meters({ enabled }: { enabled: boolean }) {
  const [levels, setLevels] = useState<VoiceLevels>({ input: 0, output: 0 });
  usePolling(async () => setLevels(await api.voiceLevels()), 100, enabled);
  const shown = enabled ? levels : { input: 0, output: 0 };
  return (
    <div className="grid grid-cols-[56px_1fr] items-center gap-x-3 gap-y-1.5 text-xs text-muted">
      <span>Input</span>
      <Meter level={shown.input} />
      <span>Output</span>
      <Meter level={shown.output} />
    </div>
  );
}

/** -60..0 dBFS bar. Scaled with a transform (no layout, no animation). */
function Meter({ level }: { level: number }) {
  const dbfs = level > 0 ? 20 * Math.log10(level) : -60;
  const fill = Math.min(1, Math.max(0, (dbfs + 60) / 60));
  return (
    <div className="h-1.5 overflow-hidden rounded-full bg-hover">
      <div
        className={cx("h-full w-full origin-left", dbfs > -3 ? "bg-danger" : dbfs > -12 ? "bg-warning" : "bg-success")}
        style={{ transform: `scaleX(${fill})` }}
      />
    </div>
  );
}
