// Right panel: everything about the selected control (look, action or slider target,
// global hotkey, test, AI assistant). Multi-selection gets bulk actions only.
import type { Control, GeneratedControl, Icon, LabelPosition, Page, Profile, Style, Theme } from "@omni/protocol";
import { useMemo, useState, type ReactNode } from "react";
import { HotkeyInput } from "../../components/HotkeyInput";
import { ControlIcon, I } from "../../components/Icon";
import { VolumeTargetPicker } from "../../components/VolumeTargetPicker";
import { Button, ColorField, Field, IconButton, Kbd, Notice, Segmented, Select, Switch, TextInput } from "../../components/ui";
import { normalizeAccelerator } from "../../lib/accelerator";
import { api } from "../../lib/backend";
import { iconMarkup } from "../../lib/icons";
import { useApp, useAppTheme, useIsMac, useProfiles, useThemes } from "../../lib/store";
import { findTheme } from "../../lib/theme";
import { toast, toastError } from "../../lib/toast";
import { ActionEditor } from "./ActionEditor";
import { AiAssist } from "./AiAssist";
import { deleteSelected, duplicateSelected, editProfile, resizeControl, setGrid, updateControl, useEditor } from "./editor";
import { IconPicker } from "./IconPicker";

export function Inspector({ profile, page }: { profile: Profile; page: Page }) {
  const selected = useEditor((s) => s.selected);
  const controls = page.controls.filter((c) => selected.includes(c.id));
  const [control] = controls;
  if (!control) return <ProfilePanel profile={profile} />;
  if (controls.length > 1) return <MultiSelection count={controls.length} />;
  return <ControlInspector key={control.id} control={control} profile={profile} />;
}

/** Mirrors omni_core::MAX_GRID. */
const MAX_GRID = 12;
const GRID_SIZES = Array.from({ length: MAX_GRID }, (_, i) => ({ value: String(i + 1), label: String(i + 1) }));

