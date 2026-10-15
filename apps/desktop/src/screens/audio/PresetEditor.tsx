// Custom voice preset editor: name, ordered effect chain, typed parameter sliders.
// Built-in presets are read-only and can be duplicated.
import type { Effect, VoicePreset } from "@omni/protocol";
import { Button, EnumSelect, IconButton, Range, Select, TextInput } from "../../components/ui";
import { EFFECT_LABELS, FILTER_LABELS, defaultEffect } from "../../lib/actions";
import { confirm } from "../../lib/confirm";
import { deleteVoicePreset, saveVoicePreset } from "../../lib/store";
import { moveItem } from "../../lib/util";

type NumericKey<E> = { [K in keyof E]-?: E[K] extends number ? K : never }[keyof E] & string;

interface Param<E> {
  key: NumericKey<E>;
  label: string;
  min: number;
  max: number;
  step: number;
  format: (value: number) => string;
  /** Logarithmic slider (frequencies). */
  log?: boolean;
}

const isEffectType = (value: string): value is Effect["type"] => Object.hasOwn(EFFECT_LABELS, value);

type EffectOf<T extends Effect["type"]> = Extract<Effect, { type: T }>;

const pct = (v: number) => `${Math.round(v * 100)}%`;
const hz = (v: number) => (v >= 1000 ? `${(v / 1000).toFixed(1)} kHz` : `${Math.round(v)} Hz`);
const ms = (v: number) => `${Math.round(v)} ms`;
const rate = (v: number) => `${v.toFixed(2)} Hz`;
const MIX = { key: "mix", label: "Mix", min: 0, max: 1, step: 0.01, format: pct } as const;

const PARAMS: { [T in Effect["type"]]: readonly Param<EffectOf<T>>[] } = {
  pitch: [{ key: "semitones", label: "Pitch", min: -12, max: 12, step: 0.5, format: (v) => `${v > 0 ? "+" : ""}${v} st` }],
  robot: [{ key: "frequency", label: "Frequency", min: 30, max: 400, step: 1, format: hz }, MIX],
  ringMod: [{ key: "frequency", label: "Frequency", min: 10, max: 2000, step: 1, format: hz, log: true }, MIX],
  echo: [
    { key: "delayMs", label: "Delay", min: 20, max: 1000, step: 5, format: ms },
    { key: "feedback", label: "Feedback", min: 0, max: 0.95, step: 0.01, format: pct },
    MIX,
  ],
  reverb: [
    { key: "roomSize", label: "Room size", min: 0, max: 1, step: 0.01, format: pct },
    { key: "damping", label: "Damping", min: 0, max: 1, step: 0.01, format: pct },
    MIX,
  ],
  distortion: [{ key: "drive", label: "Drive", min: 0, max: 1, step: 0.01, format: pct }, MIX],
  filter: [
    { key: "frequency", label: "Cutoff", min: 20, max: 20000, step: 1, format: hz, log: true },
    { key: "q", label: "Q", min: 0.1, max: 10, step: 0.1, format: (v) => v.toFixed(1) },
  ],
  chorus: [
    { key: "rateHz", label: "Rate", min: 0.05, max: 5, step: 0.05, format: rate },
    { key: "depthMs", label: "Depth", min: 0.5, max: 20, step: 0.5, format: ms },
    MIX,
  ],
  flanger: [
    { key: "rateHz", label: "Rate", min: 0.05, max: 5, step: 0.05, format: rate },
    { key: "depthMs", label: "Depth", min: 0.1, max: 10, step: 0.1, format: (v) => `${v.toFixed(1)} ms` },
    { key: "feedback", label: "Feedback", min: 0, max: 0.95, step: 0.01, format: pct },
    MIX,
  ],
  tremolo: [
    { key: "rateHz", label: "Rate", min: 0.5, max: 20, step: 0.1, format: rate },
    { key: "depth", label: "Depth", min: 0, max: 1, step: 0.01, format: pct },
  ],
  bitcrush: [
    { key: "bits", label: "Bits", min: 2, max: 16, step: 1, format: (v) => `${v} bit` },
    { key: "downsample", label: "Downsample", min: 1, max: 16, step: 1, format: (v) => `${v}x` },
  ],
  gain: [{ key: "db", label: "Gain", min: -24, max: 24, step: 0.5, format: (v) => `${v > 0 ? "+" : ""}${v} dB` }],
};

