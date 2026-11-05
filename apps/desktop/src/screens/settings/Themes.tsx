// Theme picker (built-in + custom) as a dense list with three swatches per theme,
// "Customize" to make an editable copy, and the custom theme editor with a live preview.
import type { Theme, ThemeColors } from "@omni/protocol";
import { DeckControlView } from "../../components/DeckControl";
import { I } from "../../components/Icon";
import { Button, ColorField, Field, Range, TextInput } from "../../components/ui";
import { confirm } from "../../lib/confirm";
import { newButton } from "../../lib/controls";
import { deleteTheme, patchSettings, saveTheme, useApp, useThemes } from "../../lib/store";
import { COLOR_LABELS, themeStyle } from "../../lib/theme";
import { cx, newId, uniqueName } from "../../lib/util";

export function ThemeSettings() {
  const themes = useThemes();
  const activeId = useApp((s) => s.config.settings.themeId);
  const custom = useApp((s) => s.config.themes);
  const active = themes.find((t) => t.id === activeId);
  const editing = custom.find((t) => t.id === activeId);

  /** Makes an editable copy of the active theme and switches to it. */
  const duplicate = () => {
    if (!active) return;
    const copy: Theme = {
      ...structuredClone(active),
      id: `custom-${newId()}`,
      name: uniqueName(`${active.name} custom`, themes.map((t) => t.name)),
    };
    saveTheme(copy);
    patchSettings({ themeId: copy.id });
  };

  return (
    <div className="flex flex-col gap-2">
      <div role="radiogroup" aria-label="Theme" className="grid grid-cols-[repeat(auto-fill,minmax(168px,1fr))] gap-x-2">
        {themes.map((t) => {
          const on = t.id === activeId;
          return (
            <button
              key={t.id}
              type="button"
              role="radio"
              aria-checked={on}
              onClick={() => patchSettings({ themeId: t.id })}
              className={cx("flex h-8 min-w-0 items-center gap-2 rounded-ui-sm px-1.5 text-left hover:bg-hover", on ? "font-medium text-accent" : "text-fg")}
            >
              <span className="w-3.5 shrink-0">{on && <I name="check" className="size-3.5" />}</span>
              <span className="flex shrink-0 gap-0.5">
                {[t.colors.background, t.colors.button, t.colors.accent].map((c, i) => (
                  <span key={i} className="size-3 rounded-full border border-line" style={{ background: c }} />
                ))}
              </span>
              <span className="truncate">{t.name}</span>
            </button>
          );
        })}
      </div>
      <div className="flex items-center gap-3">
        {active && (
          <Button size="sm" icon="copy" onClick={duplicate}>
            {editing ? "Make a copy" : `Customize ${active.name}`}
          </Button>
        )}
        <span className="text-xs text-muted">Devices use this theme too, unless a layout picks its own.</span>
      </div>
      {editing && <ThemeEditor key={editing.id} theme={editing} />}
    </div>
  );
}

function ThemeEditor({ theme }: { theme: Theme }) {
  const save = (next: Theme) => saveTheme(next, 300);
  const colors: Record<string, string> = theme.colors;
  const setColor = (key: string, value: string) => {
    const next: ThemeColors = { ...theme.colors, [key]: value };
    save({ ...theme, colors: next });
  };
  const remove = async () => {
    if (await confirm({ title: `Delete ${theme.name}?`, confirmLabel: "Delete", danger: true })) void deleteTheme(theme.id);
  };

  return (
    <div className="mt-1 grid grid-cols-1 gap-6 border-t border-line pt-3 lg:grid-cols-[minmax(0,1fr)_300px]">
      <div>
        <Field label="Name">
          <TextInput aria-label="Theme name" className="max-w-xs" value={theme.name} onChange={(e) => save({ ...theme, name: e.target.value })} />
        </Field>
        <Field label="Corner roundness">
          <Range
            label="Corner roundness"
            className="max-w-xs"
            min={0}
            max={24}
            step={1}
            value={theme.radius}
            format={(v) => `${v} px`}
            onChange={(radius) => save({ ...theme, radius })}
          />
        </Field>
        <div className="grid grid-cols-1 gap-x-6 xl:grid-cols-2">
          {Object.entries(COLOR_LABELS).map(([key, label]) => (
            <label key={key} className="flex h-9 items-center justify-between gap-3">
              <span>{label}</span>
              <ColorField label={label} value={colors[key] ?? "#000000"} fallback="#000000" resettable={false} onChange={(v) => v && setColor(key, v)} />
            </label>
          ))}
        </div>
        <Button size="sm" variant="danger" icon="trash-2" className="mt-3" onClick={() => void remove()}>
          Delete theme
        </Button>
      </div>
      <ThemePreview theme={theme} />
    </div>
  );
}

const PREVIEW_CONTROLS = [
  newButton(0, 0, null, { label: "Mic", icon: { type: "builtin", name: "mic" } }),
  newButton(1, 0, null, { label: "Scene", icon: { type: "builtin", name: "clapperboard" } }),
  // Preview only: shows the active state in the theme's danger color.
  newButton(2, 0, null, { label: "Live", icon: { type: "builtin", name: "radio" }, activeBg: "var(--danger)" }),
];

/** Mini app + deck rendered with the edited theme only. */
function ThemePreview({ theme }: { theme: Theme }) {
  return (
    <div style={themeStyle(theme)} className="flex flex-col gap-3 self-start rounded-ui border border-line bg-bg p-3 text-fg" aria-label="Theme preview">
      <div className="rounded-ui border border-line bg-surface p-3">
        <p className="font-semibold">Panel</p>
        <p className="text-muted">Secondary text on a panel.</p>
        <div className="mt-2 flex gap-1.5">
          <span className="rounded-ui-sm bg-accent px-2.5 py-1 text-xs font-medium text-on-accent">Accent</span>
          <span className="rounded-ui-sm border border-line bg-hover px-2.5 py-1 text-xs">Highlight</span>
        </div>
        <div className="mt-2 flex gap-3 text-xs">
          <span className="text-success">Success</span>
          <span className="text-warning">Warning</span>
          <span className="text-danger">Error</span>
        </div>
      </div>
      <div className="grid grid-cols-3 gap-2">
        {PREVIEW_CONTROLS.map((c, i) => (
          <div key={c.id} className="aspect-square">
            <DeckControlView control={c} active={i === 2} />
          </div>
        ))}
      </div>
    </div>
  );
}
