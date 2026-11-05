// Right panel of the Buttons screen. Nothing selected: the layout's name, size and theme,
// plus the editor shortcuts. Empty square targeted: the action picker for a new button. One
// control: header and actions, What it does, Look, the live ("While …") look and its
// keyboard shortcut. Several: bulk actions only.
import type { Control, Icon, LabelPosition, Page, Profile, Style, Theme } from "@omni/protocol";
import { useMemo, useState } from "react";
import { DeckControlView } from "../../components/DeckControl";
import { HotkeyInput } from "../../components/HotkeyInput";
import { ControlIcon, I } from "../../components/Icon";
import { Button, ColorField, Field, Group, IconButton, Kbd, Notice, Segmented, Select, Switch, TextInput } from "../../components/ui";
import { formatAccelerator, normalizeAccelerator } from "../../lib/accelerator";
import { describeAction, liveStateTitle, type NameLookup } from "../../lib/actions";
import { api } from "../../lib/backend";
import { confirm } from "../../lib/confirm";
import { newControlAt } from "../../lib/controls";
import { MAX_GRID } from "../../lib/grid";
import { useAppTheme, useIsMac, useProfiles, useThemes, useVoicePresets } from "../../lib/store";
import { findTheme, themeStyle } from "../../lib/theme";
import { toast, toastError } from "../../lib/toast";
import { ActionPicker } from "./ActionPicker";
import { WhatItDoes } from "./ActionEditor";
import {
  addControl,
  clearTarget,
  deleteSelected,
  duplicateSelected,
  editProfile,
  navigateEditor,
  planGrid,
  resizeControl,
  setGrid,
  updateControl,
  useEditor,
} from "./editor";
import { IconPicker } from "./IconPicker";

export function Inspector({ profile, page }: { profile: Profile; page: Page }) {
  const selected = useEditor((s) => s.selected);
  const target = useEditor((s) => s.target);
  const controls = page.controls.filter((c) => selected.includes(c.id));
  const [control] = controls;
  if (target) return <NewControl key={`${target.x}:${target.y}`} target={target} profile={profile} page={page} />;
  if (!control) return <ProfilePanel profile={profile} />;
  if (controls.length > 1) return <MultiSelection count={controls.length} />;
  return <ControlInspector key={control.id} control={control} profile={profile} />;
}

const sizeOptions = (max: number) => Array.from({ length: max }, (_, i) => ({ value: String(i + 1), label: String(i + 1) }));

/** "[w] across [h] down" pair of selects. */
function SizeFields({
  w,
  h,
  maxW,
  maxH,
  onChange,
}: {
  w: number;
  h: number;
  maxW: number;
  maxH: number;
  onChange: (w: number, h: number) => void;
}) {
  return (
    <div className="flex items-center gap-1.5">
      <Select ariaLabel="Across" className="w-16 pr-2" value={String(w)} options={sizeOptions(maxW)} onChange={(v) => onChange(Number(v), h)} />
      <span className="text-xs">across</span>
      <Select ariaLabel="Down" className="w-16 pr-2" value={String(h)} options={sizeOptions(maxH)} onChange={(v) => onChange(w, Number(v))} />
      <span className="text-xs">down</span>
    </div>
  );
}

/** Resizes the open layout's grid, confirming first when buttons would be removed. */
async function resizeGrid(rows: number, cols: number) {
  const removed = planGrid(rows, cols);
  if (removed > 0) {
    const ok = await confirm({
      title: "Make the grid smaller?",
      body: `${removed === 1 ? "1 button doesn't" : `${removed} buttons don't`} fit and will be removed. Undo brings them back.`,
      confirmLabel: `Remove ${removed}`,
      danger: true,
    });
    if (!ok) return;
  }
  setGrid(rows, cols);
}

/** Id of the layout name field, so the header's Rename can focus it. */
export const LAYOUT_NAME_ID = "layout-name";

