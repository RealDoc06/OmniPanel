// "What it does": the inspector section that shows a control's action as a plain
// summary with [Change] (opens the ActionPicker), and the typed form for every
// `Action` variant. Nested actions (steps, alternating presses) reuse <ActionEditor />
// with a "Does" select; the top level is chosen through the picker instead.
import type { Action, Control, HaOp, JsonValue, NavTarget, ObsOp, VoiceOp, VolumeOp } from "@omni/protocol";
import { useId, useRef, useState, type ReactNode } from "react";
import { HotkeyInput } from "../../components/HotkeyInput";
import { I } from "../../components/Icon";
import { DeviceSelect, useDeviceName } from "../../components/DeviceSelect";
import { AppField, VolumeTargetPicker } from "../../components/VolumeTargetPicker";
import {
  Button,
  ComboInput,
  Disclosure,
  EnumSelect,
  Field,
  Group,
  IconButton,
  Notice,
  NumberField,
  Range,
  Segmented,
  Select,
  Switch,
  TextArea,
  TextInput,
  inputClass,
} from "../../components/ui";
import {
  ACTION_GROUPS,
  ACTION_META,
  APP_DEVICE_LABELS,
  ACTION_TYPES,
  HTTP_METHODS,
  INTERPRETER_LABELS,
  MEDIA_LABELS,
  NAV_LABELS,
  OBS_OP_LABELS,
  SYSTEM_LABELS,
  VOICE_OP_LABELS,
  VOLUME_OP_LABELS,
  defaultAction,
  describeAction,
  isActionType,
  type ActionOf,
  type NameLookup,
} from "../../lib/actions";
import { api, dialog } from "../../lib/backend";
import { useResource } from "../../lib/hooks";
import { applyAutoLabel, applyPick, type PickResult } from "../../lib/recipes";
import { navigate, useApp, useProfiles, useSettings, useVoicePresets } from "../../lib/store";
import { cx, moveItem, newId } from "../../lib/util";
import { ActionPicker } from "./ActionPicker";
import { useCurrent } from "./editor";

/** Names for ids in action summaries (pages of the open layout, layouts, voices). */
export function useActionNames(): NameLookup {
  const pages = useCurrent()?.profile.pages ?? [];
  const profiles = useProfiles();
  const presets = useVoicePresets();
  return {
    page: (id) => {
      const page = pages.find((p) => p.id === id);
      return page && (page.name || "Untitled page");
    },
    profile: (id) => profiles.find((p) => p.id === id)?.name,
    preset: (id) => presets.find((p) => p.id === id)?.name,
  };
}

/**
 * The whole "What it does" section of the inspector: summary row with [Change],
 * the picker, and the form (or the slider fields). Picks follow the auto-label rule
 * (`applyPick`): a label the user typed is kept.
 */
