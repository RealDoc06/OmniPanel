// Profile editor: profile toolbar, page tabs, live deck canvas and the inspector.
import type { Page, Profile } from "@omni/protocol";
import { useEffect, useMemo, useState } from "react";
import { EmptyState, IconButton, ScreenHeader, Select, TextInput } from "../../components/ui";
import { confirm } from "../../lib/confirm";
import { useApp, useAppTheme, useProfiles, useThemes } from "../../lib/store";
import { findTheme } from "../../lib/theme";
import { cx } from "../../lib/util";
import { Canvas } from "./Canvas";
import {
  addPage,
  copySelected,
  createProfile,
  cutSelected,
  deletePage,
  deleteSelected,
  duplicateProfile,
  duplicateSelected,
  editProfile,
  exportProfile,
  importProfile,
  movePage,
  moveSelected,
  openPage,
  openProfile,
  paste,
  redo,
  removeProfile,
  renamePage,
  select,
  selectAll,
  undo,
  useCanUndo,
  useCurrent,
} from "./editor";
import { Inspector } from "./Inspector";

export function ProfilesScreen() {
  const current = useCurrent();
  useEditorShortcuts();
  if (!current) return <EmptyState icon="layout-grid" title="No profiles" />;
  const { profile, page } = current;
  return (
    <>
      <ScreenHeader title="Profiles">
        <ProfileToolbar profile={profile} />
      </ScreenHeader>
      <div className="flex min-h-0 flex-1">
        <div className="flex min-w-0 flex-1 flex-col">
          <PageBar profile={profile} page={page} />
          <DeckCanvas profile={profile} page={page} />
        </div>
        <aside className="w-[348px] shrink-0 overflow-y-auto border-l border-line bg-surface" aria-label="Inspector">
          <Inspector profile={profile} page={page} />
        </aside>
      </div>
    </>
  );
}

function DeckCanvas({ profile, page }: { profile: Profile; page: Page }) {
  const themes = useThemes();
  const appTheme = useAppTheme();
  const theme = useMemo(() => (profile.themeId ? findTheme(themes, profile.themeId) : appTheme), [themes, profile.themeId, appTheme]);
  return <Canvas profile={profile} page={page} theme={theme} />;
}