/** Shown with nothing selected: layout-wide settings and the editor shortcuts. */
function ProfilePanel({ profile }: { profile: Profile }) {
  const themes = useThemes();
  const mod = useIsMac() ? "⌘" : "Ctrl+";
  const rows: [string, string][] = [
    ["Click an empty square", "Add a button"],
    ["Drag", "Move or swap"],
    ["Arrow keys", "Move the selection one square"],
    ["Shift+click", "Select several"],
    ["Right-click", "More options"],
    [`${mod}C / ${mod}V`, "Copy / paste"],
    [`${mod}D`, "Duplicate"],
    ["Delete", "Remove"],
    [`${mod}Z`, "Undo"],
  ];
  return (
    <div>
      <Group title="Layout">
        <Field label="Name" compact htmlFor={LAYOUT_NAME_ID}>
          {/* Saved on blur or Enter, never empty; Escape puts the saved name back. Remounts when the
              saved name changes from elsewhere (undo, redo). */}
          <TextInput
            key={profile.name}
            id={LAYOUT_NAME_ID}
            defaultValue={profile.name}
            onBlur={(e) => {
              const name = e.target.value.trim();
              if (name && name !== profile.name) editProfile((p) => ({ ...p, name }));
              else e.target.value = profile.name;
            }}
            onKeyDown={(e) => {
              if (e.key === "Escape") e.currentTarget.value = profile.name;
              if (e.key === "Enter" || e.key === "Escape") e.currentTarget.blur();
            }}
          />
        </Field>
        <Field label="Size" compact>
          <SizeFields
            w={profile.cols}
            h={profile.rows}
            maxW={MAX_GRID}
            maxH={MAX_GRID}
            onChange={(cols, rows) => void resizeGrid(rows, cols)}
          />
        </Field>
        <Field label="Theme" compact>
          <Select
            ariaLabel="Theme"
            value={profile.themeId ?? ""}
            options={[{ value: "", label: "Same as the app" }, ...themes.map((t) => ({ value: t.id, label: t.name }))]}
            onChange={(id) => editProfile((p) => ({ ...p, themeId: id || null }))}
          />
        </Field>
      </Group>
      <Group title="Keyboard shortcuts" open={false}>
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 text-xs">
          {rows.map(([key, what]) => (
            <div key={key} className="contents">
              <dt>
                <Kbd>{key}</Kbd>
              </dt>
              <dd>{what}</dd>
            </div>
          ))}
        </dl>
      </Group>
    </div>
  );
}

/** An empty square is targeted: pick what the new control does. Nothing exists until a pick. */
function NewControl({ target, profile, page }: { target: { x: number; y: number }; profile: Profile; page: Page }) {
  return (
    <div>
      <div className="flex h-12 items-center gap-2 border-b border-line px-5">
        <p className="text-[14px] font-semibold">New button</p>
        <Button size="sm" variant="ghost" className="ml-auto" onClick={clearTarget}>
          Cancel
        </Button>
      </div>
      <ActionPicker
        current={null}
        onPick={(r) => addControl(newControlAt(target.x, target.y, r, page, profile))}
        onCancel={clearTarget}
      />
    </div>
  );
}

function MultiSelection({ count }: { count: number }) {
  return (
    <div className="flex flex-col gap-3 p-5">
      <p className="text-[14px] font-semibold">{count} selected</p>
      <div className="flex gap-2">
        <Button size="sm" onClick={duplicateSelected}>
          Duplicate
        </Button>
        <Button size="sm" variant="danger" onClick={deleteSelected}>
          Delete
        </Button>
      </div>
      <p>Drag or use the arrow keys to move them together.</p>
    </div>
  );
}

const LABEL_POSITIONS: { value: LabelPosition; label: string }[] = [
  { value: "bottom", label: "Bottom" },
  { value: "top", label: "Top" },
  { value: "center", label: "Middle" },
  { value: "hidden", label: "Hidden" },
];

/** Page, layout and voice names for action summaries. */
function useNames(profile: Profile): NameLookup {
  const profiles = useProfiles();
  const presets = useVoicePresets();
  return useMemo<NameLookup>(
    () => ({
      page: (id) => profile.pages.find((p) => p.id === id)?.name,
      profile: (id) => profiles.find((p) => p.id === id)?.name,
      preset: (id) => presets.find((p) => p.id === id)?.name,
    }),
    [profile.pages, profiles, presets],
  );
}