export function WhatItDoes({ control, onChange }: { control: Control; onChange: (next: Pick<Control, "kind" | "style">) => void }) {
  const [picking, setPicking] = useState(false);
  const { kind, style } = control;
  const action = kind.type === "button" ? kind.action : null;
  const ha = action?.type === "homeAssistant";
  const entities = useResource(() => (ha ? api.haEntities() : Promise.resolve([])), [ha]);
  const apps = useResource(() => action?.type === "openApp" ? api.installedApps() : Promise.resolve([]), [action?.type === "openApp"]);
  const names: NameLookup = {
    ...useActionNames(),
    entity: (id) => entities.data?.find((e) => e.entityId === id)?.name,
    device: useDeviceName(action),
  };
  const changeKind = (next: Control["kind"]) => onChange({
    kind: next,
    style: applyAutoLabel(style, kind, next, { ...names, app: (launch) => apps.data?.find((app) => app.launch === launch)?.name }),
  });

  const pick = (result: PickResult) => {
    setPicking(false);
    // A button turned into a slider keeps its shape: tall means vertical.
    const next = result.kind.type === "slider" ? { ...result.kind, vertical: control.h >= control.w } : result.kind;
    onChange({ kind: next, style: applyPick(style, result, kind, { ...names, app: (launch) => apps.data?.find((app) => app.launch === launch)?.name }) });
  };

  return (
    <Group title="What it does" padded={false}>
      {picking ? (
        <ActionPicker current={control} onPick={pick} onCancel={() => setPicking(false)} />
      ) : (
        <div className="flex flex-col px-5">
          {kind.type === "button" && !kind.action ? (
            <div className="flex items-center gap-2">
              <span>Does nothing yet.</span>
              <Button size="sm" variant="primary" className="ml-auto" onClick={() => setPicking(true)}>
                Choose what it does
              </Button>
            </div>
          ) : (
            <div className="flex min-h-8 items-center gap-2">
              <I name={kind.type === "slider" ? "sliders-vertical" : kind.action ? ACTION_META[kind.action.type].icon : "square"} className="size-4 shrink-0" />
              <span className="min-w-0 flex-1 truncate">{kind.type === "slider" ? "Volume slider" : describeAction(kind.action, names)}</span>
              <Button size="sm" onClick={() => setPicking(true)}>
                Change
              </Button>
            </div>
          )}
          {kind.type === "slider" ? (
            <>
              <Field label="Sound" compact>
                <VolumeTargetPicker value={kind.target} onChange={(target) => changeKind({ ...kind, target })} />
              </Field>
              <Field label="Direction" compact>
                <Segmented
                  label="Direction"
                  value={kind.vertical ? "vertical" : "horizontal"}
                  onChange={(v) => onChange({ kind: { ...kind, vertical: v === "vertical" }, style })}
                  options={[
                    { value: "vertical", label: "Vertical" },
                    { value: "horizontal", label: "Horizontal" },
                  ]}
                />
              </Field>
            </>
          ) : (
            kind.action && <ActionEditor action={kind.action} onChange={(next) => changeKind({ type: "button", action: next })} />
          )}
        </div>
      )}
    </Group>
  );
}

type FormProps<T extends Action["type"]> = { action: ActionOf<T>; onChange: (action: Action) => void; depth: number };

/**
 * Form for one action. At depth 0 (the button itself) it is the form only; nested
 * steps (depth 1 and below) start with a "Does" select, the only place "Wait" is offered.
 */
