// Typed editor for every `Action` variant. Nested actions (multi steps, toggle
// branches) reuse <ActionEditor /> recursively.
import type { Action, HaOp, JsonValue, NavTarget, ObsOp, VoiceOp, VolumeOp } from "@omni/protocol";
import { useId, useState, type ReactNode } from "react";
import { HotkeyInput } from "../../components/HotkeyInput";
import { I } from "../../components/Icon";
import { ObsInputField, VolumeTargetPicker } from "../../components/VolumeTargetPicker";
import {
  Button,
  ComboInput,
  EnumSelect,
  Field,
  IconButton,
  NumberField,
  Range,
  Select,
  Switch,
  TextArea,
  TextInput,
  inputClass,
} from "../../components/ui";
import {
  ACTION_META,
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
} from "../../lib/actions";
import { api, dialog } from "../../lib/backend";
import { useResource } from "../../lib/hooks";
import { useApp, useProfiles, useVoicePresets } from "../../lib/store";
import { moveItem } from "../../lib/util";
import { useCurrent } from "./editor";

type FormProps<T extends Action["type"]> = { action: ActionOf<T>; onChange: (action: Action) => void; depth: number };

const GROUPS = [...new Set(ACTION_TYPES.map((t) => ACTION_META[t].group))];

export function ActionEditor({
  action,
  onChange,
  allowNone = true,
  depth = 0,
}: {
  action: Action | null;
  onChange: (action: Action | null) => void;
  allowNone?: boolean;
  depth?: number;
}) {
  const id = useId();
  return (
    <div className="flex flex-col gap-1">
      <Field label="Action" htmlFor={id} compact>
        <select
          id={id}
          className={inputClass}
          value={action?.type ?? ""}
          onChange={(e) => {
            const type = e.target.value;
            if (isActionType(type)) onChange(defaultAction(type));
            else if (allowNone) onChange(null);
          }}
        >
          {allowNone && <option value="">No action</option>}
          {GROUPS.map((group) => (
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
      {action && <ActionForm action={action} onChange={onChange} depth={depth} />}
    </div>
  );
}

function ActionForm({ action, onChange, depth }: { action: Action; onChange: (action: Action) => void; depth: number }) {
  switch (action.type) {
    case "hotkey":
      return <HotkeyForm action={action} onChange={onChange} depth={depth} />;
    case "typeText":
      return <TypeTextForm action={action} onChange={onChange} depth={depth} />;
    case "openApp":
      return <OpenAppForm action={action} onChange={onChange} depth={depth} />;
    case "open":
      return <OpenForm action={action} onChange={onChange} depth={depth} />;
    case "runScript":
      return <ScriptForm action={action} onChange={onChange} depth={depth} />;
    case "media":
      return (
        <Field label="Key" compact>
          <EnumSelect ariaLabel="Media key" value={action.command} labels={MEDIA_LABELS} onChange={(command) => onChange({ ...action, command })} />
        </Field>
      );
    case "volume":
      return <VolumeForm action={action} onChange={onChange} depth={depth} />;
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
        <Field label="Command" compact>
          <EnumSelect ariaLabel="System command" value={action.command} labels={SYSTEM_LABELS} onChange={(command) => onChange({ ...action, command })} />
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
          <NumberField ariaLabel="Delay" value={action.ms} min={0} max={600_000} suffix="ms" onChange={(ms) => onChange({ ...action, ms })} />
        </Field>
      );
  }
}

function HotkeyForm({ action, onChange }: FormProps<"hotkey">) {
  return (
    <Field label="Keys" compact hint="Modifiers: CmdOrCtrl, Ctrl, Alt, Shift, Super.">
      <HotkeyInput editable value={action.keys} onChange={(keys) => onChange({ ...action, keys })} />
    </Field>
  );
}

function TypeTextForm({ action, onChange }: FormProps<"typeText">) {
  return (
    <Field label="Text" compact>
      <TextArea aria-label="Text to type" rows={3} value={action.text} onChange={(e) => onChange({ ...action, text: e.target.value })} />
    </Field>
  );
}

function OpenAppForm({ action, onChange }: FormProps<"openApp">) {
  const apps = useResource(() => api.installedApps());
  const [args, setArgs] = useState(action.args.join("\n"));
  return (
    <>
      <Field label="App" compact>
        <ComboInput
          ariaLabel="Application"
          value={action.app}
          placeholder="Name, path or desktop entry"
          options={(apps.data ?? []).map((a) => ({ value: a.launch, label: a.name }))}
          onChange={(app) => onChange({ ...action, app })}
        />
      </Field>
      <Field label="Arguments" compact hint="One per line.">
        <TextArea
          aria-label="Arguments"
          rows={2}
          className="font-mono text-xs"
          value={args}
          onChange={(e) => {
            setArgs(e.target.value);
            onChange({ ...action, args: e.target.value.split("\n").filter((a) => a.trim() !== "") });
          }}
        />
      </Field>
    </>
  );
}

function OpenForm({ action, onChange }: FormProps<"open">) {
  const browse = async (directory: boolean) => {
    const path = await dialog.open({ directory });
    if (path) onChange({ ...action, target: path });
  };
  return (
    <Field label="Target" compact hint="URL, file or folder. Opens with the default app.">
      <div className="flex gap-1">
        <TextInput aria-label="Target" value={action.target} onChange={(e) => onChange({ ...action, target: e.target.value })} />
        <IconButton icon="file" label="Choose file" className="size-8" onClick={() => void browse(false)} />
        <IconButton icon="folder-open" label="Choose folder" className="size-8" onClick={() => void browse(true)} />
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
          ariaLabel="Interpreter"
          value={action.interpreter}
          labels={INTERPRETER_LABELS}
          only={interpreters}
          onChange={(interpreter) => onChange({ ...action, interpreter })}
        />
      </Field>
      <TextArea
        aria-label="Script"
        rows={8}
        placeholder={action.interpreter === "applescript" ? 'tell application "Music" to playpause' : "echo hello"}
        className="mt-1 font-mono text-xs leading-relaxed"
        value={action.script}
        onChange={(e) => onChange({ ...action, script: e.target.value })}
      />
      <Field label="Timeout" compact>
        <NumberField ariaLabel="Timeout" value={action.timeoutSecs} min={1} max={3600} suffix="s" onChange={(timeoutSecs) => onChange({ ...action, timeoutSecs })} />
      </Field>
      <Field label="Output" compact>
        <label className="flex h-8 items-center gap-2">
          <Switch label="Show output on the deck" checked={action.showOutput} onChange={(showOutput) => onChange({ ...action, showOutput })} />
          Show on the deck
        </label>
      </Field>
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
      <Field label="Target" compact>
        <VolumeTargetPicker value={action.target} onChange={(target) => onChange({ ...action, target })} />
      </Field>
      <Field label="Do" compact>
        <EnumSelect ariaLabel="Volume operation" value={op.type} labels={VOLUME_OP_LABELS} onChange={(type) => onChange({ ...action, op: setOp(type) })} />
      </Field>
      {op.type === "set" && (
        <Field label="Volume" compact>
          <Range
            label="Volume"
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
        <Field label="Change" compact>
          <Range
            label="Change"
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

function VoiceForm({ action, onChange }: FormProps<"voice">) {
  const presets = useVoicePresets();
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
        <EnumSelect ariaLabel="Voice operation" value={op.type} labels={VOICE_OP_LABELS} onChange={(type) => onChange({ ...action, op: toOp(type) })} />
      </Field>
      {op.type === "setPreset" && (
        <Field label="Preset" compact>
          <Select
            ariaLabel="Preset"
            value={op.presetId}
            options={presets.map((p) => ({ value: p.id, label: p.name }))}
            onChange={(presetId) => onChange({ ...action, op: { type: "setPreset", presetId } })}
          />
        </Field>
      )}
      {op.type === "cycle" && (
        <Field label="Presets" compact hint={op.presetIds.length ? `${op.presetIds.length} selected` : "None selected cycles through all."}>
          <div className="grid max-h-40 grid-cols-2 gap-x-2 overflow-y-auto rounded-ui-sm border border-line px-2 py-1">
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
    </>
  );
}

function ObsForm({ action, onChange }: FormProps<"obs">) {
  const inventory = useResource(() => api.obsInventory());
  const { op } = action;
  const scenes = (inventory.data?.scenes ?? []).map((s) => ({ value: s, label: s }));
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
  const offline = inventory.error && (op.type === "setScene" || op.type === "toggleSource");
  return (
    <>
      <Field label="Do" compact>
        <EnumSelect ariaLabel="OBS operation" value={op.type} labels={OBS_OP_LABELS} onChange={(type) => set(toOp(type))} />
      </Field>
      {(op.type === "setScene" || op.type === "toggleSource") && (
        <Field label="Scene" compact>
          <ComboInput
            ariaLabel="Scene"
            value={op.scene}
            options={scenes}
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
            options={(inventory.data?.sceneItems.find((i) => i.scene === op.scene)?.sources ?? []).map((s) => ({ value: s, label: s }))}
            onChange={(source) => set({ ...op, source })}
          />
        </Field>
      )}
      {op.type === "toggleMute" && (
        <Field label="Input" compact>
          <ObsInputField value={op.input} onChange={(input) => set({ ...op, input })} />
        </Field>
      )}
      {offline && <p className="pl-[100px] text-xs text-muted">OBS is offline. Type names by hand.</p>}
    </>
  );
}

const HA_OP_LABELS: Record<HaOp["type"], string> = { toggle: "Toggle entity", callService: "Call service" };

function HomeAssistantForm({ action, onChange }: FormProps<"homeAssistant">) {
  const entities = useResource(() => api.haEntities());
  const options = (entities.data ?? []).map((e) => ({ value: e.entityId, label: `${e.name} (${e.state})` }));
  const { op } = action;
  const set = (next: HaOp) => onChange({ ...action, op: next });
  return (
    <>
      <Field label="Do" compact>
        <EnumSelect
          ariaLabel="Home Assistant operation"
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
        <Field label="Entity" compact>
          <ComboInput ariaLabel="Entity" value={op.entityId} options={options} placeholder="light.desk" onChange={(entityId) => set({ ...op, entityId })} />
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
          <Field label="Entity" compact>
            <ComboInput
              ariaLabel="Entity"
              value={op.entityId ?? ""}
              options={options}
              placeholder="Optional"
              onChange={(entityId) => set({ ...op, entityId: entityId || null })}
            />
          </Field>
          <Field label="Data" compact>
            <JsonObjectField value={op.data} onChange={(data) => set({ ...op, data })} />
          </Field>
        </>
      )}
      {entities.error && <p className="pl-[100px] text-xs text-muted">{entities.error}. Type the entity id.</p>}
    </>
  );
}

const isJsonObject = (value: JsonValue): value is { [key in string]: JsonValue } =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** JSON object editor: commits only valid objects (or empty = null). */
function JsonObjectField({ value, onChange }: { value: JsonValue | null; onChange: (value: JsonValue | null) => void }) {
  const [text, setText] = useState(value === null ? "" : JSON.stringify(value, null, 2));
  const [error, setError] = useState<string | null>(null);
  return (
    <>
      <TextArea
        aria-label="Service data"
        aria-invalid={error !== null}
        rows={4}
        placeholder={'{ "brightness_pct": 40 }'}
        className={`font-mono text-xs ${error ? "border-danger" : ""}`}
        value={text}
        onChange={(e) => {
          const next = e.target.value;
          setText(next);
          if (next.trim() === "") {
            setError(null);
            return onChange(null);
          }
          try {
            const parsed: JsonValue = JSON.parse(next);
            if (!isJsonObject(parsed)) return setError("Must be a JSON object");
            setError(null);
            onChange(parsed);
          } catch {
            setError("Invalid JSON");
          }
        }}
      />
      {error && <p className="mt-1 text-xs text-danger">{error}</p>}
    </>
  );
}

function HttpForm({ action, onChange }: FormProps<"http">) {
  const headers = action.headers;
  const setHeader = (i: number, patch: Partial<{ name: string; value: string }>) =>
    onChange({ ...action, headers: headers.map((h, j) => (j === i ? { ...h, ...patch } : h)) });
  return (
    <>
      <Field label="Request" compact>
        <div className="flex gap-1">
          <EnumSelect ariaLabel="Method" className="w-24 shrink-0" value={action.method} labels={HTTP_METHODS} onChange={(method) => onChange({ ...action, method })} />
          <TextInput aria-label="URL" value={action.url} placeholder="https://" onChange={(e) => onChange({ ...action, url: e.target.value })} />
        </div>
      </Field>
      <Field label="Headers" compact>
        <div className="flex flex-col gap-1">
          {headers.map((h, i) => (
            <div key={i} className="flex gap-1">
              <TextInput aria-label="Header name" placeholder="Name" value={h.name} onChange={(e) => setHeader(i, { name: e.target.value })} />
              <TextInput aria-label="Header value" placeholder="Value" value={h.value} onChange={(e) => setHeader(i, { value: e.target.value })} />
              <IconButton icon="x" label="Remove header" className="size-8" onClick={() => onChange({ ...action, headers: headers.filter((_, j) => j !== i) })} />
            </div>
          ))}
          <Button size="sm" icon="plus" className="self-start" onClick={() => onChange({ ...action, headers: [...headers, { name: "", value: "" }] })}>
            Header
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
        <EnumSelect ariaLabel="Navigation" value={to.type} labels={NAV_LABELS} onChange={(type) => onChange({ ...action, to: toTarget(type) })} />
      </Field>
      {to.type === "page" && (
        <Field label="Page" compact>
          <Select
            ariaLabel="Page"
            value={to.pageId}
            options={pages.map((p) => ({ value: p.id, label: p.name || "Untitled" }))}
            placeholder="Missing page"
            onChange={(pageId) => onChange({ ...action, to: { type: "page", pageId } })}
          />
        </Field>
      )}
      {to.type === "profile" && (
        <Field label="Profile" compact>
          <Select
            ariaLabel="Profile"
            value={to.profileId}
            options={profiles.map((p) => ({ value: p.id, label: p.name }))}
            placeholder="Missing profile"
            onChange={(profileId) => onChange({ ...action, to: { type: "profile", profileId } })}
          />
        </Field>
      )}
    </>
  );
}

/** Collapsible box around a nested action. */
function Nested({ title, summary, children, actions }: { title: string; summary: string; children: ReactNode; actions?: ReactNode }) {
  return (
    <details className="group rounded-ui-sm border border-line bg-bg" open>
      <summary className="flex h-8 items-center gap-1.5 pr-1 pl-2">
        <I name="chevron-right" className="size-3.5 text-muted group-open:rotate-90" />
        <span className="font-medium">{title}</span>
        <span className="min-w-0 flex-1 truncate text-xs text-muted">{summary}</span>
        {actions}
      </summary>
      <div className="border-t border-line px-2 py-2">{children}</div>
    </details>
  );
}

function MultiForm({ action, onChange, depth }: FormProps<"multi">) {
  const presets = useVoicePresets();
  const names = { preset: (id: string) => presets.find((p) => p.id === id)?.name };
  const steps = action.steps;
  const setSteps = (next: Action[]) => onChange({ ...action, steps: next });
  const stop = (fn: () => void) => (e: { preventDefault: () => void }) => {
    e.preventDefault();
    fn();
  };
  return (
    <div className="flex flex-col gap-1.5 pt-1">
      {steps.map((step, i) => (
        <Nested
          key={i}
          title={`${i + 1}.`}
          summary={describeAction(step, names)}
          actions={
            <>
              <IconButton icon="arrow-up" label="Move up" disabled={i === 0} onClick={stop(() => setSteps(moveItem(steps, i, i - 1)))} />
              <IconButton icon="arrow-down" label="Move down" disabled={i === steps.length - 1} onClick={stop(() => setSteps(moveItem(steps, i, i + 1)))} />
              <IconButton icon="x" label="Remove step" onClick={stop(() => setSteps(steps.filter((_, j) => j !== i)))} />
            </>
          }
        >
          <ActionEditor
            action={step}
            allowNone={false}
            depth={depth + 1}
            onChange={(next) => next && setSteps(steps.map((s, j) => (j === i ? next : s)))}
          />
        </Nested>
      ))}
      <div className="flex items-center gap-2">
        <Button size="sm" icon="plus" onClick={() => setSteps([...steps, defaultAction("hotkey")])}>
          Step
        </Button>
        <span className="ml-auto text-muted">Pause</span>
        <NumberField
          ariaLabel="Pause between steps"
          className="w-28"
          value={action.delayMs}
          min={0}
          max={60_000}
          suffix="ms"
          onChange={(delayMs) => onChange({ ...action, delayMs })}
        />
      </div>
    </div>
  );
}

function ToggleForm({ action, onChange, depth }: FormProps<"toggle">) {
  const presets = useVoicePresets();
  const names = { preset: (id: string) => presets.find((p) => p.id === id)?.name };
  return (
    <div className="flex flex-col gap-1.5 pt-1">
      <Nested title="On" summary={describeAction(action.on, names)}>
        <ActionEditor action={action.on} allowNone={false} depth={depth + 1} onChange={(on) => on && onChange({ ...action, on })} />
      </Nested>
      <Nested title="Off" summary={describeAction(action.off, names)}>
        <ActionEditor action={action.off} allowNone={false} depth={depth + 1} onChange={(off) => off && onChange({ ...action, off })} />
      </Nested>
      <p className="text-xs text-muted">Presses alternate between On and Off. The button shows its active style while on.</p>
    </div>
  );
}