function ControlInspector({ control, profile }: { control: Control; profile: Profile }) {
  const themes = useThemes();
  const appTheme = useAppTheme();
  const theme: Theme = profile.themeId ? findTheme(themes, profile.themeId) : appTheme;
  const previewActive = useEditor((s) => s.previewActive);
  const names = useNames(profile);
  const { style, kind } = control;

  const update = (recipe: (c: Control) => Control, key?: string) => updateControl(control.id, recipe, key);
  const setStyle = (patch: Partial<Style>, key?: string) => update((c) => ({ ...c, style: { ...c.style, ...patch } }), key);

  const summary = kind.type === "slider" ? "Volume slider" : describeAction(kind.action, names);
  const hasLiveLook = style.activeBg !== null || style.activeLabel !== null || style.activeIcon !== null;
  const liveTitle = liveStateTitle(kind) ?? (hasLiveLook ? "While on" : null);

  const runNow = async () => {
    if (kind.type !== "button" || !kind.action) return;
    try {
      let message: string | null = null;
      if (kind.action.type === "navigate") navigateEditor(kind.action.to);
      else ({ message } = await api.runAction(kind.action));
      toast(message ?? `Done: ${style.label || summary}`, "success");
    } catch (error) {
      toastError(error);
    }
  };

  return (
    <div>
      <div className="flex items-center gap-3 border-b border-line px-5 py-4">
        <div className="size-12 shrink-0" style={themeStyle(theme)}>
          <DeckControlView control={control} active={previewActive} truncateLabel />
        </div>
        <div className="min-w-0 flex-1">
          <p className="truncate text-[14px] font-semibold">{style.label || (kind.type === "slider" ? "Slider" : "Button")}</p>
          <p className="truncate text-xs text-muted">{summary}</p>
        </div>
        <div className="flex shrink-0">
          {kind.type === "button" && <IconButton icon="play" label="Run now" disabled={!kind.action} onClick={() => void runNow()} />}
          <IconButton icon="copy" label="Duplicate" onClick={duplicateSelected} />
          <IconButton icon="trash-2" label="Delete" tone="danger" onClick={deleteSelected} />
        </div>
      </div>

      <WhatItDoes control={control} onChange={(next) => update((c) => ({ ...c, ...next }), "action")} />

      <Group title="Look">
        <Field label="Label" compact>
          <TextInput
            aria-label="Label"
            value={style.label}
            placeholder="Text on the button"
            onChange={(e) => setStyle({ label: e.target.value }, "label")}
          />
        </Field>
        <Field label="Icon" compact>
          <IconChooser value={style.icon} onChange={(icon) => setStyle({ icon })} />
        </Field>
        <div className="flex flex-col gap-1 py-1 [&>[role=radiogroup]]:w-full [&_[role=radio]]:min-w-0 [&_[role=radio]]:flex-1 [&_[role=radio]]:justify-center [&_[role=radio]]:px-1">
          <span className="text-muted">Label position</span>
          <Segmented label="Label position" value={style.labelPosition} options={LABEL_POSITIONS} onChange={(labelPosition) => setStyle({ labelPosition })} />
        </div>
        <Field label="Background" compact>
          <ColorField label="Background" value={style.bg} fallback={theme.colors.button} onChange={(bg) => setStyle({ bg }, "bg")} />
        </Field>
        <Field label="Label color" compact>
          <ColorField label="Label color" value={style.fg} fallback={theme.colors.buttonText} onChange={(fg) => setStyle({ fg }, "fg")} />
        </Field>
        <Field label="Size" compact>
          <SizeFields
            w={control.w}
            h={control.h}
            maxW={profile.cols - control.x}
            maxH={profile.rows - control.y}
            onChange={(w, h) => resizeControl(control.id, w, h)}
          />
        </Field>
      </Group>

      {liveTitle && (
        <Group title={liveTitle}>
          <Field label="Preview" compact>
            <Switch label={`Preview ${liveTitle.toLowerCase()}`} checked={previewActive} onChange={(on) => useEditor.setState({ previewActive: on })} />
          </Field>
          <Field label="Background" compact>
            <ColorField
              label={`Background ${liveTitle.toLowerCase()}`}
              value={style.activeBg}
              fallback={theme.colors.accent}
              onChange={(activeBg) => setStyle({ activeBg }, "activeBg")}
            />
          </Field>
          <Field label="Label" compact>
            <TextInput
              aria-label={`Label ${liveTitle.toLowerCase()}`}
              value={style.activeLabel ?? ""}
              placeholder={style.label}
              onChange={(e) => setStyle({ activeLabel: e.target.value === "" ? null : e.target.value }, "activeLabel")}
            />
          </Field>
          <Field label="Icon" compact>
            <IconChooser value={style.activeIcon} onChange={(activeIcon) => setStyle({ activeIcon })} />
          </Field>
        </Group>
      )}

      <ShortcutSection control={control} onChange={(hotkey) => update((c) => ({ ...c, hotkey }))} />
    </div>
  );
}

/** Keyboard shortcut that presses this control from any app, with a duplicate warning. */
function ShortcutSection({ control, onChange }: { control: Control; onChange: (hotkey: string | null) => void }) {
  const profiles = useProfiles();
  const mac = useIsMac();
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
    <Group
      title="Keyboard shortcut"
      open={conflicts.length > 0}
      aside={control.hotkey ? formatAccelerator(control.hotkey, mac) : "None"}
    >
      <p className="pb-1">Press these keys in any app to press this button.</p>
      <HotkeyInput variant="trigger" value={control.hotkey ?? ""} onChange={(v) => onChange(v || null)} />
      {conflicts.length > 0 && <Notice tone="warn">Also used by {conflicts.join(", ")}. Only one will work.</Notice>}
    </Group>
  );
}

/** Icon preview, "Choose…" (opens the picker), and "No icon" when one is set. */
function IconChooser({ value, onChange }: { value: Icon | null; onChange: (icon: Icon | null) => void }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="flex items-center gap-1.5">
      <div aria-hidden className="flex size-7 shrink-0 items-center justify-center rounded-ui-sm border border-line">
        {value ? <ControlIcon icon={value} size="18px" /> : <I name="image" className="size-4 text-muted" />}
      </div>
      <Button size="sm" onClick={() => setOpen(true)}>
        Choose…
      </Button>
      {value && (
        <Button size="sm" variant="ghost" onClick={() => onChange(null)}>
          No icon
        </Button>
      )}
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