export function ActionEditor({ action, onChange, depth = 0 }: { action: Action; onChange: (action: Action) => void; depth?: number }) {
  const id = useId();
  return (
    <div className="flex flex-col gap-0.5">
      {depth > 0 && (
        <Field label="Does" htmlFor={id} compact>
          <select
            id={id}
            className={inputClass}
            value={action.type}
            onChange={(e) => {
              const type = e.target.value;
              if (isActionType(type) && type !== action.type) onChange(defaultAction(type));
            }}
          >
            {ACTION_GROUPS.map((group) => (
              <optgroup key={group} label={group}>
                {ACTION_TYPES.filter((t) => ACTION_META[t].group === group).map((t) => (
                  <option key={t} value={t}>
                    {ACTION_META[t].label}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
        </Field>
      )}
      <ActionForm action={action} onChange={onChange} depth={depth} />
    </div>
  );
}

function ActionForm({ action, onChange, depth }: { action: Action; onChange: (action: Action) => void; depth: number }) {
  switch (action.type) {
    case "hotkey":
      return (
        <Field label="Keys" compact>
          <HotkeyInput variant="send" value={action.keys} onChange={(keys) => onChange({ ...action, keys })} />
        </Field>
      );
    case "typeText":
      return (
        <Field label="Text to type" compact>
          <TextArea aria-label="Text to type" rows={3} value={action.text} onChange={(e) => onChange({ ...action, text: e.target.value })} />
        </Field>
      );
    case "openApp":
      return <OpenAppForm action={action} onChange={onChange} depth={depth} />;
    case "open":
      return <OpenForm action={action} onChange={onChange} depth={depth} />;
    case "runScript":
      return <ScriptForm action={action} onChange={onChange} depth={depth} />;
    case "media":
      return (
        <Field label="Do" compact>
          <EnumSelect ariaLabel="Media key" value={action.command} labels={MEDIA_LABELS} onChange={(command) => onChange({ ...action, command })} />
        </Field>
      );
    case "volume":
      return <VolumeForm action={action} onChange={onChange} depth={depth} />;
    case "appDevice":
      return <AppDeviceForm action={action} onChange={onChange} depth={depth} />;
    case "voice":
      return <VoiceForm action={action} onChange={onChange} depth={depth} />;
    case "obs":
      return <ObsForm action={action} onChange={onChange} depth={depth} />;
    case "homeAssistant":
      return <HomeAssistantForm action={action} onChange={onChange} depth={depth} />;
    case "http":
      return <HttpForm action={action} onChange={onChange} depth={depth} />;
    case "system":
      return (
        <Field label="Do" compact>
          <EnumSelect ariaLabel="Computer" value={action.command} labels={SYSTEM_LABELS} onChange={(command) => onChange({ ...action, command })} />
        </Field>
      );
    case "navigate":
      return <NavigateForm action={action} onChange={onChange} depth={depth} />;
    case "multi":
      return <MultiForm action={action} onChange={onChange} depth={depth} />;
    case "toggle":
      return <ToggleForm action={action} onChange={onChange} depth={depth} />;
    case "delay":
      return (
        <Field label="Wait" compact>
          <Seconds ariaLabel="Wait" ms={action.ms} step={0.1} onChange={(ms) => onChange({ ...action, ms })} />
        </Field>
      );
  }
}

/**
 * Seconds field over a value stored in ms. Capped at 60 s, the engine's limit for
 * waits (`MAX_DELAY_MS` in crates/omni-host/src/dispatch.rs).
 */
function Seconds({ ms, onChange, step, ariaLabel }: { ms: number; onChange: (ms: number) => void; step: number; ariaLabel: string }) {
  return (
    <NumberField
      ariaLabel={ariaLabel}
      className="w-28"
      value={ms / 1000}
      min={0}
      max={60}
      step={step}
      suffix="s"
      onChange={(s) => onChange(Math.round(s * 1000))}
    />
  );
}

/**
 * Local text for a field whose saved value can change underneath it (undo or AI).
 * The draft shows while the saved value is still the one it was typed
 * against (`saved`, compared as a string); any other change replaces it with `shown`.
 * Call `setText(text, savedAfter)` with the value the text saved, or the old one.
 */
function useDraft(saved: string, shown: string) {
  const [draft, setDraft] = useState({ text: shown, saved });
  // Replace the draft itself so redo cannot resurrect text discarded by undo.
  if (draft.saved !== saved) setDraft({ text: shown, saved });
  const text = draft.saved === saved ? draft.text : shown;
  return [text, (next: string, savedAfter: string) => setDraft({ text: next, saved: savedAfter })] as const;
}

/** "Set up" link for integration hints. */
function SetUpLink({ children, onClick }: { children: ReactNode; onClick: () => void }) {
  return (
    <button type="button" className="text-accent hover:underline" onClick={onClick}>
      {children}
    </button>
  );
}

const hintClass = "pl-[100px] text-xs text-muted @max-[320px]:pl-0";

const parseArgs = (text: string) => text.split("\n").filter((a) => a.trim() !== "");

function OpenAppForm({ action, onChange }: FormProps<"openApp">) {
  const apps = useResource(() => api.installedApps());
  const installed = apps.data ?? [];
  // Show app names; store what the host launches (a path on macOS and Windows).
  const shown = installed.find((a) => a.launch === action.app)?.name ?? action.app;
  const savedArgs = JSON.stringify(action.args);
  const [args, setArgs] = useDraft(savedArgs, action.args.join("\n"));
  return (
    <>
      <Field label="App" compact>
        <ComboInput
          ariaLabel="App"
          value={shown}
          placeholder="Pick or type an app"
          options={installed.map((a) => ({ value: a.name, label: a.name }))}
          onChange={(app) => onChange({ ...action, app: installed.find((a) => a.name === app)?.launch ?? app })}
        />
      </Field>
      <Disclosure label="Advanced">
        <Field label="Launch options" compact hint="One per line.">
          <TextArea
            aria-label="Launch options"
            rows={2}
            className="font-mono text-xs"
            value={args}
            onChange={(e) => {
              const next = parseArgs(e.target.value);
              setArgs(e.target.value, JSON.stringify(next));
              onChange({ ...action, args: next });
            }}
          />
        </Field>
      </Disclosure>
    </>
  );
}

function OpenForm({ action, onChange }: FormProps<"open">) {
  const browse = async (directory: boolean) => {
    const path = await dialog.open({ directory });
    if (path) onChange({ ...action, target: path });
  };
  return (
    <Field label="Opens" compact>
      <div className="flex flex-col gap-1">
        <TextInput aria-label="Opens" value={action.target} placeholder="https://example.com" onChange={(e) => onChange({ ...action, target: e.target.value })} />
        <div className="flex gap-1">
          <Button size="sm" onClick={() => void browse(false)}>
            Choose file…
          </Button>
          <Button size="sm" onClick={() => void browse(true)}>
            Choose folder…
          </Button>
        </div>
      </div>
    </Field>
  );
}

function ScriptForm({ action, onChange }: FormProps<"runScript">) {
  const mac = useApp((s) => s.info.os === "macos");
  const interpreters = mac ? undefined : (["shell", "bash", "powershell", "python", "node"] as const);
  return (
    <>
      <Field label="Language" compact>
        <EnumSelect
          ariaLabel="Language"
          value={action.interpreter}
          labels={INTERPRETER_LABELS}
          only={interpreters}
          onChange={(interpreter) => onChange({ ...action, interpreter })}
        />
      </Field>
      <TextArea
        aria-label="Script"
        rows={8}
        placeholder={action.interpreter === "applescript" ? 'For example: tell application "Music" to playpause' : "For example: echo hello"}
        className="mt-1 font-mono text-xs leading-relaxed"
        value={action.script}
        onChange={(e) => onChange({ ...action, script: e.target.value })}
      />
      <label className="flex h-8 items-center gap-2">
        <Switch label="Show the result on the device" checked={action.showOutput} onChange={(showOutput) => onChange({ ...action, showOutput })} />
        Show the result on the device
      </label>
      <Disclosure label="Advanced">
        <Field label="Stop after" compact>
          <NumberField
            ariaLabel="Stop after"
            className="w-28"
            value={action.timeoutSecs}
            min={1}
            max={3600}
            suffix="s"
            onChange={(timeoutSecs) => onChange({ ...action, timeoutSecs })}
          />
        </Field>
      </Disclosure>
    </>
  );
}

function VolumeForm({ action, onChange }: FormProps<"volume">) {
  const setOp = (type: VolumeOp["type"]): VolumeOp => {
    switch (type) {
      case "set":
        return { type, value: 0.5 };
      case "step":
        return { type, delta: 0.05 };
      case "toggleMute":
      case "mute":
      case "unmute":
        return { type };
    }
  };
  const { op } = action;
  return (
    <>
      <Field label="Sound" compact>
        <VolumeTargetPicker value={action.target} onChange={(target) => onChange({ ...action, target })} />
      </Field>
      <Field label="Do" compact>
        <EnumSelect ariaLabel="Do" value={op.type} labels={VOLUME_OP_LABELS} onChange={(type) => onChange({ ...action, op: setOp(type) })} />
      </Field>
      {op.type === "set" && (
        <Field label="Level" compact>
          <Range
            label="Level"
            min={0}
            max={1}
            step={0.01}
            value={op.value}
            format={(v) => `${Math.round(v * 100)}%`}
            onChange={(value) => onChange({ ...action, op: { type: "set", value } })}
          />
        </Field>
      )}
      {op.type === "step" && (
        <Field label="By" compact>
          <Range
            label="By"
            min={-0.5}
            max={0.5}
            step={0.01}
            value={op.delta}
            format={(v) => `${v > 0 ? "+" : ""}${Math.round(v * 100)}%`}
            onChange={(delta) => onChange({ ...action, op: { type: "step", delta } })}
          />
        </Field>
      )}
    </>
  );
}

/** The device one app plays on or records from. Says so when this computer cannot do it. */
function AppDeviceForm({ action, onChange }: FormProps<"appDevice">) {
  const mixer = useResource(() => api.getMixer());
  const capabilities = mixer.data?.capabilities;
  const output = action.direction === "output";
  const supported = !capabilities || (output ? capabilities.perAppOutput : capabilities.perAppInput);
  const note = output ? capabilities?.outputNote : capabilities?.inputNote;
  const devices = (output ? mixer.data?.devices.outputs : mixer.data?.devices.inputs) ?? [];
  return (
    <>
      <Field label="App" compact>
        <AppField value={action.app} onChange={(app) => onChange({ ...action, app })} />
      </Field>
      <Field label="Change its" compact>
        <EnumSelect
          ariaLabel="Change its"
          value={action.direction}
          labels={APP_DEVICE_LABELS}
          onChange={(direction) => onChange({ ...action, direction, device: null })}
        />
      </Field>
      <Field label={output ? "Play on" : "Record from"} compact>
        <DeviceSelect
          ariaLabel={output ? "Play on" : "Record from"}
          devices={devices}
          value={action.device === null ? { type: "default" } : { type: "device", id: action.device }}
          onChange={(device) => onChange({ ...action, device })}
        />
      </Field>
      {!supported && <Notice tone="warn">{note ?? "This computer cannot do that."}</Notice>}
    </>
  );
}

function VoiceForm({ action, onChange }: FormProps<"voice">) {
  const presets = useVoicePresets();
  const ready = useSettings().voice.outputDevice !== null;
  const { op } = action;
  const toOp = (type: VoiceOp["type"]): VoiceOp => {
    switch (type) {
      case "setPreset":
        return { type, presetId: op.type === "setPreset" ? op.presetId : (presets[0]?.id ?? "clean") };
      case "cycle":
        return { type, presetIds: [] };
      case "toggle":
      case "enable":
      case "disable":
        return { type };
    }
  };
  return (
    <>
      <Field label="Do" compact>
        <EnumSelect ariaLabel="Do" value={op.type} labels={VOICE_OP_LABELS} onChange={(type) => onChange({ ...action, op: toOp(type) })} />
      </Field>
      {op.type === "setPreset" && (
        <Field label="Voice" compact>
          <Select
            ariaLabel="Voice"
            value={op.presetId}
            options={presets.map((p) => ({ value: p.id, label: p.name }))}
            onChange={(presetId) => onChange({ ...action, op: { type: "setPreset", presetId } })}
          />
        </Field>
      )}
      {op.type === "cycle" && (
        <Field label="Voices" compact hint={op.presetIds.length ? `${op.presetIds.length} checked` : "None checked: every voice is used."}>
          <div className="grid max-h-40 grid-cols-2 gap-x-2 overflow-y-auto py-1">
            {presets.map((p) => (
              <label key={p.id} className="flex h-6 items-center gap-1.5 truncate">
                <input
                  type="checkbox"
                  checked={op.presetIds.includes(p.id)}
                  onChange={(e) =>
                    onChange({
                      ...action,
                      op: {
                        type: "cycle",
                        presetIds: e.target.checked ? [...op.presetIds, p.id] : op.presetIds.filter((x) => x !== p.id),
                      },
                    })
                  }
                />
                {p.name}
              </label>
            ))}
          </div>
        </Field>
      )}
      {!ready && (
        <p className={hintClass}>
          The voice changer needs a one-time setup. <SetUpLink onClick={() => navigate("sound", "voice")}>Set up</SetUpLink>
        </p>
      )}
    </>
  );
}

function ObsForm({ action, onChange }: FormProps<"obs">) {
  const inventory = useResource(() => api.obsInventory());
  const { op } = action;
  const options = (names: readonly string[] | undefined) => (names ?? []).map((s) => ({ value: s, label: s }));
  const set = (next: ObsOp) => onChange({ ...action, op: next });
  const toOp = (type: ObsOp["type"]): ObsOp => {
    switch (type) {
      case "setScene":
        return { type, scene: "" };
      case "toggleMute":
        return { type, input: "" };
      case "toggleSource":
        return { type, scene: "", source: "" };
      default:
        return { type };
    }
  };
  const named = op.type === "setScene" || op.type === "toggleSource" || op.type === "toggleMute";
  return (
    <>
      <Field label="Do" compact>
        <EnumSelect ariaLabel="Do" value={op.type} labels={OBS_OP_LABELS} onChange={(type) => set(toOp(type))} />
      </Field>
      {(op.type === "setScene" || op.type === "toggleSource") && (
        <Field label="Scene" compact>
          <ComboInput
            ariaLabel="Scene"
            value={op.scene}
            options={options(inventory.data?.scenes)}
            placeholder="Scene name"
            onChange={(scene) => set({ ...op, scene })}
          />
        </Field>
      )}
      {op.type === "toggleSource" && (
        <Field label="Source" compact>
          <ComboInput
            ariaLabel="Source"
            value={op.source}
            placeholder="Source name"
            options={options(inventory.data?.sceneItems.find((i) => i.scene === op.scene)?.sources)}
            onChange={(source) => set({ ...op, source })}
          />
        </Field>
      )}
      {op.type === "toggleMute" && (
        <Field label="Audio source" compact>
          <ComboInput
            ariaLabel="Audio source"
            value={op.input}
            placeholder="Like Mic/Aux"
            options={options(inventory.data?.inputs)}
            onChange={(input) => set({ ...op, input })}
          />
        </Field>
      )}
      {named && inventory.error && (
        <p className={hintClass}>
          OBS isn't connected. <SetUpLink onClick={() => navigate("settings", "obs")}>Set up OBS</SetUpLink>, or type the name.
        </p>
      )}
    </>
  );
}

const HA_OP_LABELS: Record<HaOp["type"], string> = { toggle: "Turn on / off", callService: "Run a service" };

function HomeAssistantForm({ action, onChange }: FormProps<"homeAssistant">) {
  const entities = useResource(() => api.haEntities());
  const options = (entities.data ?? []).map((e) => ({ value: e.entityId, label: e.name }));
  const { op } = action;
  const set = (next: HaOp) => onChange({ ...action, op: next });
  return (
    <>
      <Field label="Do" compact>
        <EnumSelect
          ariaLabel="Do"
          value={op.type}
          labels={HA_OP_LABELS}
          onChange={(type) =>
            set(
              type === "toggle"
                ? { type, entityId: op.entityId ?? "" }
                : { type, domain: "", service: "", entityId: op.entityId || null, data: null },
            )
          }
        />
      </Field>
      {op.type === "toggle" ? (
        <Field label="Device" compact>
          <ComboInput ariaLabel="Device" value={op.entityId} options={options} placeholder="light.desk" onChange={(entityId) => set({ ...op, entityId })} />
        </Field>
      ) : (
        <>
          <Field label="Service" compact>
            <div className="flex items-center gap-1">
              <TextInput aria-label="Domain" placeholder="scene" value={op.domain} onChange={(e) => set({ ...op, domain: e.target.value.trim() })} />
              <span className="text-muted">.</span>
              <TextInput aria-label="Service" placeholder="turn_on" value={op.service} onChange={(e) => set({ ...op, service: e.target.value.trim() })} />
            </div>
          </Field>
          <Field label="Device (optional)" compact>
            <ComboInput
              ariaLabel="Device (optional)"
              value={op.entityId ?? ""}
              options={options}
              placeholder="light.desk"
              onChange={(entityId) => set({ ...op, entityId: entityId || null })}
            />
          </Field>
          <Disclosure label="Advanced">
            <Field label="Extra data (JSON)" compact>
              <JsonObjectField value={op.data} onChange={(data) => set({ ...op, data })} />
            </Field>
          </Disclosure>
        </>
      )}
      {entities.error && (
        <p className={hintClass}>
          Home Assistant isn't connected. <SetUpLink onClick={() => navigate("settings", "ha")}>Set up Home Assistant</SetUpLink>, or type the
          device id, like light.desk.
        </p>
      )}
    </>
  );
}

const isJsonObject = (value: JsonValue): value is { [key in string]: JsonValue } =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** JSON object editor: commits only valid objects (or empty = null). Follows undo (R6-5). */
function JsonObjectField({ value, onChange }: { value: JsonValue | null; onChange: (value: JsonValue | null) => void }) {
  const saved = JSON.stringify(value);
  const [text, setText] = useDraft(saved, value === null ? "" : JSON.stringify(value, null, 2));
  const parse = (text: string) => {
    if (text.trim() === "") return { value: null, error: null };
    try {
      const parsed: JsonValue = JSON.parse(text);
      return isJsonObject(parsed) ? { value: parsed, error: null } : { value: null, error: "Must be a JSON object" };
    } catch {
      return { value: null, error: "Not valid JSON" };
    }
  };
  const { error } = parse(text);
  return (
    <>
      <TextArea
        aria-label="Extra data (JSON)"
        aria-invalid={error !== null}
        rows={4}
        placeholder={'{ "brightness_pct": 40 }'}
        className={cx("font-mono text-xs", error && "border-danger")}
        value={text}
        onChange={(e) => {
          const next = e.target.value;
          const parsed = parse(next);
          setText(next, parsed.error ? saved : JSON.stringify(parsed.value));
          if (!parsed.error) onChange(parsed.value);
        }}
      />
      {error && <p className="mt-1 text-xs text-danger">{error}</p>}
    </>
  );
}

const isWebAddress = (text: string) => {
  try {
    const url = new URL(text);
    return url.protocol === "http:" || url.protocol === "https:";
  } catch {
    return false;
  }
};

function HttpForm({ action, onChange }: FormProps<"http">) {
  const headers = action.headers;
  const setHeader = (i: number, patch: Partial<{ name: string; value: string }>) =>
    onChange({ ...action, headers: headers.map((h, j) => (j === i ? { ...h, ...patch } : h)) });
  // The address only saves once it is a full http(s) URL (T2-2).
  const [url, setUrl] = useDraft(action.url, action.url);
  const invalid = url !== action.url && !isWebAddress(url);
  return (
    <>
      <Field label="Address" compact hint={invalid && <span className="text-danger">Enter a full web address, like https://example.com</span>}>
        <div className="flex gap-1">
          <EnumSelect ariaLabel="Method" className="w-24 shrink-0" value={action.method} labels={HTTP_METHODS} onChange={(method) => onChange({ ...action, method })} />
          <TextInput
            aria-label="Address"
            aria-invalid={invalid}
            className={cx(invalid && "border-danger")}
            value={url}
            placeholder="https://example.com"
            onChange={(e) => {
              const next = e.target.value.trim();
              const valid = isWebAddress(next);
              setUrl(e.target.value, valid ? next : action.url);
              if (valid) onChange({ ...action, url: next });
            }}
          />
        </div>
      </Field>
      <Disclosure label="Advanced">
        <Field label="Headers" compact>
          <div className="flex flex-col gap-1">
            {headers.map((h, i) => (
              <div key={i} className="flex gap-1">
                <TextInput aria-label="Header name" placeholder="Name" value={h.name} onChange={(e) => setHeader(i, { name: e.target.value })} />
                <TextInput aria-label="Header value" placeholder="Value" value={h.value} onChange={(e) => setHeader(i, { value: e.target.value })} />
                <Button size="sm" variant="ghost" className="shrink-0" onClick={() => onChange({ ...action, headers: headers.filter((_, j) => j !== i) })}>
                  Remove
                </Button>
              </div>
            ))}
            <Button size="sm" className="self-start" onClick={() => onChange({ ...action, headers: [...headers, { name: "", value: "" }] })}>
              Add header
            </Button>
          </div>
        </Field>
        {action.method !== "GET" && (
          <Field label="Body" compact>
            <TextArea
              aria-label="Body"
              rows={4}
              className="font-mono text-xs"
              value={action.body ?? ""}
              onChange={(e) => onChange({ ...action, body: e.target.value === "" ? null : e.target.value })}
            />
          </Field>
        )}
      </Disclosure>
    </>
  );
}

function NavigateForm({ action, onChange }: FormProps<"navigate">) {
  const current = useCurrent();
  const profiles = useProfiles();
  const pages = current?.profile.pages ?? [];
  const { to } = action;
  const toTarget = (type: NavTarget["type"]): NavTarget => {
    switch (type) {
      case "nextPage":
      case "prevPage":
        return { type };
      case "page":
        return { type, pageId: pages[0]?.id ?? "" };
      case "profile":
        return { type, profileId: profiles[0]?.id ?? "" };
    }
  };
  return (
    <>
      <Field label="Go to" compact>
        <EnumSelect ariaLabel="Go to" value={to.type} labels={NAV_LABELS} onChange={(type) => onChange({ ...action, to: toTarget(type) })} />
      </Field>
      {to.type === "page" && (
        <Field label="Page" compact>
          <Select
            ariaLabel="Page"
            value={to.pageId}
            options={pages.map((p) => ({ value: p.id, label: p.name || "Untitled page" }))}
            placeholder={to.pageId ? "Page was deleted" : "Choose a page"}
            onChange={(pageId) => onChange({ ...action, to: { type: "page", pageId } })}
          />
        </Field>
      )}
      {to.type === "profile" && (
        <Field label="Layout" compact>
          <Select
            ariaLabel="Layout"
            value={to.profileId}
            options={profiles.map((p) => ({ value: p.id, label: p.name }))}
            placeholder={to.profileId ? "Layout was deleted" : "Choose a layout"}
            onChange={(profileId) => onChange({ ...action, to: { type: "profile", profileId } })}
          />
        </Field>
      )}
    </>
  );
}

function MultiForm({ action, onChange, depth }: FormProps<"multi">) {
  // Keep identity off the saved action. Replacements inherit it; moves keep the object.
  const identities = useRef(new WeakMap<Action, string>());
  const identityOf = (step: Action) => {
    let id = identities.current.get(step);
    if (!id) identities.current.set(step, (id = newId()));
    return id;
  };
  const steps = action.steps;
  const setSteps = (next: Action[]) => onChange({ ...action, steps: next });
  return (
    <div className="flex flex-col pt-1">
      <ol className="flex flex-col">
        {steps.map((step, i) => (
          <li key={identityOf(step)} className="border-t border-line py-1">
            <div className="flex h-7 items-center gap-0.5">
              <span className="mr-auto font-medium">{i + 1}.</span>
              <IconButton icon="arrow-up" label="Move up" disabled={i === 0} onClick={() => setSteps(moveItem(steps, i, i - 1))} />
              <IconButton icon="arrow-down" label="Move down" disabled={i === steps.length - 1} onClick={() => setSteps(moveItem(steps, i, i + 1))} />
              <Button size="sm" variant="ghost" onClick={() => setSteps(steps.filter((_, j) => j !== i))}>
                Remove
              </Button>
            </div>
            <ActionEditor action={step} depth={depth + 1} onChange={(next) => {
              identities.current.set(next, identityOf(step));
              setSteps(steps.map((s, j) => (j === i ? next : s)));
            }} />
          </li>
        ))}
      </ol>
      <div className="flex flex-wrap items-center gap-2 border-t border-line pt-2">
        <Button size="sm" onClick={() => setSteps([...steps, defaultAction("hotkey")])}>
          Add step
        </Button>
        <span className="ml-auto">Wait between steps</span>
        <Seconds ariaLabel="Wait between steps" ms={action.delayMs} step={0.05} onChange={(delayMs) => onChange({ ...action, delayMs })} />
      </div>
    </div>
  );
}

function ToggleForm({ action, onChange, depth }: FormProps<"toggle">) {
  return (
    <div className="flex flex-col pt-1">
      <div className="border-t border-line py-1">
        <p className="flex h-7 items-center font-medium">First press</p>
        <ActionEditor action={action.on} depth={depth + 1} onChange={(on) => onChange({ ...action, on })} />
      </div>
      <div className="border-t border-line py-1">
        <p className="flex h-7 items-center font-medium">Second press</p>
        <ActionEditor action={action.off} depth={depth + 1} onChange={(off) => onChange({ ...action, off })} />
      </div>
      <p className="border-t border-line pt-2 text-xs text-muted">Presses alternate. The button shows its While on look after the first press.</p>
    </div>
  );
}