/** Shown with no selection: profile-wide settings and the editor shortcuts. */
function ProfilePanel({ profile }: { profile: Profile }) {
  const themes = useThemes();
  const devices = useApp((s) => s.config.devices);
  const defaultId = useApp((s) => s.config.settings.defaultProfileId);
  const users = devices.filter((d) => (d.profileId ?? defaultId) === profile.id).map((d) => d.name);
  const mod = useIsMac() ? "⌘" : "Ctrl+";
  const rows: [string, string][] = [
    ["Click empty cell", "Add a button"],
    ["Drag", "Move or swap"],
    ["Arrows", "Nudge selection"],
    ["Shift+click", "Select several"],
    ["Right-click", "More actions"],
    [`${mod}C / ${mod}V`, "Copy / paste"],
    [`${mod}D`, "Duplicate"],
    ["Delete", "Remove"],
    [`${mod}Z`, "Undo"],
  ];
  return (
    <div>
      <Group title="Profile">
        <Field label="Grid" compact hint="Columns × rows">
          <div className="flex items-center gap-1.5">
            <Select
              ariaLabel="Columns"
              className="w-[72px]"
              value={String(profile.cols)}
              options={GRID_SIZES}
              onChange={(v) => setGrid(profile.rows, Number(v))}
            />
            <span className="text-muted">×</span>
            <Select ariaLabel="Rows" className="w-[72px]" value={String(profile.rows)} options={GRID_SIZES} onChange={(v) => setGrid(Number(v), profile.cols)} />
          </div>
        </Field>
        <Field label="Deck theme" compact>
          <Select
            ariaLabel="Deck theme"
            value={profile.themeId ?? ""}
            options={[{ value: "", label: "Same as the app" }, ...themes.map((t) => ({ value: t.id, label: t.name }))]}
            onChange={(id) => editProfile((p) => ({ ...p, themeId: id || null }))}
          />
        </Field>
        <Field label="Used by" compact>
          <p className="pt-[7px] text-muted">{users.length ? users.join(", ") : "No deck yet"}</p>
        </Field>
      </Group>
      <div className="flex flex-col gap-3 p-4">
        <p className="text-fg">Select a control to edit it, or drag one from the bar under the deck.</p>
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 text-xs">
          {rows.map(([key, what]) => (
            <div key={key} className="contents">
              <dt>
                <Kbd>{key}</Kbd>
              </dt>
              <dd className="text-muted">{what}</dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  );
}

function MultiSelection({ count }: { count: number }) {
  return (
    <div className="flex flex-col gap-3 p-4">
      <p className="font-semibold">{count} controls selected</p>
      <div className="flex gap-2">
        <Button icon="copy-plus" onClick={duplicateSelected}>
          Duplicate
        </Button>
        <Button icon="trash-2" variant="danger" onClick={deleteSelected}>
          Delete
        </Button>
      </div>
      <p className="text-xs text-muted">Drag or use the arrow keys to move them together.</p>
    </div>
  );
}

/** Collapsible inspector group. */
function Group({ title, children, open = true, aside }: { title: string; children: ReactNode; open?: boolean; aside?: ReactNode }) {
  return (
    <details open={open} className="group border-b border-line">
      <summary className="flex h-9 items-center gap-1.5 px-4 font-semibold hover:bg-hover">
        <I name="chevron-right" className="size-3.5 text-muted group-open:rotate-90" />
        {title}
        <span className="ml-auto text-xs font-normal text-muted">{aside}</span>
      </summary>
      <div className="flex flex-col gap-0.5 px-4 pt-0.5 pb-3">{children}</div>
    </details>
  );
}

const LABEL_POSITIONS: { value: LabelPosition; label: string }[] = [
  { value: "bottom", label: "Bottom" },
  { value: "top", label: "Top" },
  { value: "center", label: "Center" },
  { value: "hidden", label: "Hidden" },
];

function ControlInspector({ control, profile }: { control: Control; profile: Profile }) {
  const themes = useThemes();
  const appTheme = useAppTheme();
  const theme: Theme = profile.themeId ? findTheme(themes, profile.themeId) : appTheme;
  const previewActive = useEditor((s) => s.previewActive);
  const { style, kind } = control;

  const update = (recipe: (c: Control) => Control, key?: string) => updateControl(control.id, recipe, key);
  const setStyle = (patch: Partial<Style>, key?: string) => update((c) => ({ ...c, style: { ...c.style, ...patch } }), key);

  const setKind = (type: Control["kind"]["type"]) => {
    if (type === kind.type) return;
    update((c) => {
      if (c.kind.type === "button") {
        const target = c.kind.action?.type === "volume" ? c.kind.action.target : { type: "master" as const };
        return { ...c, kind: { type: "slider", target, vertical: c.h >= c.w } };
      }
      return { ...c, kind: { type: "button", action: { type: "volume", target: c.kind.target, op: { type: "toggleMute" } } } };
    });
  };

  const test = async () => {
    if (kind.type !== "button" || !kind.action) return;
    try {
      const outcome = await api.runAction(kind.action);
      toast(outcome.message ?? `Ran ${style.label || "action"}`, "success");
    } catch (error) {
      toastError(error);
    }
  };

  const applyAi = (result: GeneratedControl) =>
    update((c) => ({
      ...c,
      style: {
        ...c.style,
        label: result.label,
        icon: result.icon && iconMarkup(result.icon) ? { type: "builtin", name: result.icon } : c.style.icon,
      },
      kind: { type: "button", action: result.action },
    }));

  const sizes = (max: number) => Array.from({ length: max }, (_, i) => ({ value: String(i + 1), label: String(i + 1) }));

  return (
    <div>
      <div className="flex items-center gap-1.5 border-b border-line px-4 py-2.5">
        <Segmented
          label="Control type"
          value={kind.type}
          onChange={setKind}
          options={[
            { value: "button", label: "Button", icon: "square" },
            { value: "slider", label: "Slider", icon: "sliders-vertical" },
          ]}
        />
        <div className="ml-auto flex items-center gap-0.5">
          {kind.type === "button" && (
            <Button size="sm" icon="play" disabled={!kind.action} onClick={() => void test()}>
              Test
            </Button>
          )}
          <IconButton icon="copy-plus" label="Duplicate" onClick={duplicateSelected} />
          <IconButton icon="trash-2" label="Delete" tone="danger" onClick={deleteSelected} />
        </div>
      </div>

      <Group title="Appearance">
        <Field label="Label" compact>
          <TextInput aria-label="Label" value={style.label} placeholder="No label" onChange={(e) => setStyle({ label: e.target.value }, "label")} />
        </Field>
        <Field label="Icon" compact>
          <IconChooser value={style.icon} onChange={(icon) => setStyle({ icon })} />
        </Field>
        <Field label="Label at" compact>
          <Segmented label="Label position" value={style.labelPosition} options={LABEL_POSITIONS} onChange={(labelPosition) => setStyle({ labelPosition })} />
        </Field>
        <Field label="Background" compact>
          <ColorField label="Background" value={style.bg} fallback={theme.colors.button} onChange={(bg) => setStyle({ bg }, "bg")} />
        </Field>
        <Field label="Text" compact>
          <ColorField label="Text color" value={style.fg} fallback={theme.colors.buttonText} onChange={(fg) => setStyle({ fg }, "fg")} />
        </Field>
        <Field label="Size" compact>
          <div className="flex items-center gap-1.5">
            <Select
              ariaLabel="Width"
              className="w-[72px]"
              value={String(control.w)}
              options={sizes(profile.cols - control.x)}
              onChange={(w) => resizeControl(control.id, Number(w), control.h)}
            />
            <span className="text-muted">×</span>
            <Select
              ariaLabel="Height"
              className="w-[72px]"
              value={String(control.h)}
              options={sizes(profile.rows - control.y)}
              onChange={(h) => resizeControl(control.id, control.w, Number(h))}
            />
            <span className="text-xs text-muted">cells</span>
          </div>
        </Field>
      </Group>

      <Group title="Active state" open={style.activeBg !== null || style.activeLabel !== null || style.activeIcon !== null}>
        <Field label="Preview" compact>
          <label className="flex h-8 items-center gap-2">
            <Switch label="Preview active state" checked={previewActive} onChange={(on) => useEditor.setState({ previewActive: on })} />
            <span className="text-muted">Show on the deck preview</span>
          </label>
        </Field>
        <Field label="Background" compact>
          <ColorField label="Active background" value={style.activeBg} fallback={theme.colors.accent} onChange={(activeBg) => setStyle({ activeBg }, "activeBg")} />
        </Field>
        <Field label="Label" compact>
          <TextInput
            aria-label="Active label"
            value={style.activeLabel ?? ""}
            placeholder={style.label || "Same label"}
            onChange={(e) => setStyle({ activeLabel: e.target.value === "" ? null : e.target.value }, "activeLabel")}
          />
        </Field>
        <Field label="Icon" compact>
          <IconChooser value={style.activeIcon} onChange={(activeIcon) => setStyle({ activeIcon })} />
        </Field>
      </Group>

      {kind.type === "button" ? (
        <Group title="Action">
          <ActionEditor action={kind.action} onChange={(action) => update((c) => ({ ...c, kind: { type: "button", action } }), "action")} />
        </Group>
      ) : (
        <Group title="Slider">
          <Field label="Controls" compact>
            <VolumeTargetPicker value={kind.target} onChange={(target) => update((c) => ({ ...c, kind: { ...kind, target } }), "target")} />
          </Field>
          <Field label="Direction" compact>
            <Segmented
              label="Direction"
              value={kind.vertical ? "vertical" : "horizontal"}
              onChange={(v) => update((c) => ({ ...c, kind: { ...kind, vertical: v === "vertical" } }))}
              options={[
                { value: "vertical", label: "Vertical" },
                { value: "horizontal", label: "Horizontal" },
              ]}
            />
          </Field>
        </Group>
      )}

      <Group title="Global hotkey" open={control.hotkey !== null}>
        <HotkeySection control={control} onChange={(hotkey) => update((c) => ({ ...c, hotkey }))} />
      </Group>

      <Group title="Describe it" aside="AI">
        <AiAssist control={control} onApply={applyAi} />
      </Group>
    </div>
  );
}

function HotkeySection({ control, onChange }: { control: Control; onChange: (hotkey: string | null) => void }) {
  const profiles = useProfiles();
  const conflicts = useMemo(() => {
    if (!control.hotkey) return [];
    const key = normalizeAccelerator(control.hotkey);
    return profiles.flatMap((p) =>
      p.pages.flatMap((page) =>
        page.controls
          .filter((c) => c.id !== control.id && c.hotkey && normalizeAccelerator(c.hotkey) === key)
          .map((c) => `${c.style.label || "Untitled"} (${p.name} / ${page.name})`),
      ),
    );
  }, [profiles, control.id, control.hotkey]);
  return (
    <>
      <HotkeyInput value={control.hotkey ?? ""} onChange={(v) => onChange(v || null)} />
      {conflicts.length > 0 && <Notice tone="warn">Also used by {conflicts.join(", ")}.</Notice>}
      <p className="text-xs text-muted">Triggers this control from anywhere while OmniPanel runs.</p>
    </>
  );
}

function IconChooser({ value, onChange }: { value: Icon | null; onChange: (icon: Icon | null) => void }) {
  const [open, setOpen] = useState(false);
  const name = value ? (value.type === "builtin" ? value.name : value.type === "emoji" ? "Emoji" : "Image") : "None";
  return (
    <div className="flex items-center gap-2">
      <button
        type="button"
        aria-label="Change icon"
        onClick={() => setOpen(true)}
        className="flex size-8 shrink-0 items-center justify-center rounded-ui-sm border border-line bg-bg hover:bg-hover"
      >
        {value ? <ControlIcon icon={value} size="18px" /> : <I name="image" className="size-4 text-muted" />}
      </button>
      <span className="min-w-0 flex-1 truncate text-xs text-muted">{name}</span>
      <Button size="sm" onClick={() => setOpen(true)}>
        Change
      </Button>
      {value && <IconButton icon="x" label="Remove icon" onClick={() => onChange(null)} />}
      {open && (
        <IconPicker
          value={value}
          onClose={() => setOpen(false)}
          onPick={(icon) => {
            onChange(icon);
            setOpen(false);
          }}
        />
      )}
    </div>
  );
}