function ProfileToolbar({ profile }: { profile: Profile }) {
  const profiles = useProfiles();
  const [renaming, setRenaming] = useState(false);
  const history = useCanUndo();

  const remove = async () => {
    const devices = useApp.getState().config.devices.filter((d) => d.profileId === profile.id).length;
    const ok = await confirm({
      title: `Delete ${profile.name}?`,
      body: devices
        ? `${devices} deck(s) use this profile and will switch to the default profile.`
        : "Its pages and controls are deleted. This can't be undone.",
      confirmLabel: "Delete",
      danger: true,
    });
    if (ok) void removeProfile(profile.id);
  };

  return (
    <div className="flex items-center gap-1">
      {renaming ? (
        <TextInput
          autoFocus
          aria-label="Profile name"
          className="w-48"
          defaultValue={profile.name}
          onBlur={(e) => {
            const name = e.target.value.trim();
            if (name && name !== profile.name) editProfile((p) => ({ ...p, name }));
            setRenaming(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            if (e.key === "Escape") {
              e.currentTarget.value = profile.name;
              e.currentTarget.blur();
            }
          }}
        />
      ) : (
        <Select
          ariaLabel="Profile"
          className="w-48"
          value={profile.id}
          options={profiles.map((p) => ({ value: p.id, label: p.name }))}
          onChange={openProfile}
        />
      )}
      <IconButton icon="pencil" label="Rename profile" onClick={() => setRenaming(true)} />
      <IconButton icon="plus" label="New profile" onClick={createProfile} />
      <IconButton icon="copy" label="Duplicate profile" onClick={duplicateProfile} />
      <IconButton icon="download" label="Import profile" onClick={() => void importProfile()} />
      <IconButton icon="upload" label="Export profile" onClick={() => void exportProfile(profile)} />
      <IconButton icon="trash-2" label="Delete profile" tone="danger" disabled={profiles.length <= 1} onClick={remove} />
      <span className="mx-1.5 h-5 w-px bg-line" />
      <IconButton icon="undo-2" label="Undo" disabled={!history.undo} onClick={undo} />
      <IconButton icon="redo-2" label="Redo" disabled={!history.redo} onClick={redo} />
    </div>
  );
}

function PageBar({ profile, page }: { profile: Profile; page: Page }) {
  const [renaming, setRenaming] = useState(false);
  const index = profile.pages.findIndex((p) => p.id === page.id);

  const remove = async () => {
    if (page.controls.length > 0) {
      const ok = await confirm({
        title: `Delete ${page.name || "this page"}?`,
        body: `Its ${page.controls.length} control(s) are deleted too. Undo brings them back.`,
        confirmLabel: "Delete",
        danger: true,
      });
      if (!ok) return;
    }
    deletePage(page.id);
  };

  return (
    <div className="flex h-11 shrink-0 items-center gap-3 border-b border-line px-4">
      <div role="tablist" aria-label="Pages" className="flex min-w-0 items-center gap-0.5 overflow-x-auto">
        {profile.pages.map((p) =>
          renaming && p.id === page.id ? (
            <TextInput
              key={p.id}
              autoFocus
              aria-label="Page name"
              className="h-7 w-32"
              defaultValue={p.name}
              onBlur={(e) => {
                const name = e.target.value.trim();
                if (name && name !== p.name) renamePage(p.id, name);
                setRenaming(false);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") e.currentTarget.blur();
                if (e.key === "Escape") {
                  e.currentTarget.value = p.name;
                  e.currentTarget.blur();
                }
              }}
            />
          ) : (
            <button
              key={p.id}
              type="button"
              role="tab"
              aria-selected={p.id === page.id}
              onClick={() => openPage(p.id)}
              onDoubleClick={() => setRenaming(true)}
              onKeyDown={(e) => {
                if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
                  const next = profile.pages[index + (e.key === "ArrowRight" ? 1 : -1)];
                  if (next) openPage(next.id);
                }
                if (e.key === "F2") setRenaming(true);
              }}
              title="Double-click to rename"
              className={cx(
                "h-7 shrink-0 rounded-ui-sm px-3 whitespace-nowrap",
                p.id === page.id ? "bg-hover font-medium text-fg" : "text-muted hover:text-fg",
              )}
            >
              {p.name || "Untitled"}
              <span className="ml-1.5 text-xs text-muted">{p.controls.length || ""}</span>
            </button>
          ),
        )}
      </div>
      <div className="flex shrink-0 items-center">
        <IconButton icon="plus" label="Add page" onClick={addPage} />
        <IconButton icon="pencil" label="Rename page" onClick={() => setRenaming(true)} />
        <IconButton icon="chevron-left" label="Move page left" disabled={index <= 0} onClick={() => movePage(page.id, -1)} />
        <IconButton
          icon="chevron-right"
          label="Move page right"
          disabled={index >= profile.pages.length - 1}
          onClick={() => movePage(page.id, 1)}
        />
        <IconButton icon="trash-2" label="Delete page" tone="danger" disabled={profile.pages.length <= 1} onClick={remove} />
      </div>
    </div>
  );
}

const isEditable = (target: EventTarget | null) =>
  target instanceof HTMLElement &&
  (target.isContentEditable || target.closest("input, textarea, select, [contenteditable]") !== null);

/** Focus is on the page itself or inside the deck canvas. */
const inCanvas = (target: EventTarget | null) =>
  target === document.body || (target instanceof Element && target.closest("[role=grid]") !== null);

const MOD_SHORTCUTS: Record<string, ((shift: boolean) => void) | undefined> = {
  z: (shift) => (shift ? redo() : undo()),
  y: redo,
  c: copySelected,
  x: cutSelected,
  v: () => paste(),
  d: duplicateSelected,
  a: selectAll,
};

const ARROWS: Record<string, [number, number] | undefined> = {
  ArrowLeft: [-1, 0],
  ArrowRight: [1, 0],
  ArrowUp: [0, -1],
  ArrowDown: [0, 1],
};

/** Editor keyboard shortcuts, active while the Profiles screen is mounted. */
function useEditorShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || isEditable(e.target) || document.querySelector("dialog[open], [role=menu]")) return;
      if (e.metaKey || e.ctrlKey) {
        const run = e.altKey ? undefined : MOD_SHORTCUTS[e.key.toLowerCase()];
        if (run) {
          e.preventDefault();
          run(e.shiftKey);
        }
        return;
      }
      if (!inCanvas(e.target)) return;
      if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        deleteSelected();
      } else if (e.key === "Escape") {
        select([]);
      } else {
        const step = ARROWS[e.key];
        if (!step) return;
        e.preventDefault();
        moveSelected(...step);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