export function PresetEditor({ preset, custom, onDuplicate }: { preset: VoicePreset; custom: boolean; onDuplicate: () => void }) {
  const save = (next: VoicePreset) => saveVoicePreset(next, 250);
  const setEffects = (effects: Effect[]) => save({ ...preset, effects });

  const remove = async () => {
    if (await confirm({ title: `Delete ${preset.name}?`, confirmLabel: "Delete", danger: true })) void deleteVoicePreset(preset.id);
  };

  return (
    <div className="border-t border-line pt-3">
      <div className="mb-2 flex items-center gap-2">
        {custom ? (
          <TextInput aria-label="Preset name" className="w-56 font-semibold" value={preset.name} onChange={(e) => save({ ...preset, name: e.target.value })} />
        ) : (
          <h3 className="font-semibold">{preset.name}</h3>
        )}
        <span className="text-xs text-muted">{custom ? "Custom preset" : "Built-in, duplicate it to change it"}</span>
        <div className="ml-auto flex gap-1.5">
          <Button size="sm" icon="copy" onClick={onDuplicate}>
            Duplicate
          </Button>
          {custom && (
            <Button size="sm" variant="danger" icon="trash-2" onClick={() => void remove()}>
              Delete
            </Button>
          )}
        </div>
      </div>

      {preset.effects.length === 0 && <p className="py-2 text-muted">No effects. The voice passes through clean.</p>}
      <ol className="flex flex-col gap-1.5">
        {preset.effects.map((effect, i) => (
          <li key={i} className="rounded-ui-sm border border-line bg-bg px-3 py-2">
            <div className="flex items-center gap-1">
              <span className="w-5 text-xs text-muted tabular-nums">{i + 1}</span>
              <span className="font-medium">{EFFECT_LABELS[effect.type]}</span>
              {custom && (
                <div className="ml-auto flex">
                  <IconButton icon="arrow-up" label="Move up" disabled={i === 0} onClick={() => setEffects(moveItem(preset.effects, i, i - 1))} />
                  <IconButton
                    icon="arrow-down"
                    label="Move down"
                    disabled={i === preset.effects.length - 1}
                    onClick={() => setEffects(moveItem(preset.effects, i, i + 1))}
                  />
                  <IconButton icon="x" label="Remove effect" onClick={() => setEffects(preset.effects.filter((_, j) => j !== i))} />
                </div>
              )}
            </div>
            <fieldset disabled={!custom} className="mt-1 grid grid-cols-1 gap-x-6 gap-y-0.5 pl-6 lg:grid-cols-2">
              <EffectFields effect={effect} onChange={(next) => setEffects(preset.effects.map((e, j) => (j === i ? next : e)))} />
            </fieldset>
          </li>
        ))}
      </ol>

      {custom && (
        <div className="mt-2 w-56">
          <Select
            ariaLabel="Add effect"
            value=""
            placeholder="Add effect…"
            options={Object.entries(EFFECT_LABELS).map(([value, label]) => ({ value, label }))}
            onChange={(type) => isEffectType(type) && setEffects([...preset.effects, defaultEffect(type)])}
          />
        </div>
      )}
    </div>
  );
}

function EffectFields({ effect, onChange }: { effect: Effect; onChange: (effect: Effect) => void }) {
  switch (effect.type) {
    case "pitch":
      return <Params effect={effect} params={PARAMS.pitch} onChange={onChange} />;
    case "robot":
      return <Params effect={effect} params={PARAMS.robot} onChange={onChange} />;
    case "ringMod":
      return <Params effect={effect} params={PARAMS.ringMod} onChange={onChange} />;
    case "echo":
      return <Params effect={effect} params={PARAMS.echo} onChange={onChange} />;
    case "reverb":
      return <Params effect={effect} params={PARAMS.reverb} onChange={onChange} />;
    case "distortion":
      return <Params effect={effect} params={PARAMS.distortion} onChange={onChange} />;
    case "filter":
      return (
        <>
          <label className="flex h-8 items-center gap-3">
            <span className="w-20 shrink-0 text-muted">Type</span>
            <EnumSelect ariaLabel="Filter type" value={effect.kind} labels={FILTER_LABELS} onChange={(kind) => onChange({ ...effect, kind })} />
          </label>
          <Params effect={effect} params={PARAMS.filter} onChange={onChange} />
        </>
      );
    case "chorus":
      return <Params effect={effect} params={PARAMS.chorus} onChange={onChange} />;
    case "flanger":
      return <Params effect={effect} params={PARAMS.flanger} onChange={onChange} />;
    case "tremolo":
      return <Params effect={effect} params={PARAMS.tremolo} onChange={onChange} />;
    case "bitcrush":
      return <Params effect={effect} params={PARAMS.bitcrush} onChange={onChange} />;
    case "gain":
      return <Params effect={effect} params={PARAMS.gain} onChange={onChange} />;
  }
}

const LOG_STEPS = 1000;

function Params<E extends Effect>({ effect, params, onChange }: { effect: E; params: readonly Param<E>[]; onChange: (effect: Effect) => void }) {
  return params.map((p) => {
    const raw = effect[p.key];
    const value = typeof raw === "number" ? raw : p.min;
    const set = (v: number) => onChange({ ...effect, [p.key]: v });
    return (
      <label key={p.key} className="flex h-8 items-center gap-3">
        <span className="w-20 shrink-0 text-muted">{p.label}</span>
        {p.log ? (
          <Range
            label={p.label}
            className="flex-1"
            min={0}
            max={LOG_STEPS}
            step={1}
            value={Math.round((Math.log(value / p.min) / Math.log(p.max / p.min)) * LOG_STEPS)}
            format={() => p.format(value)}
            onChange={(pos) => set(Math.round(p.min * (p.max / p.min) ** (pos / LOG_STEPS)))}
          />
        ) : (
          <Range label={p.label} className="flex-1" min={p.min} max={p.max} step={p.step} value={value} format={p.format} onChange={set} />
        )}
      </label>
    );
  });
}
