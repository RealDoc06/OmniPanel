// Buttons screen (layout editor): the open layout's name and menu in the header, page tabs,
// the live deck canvas and the inspector. Layouts are switched from the sidebar.
// Internally a layout is still a `Profile`.
import type { Page, Profile } from "@omni/protocol";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { MenuButton, useContextMenu, type MenuItem } from "../../components/ContextMenu";
import { EmptyState, IconButton, TextInput } from "../../components/ui";
import { confirm } from "../../lib/confirm";
import { navigate, useApp, useAppTheme, useIsMac, useProfiles, useThemes } from "../../lib/store";
import { findTheme } from "../../lib/theme";
import { cx } from "../../lib/util";
import { Canvas } from "./Canvas";
import {
  addPage,
  buttonCount,
  copySelected,
  createProfile,
  cutSelected,
  deletePage,
  deleteSelected,
  duplicateProfile,
  duplicateSelected,
  exportProfile,
  importProfile,
  movePage,
  moveSelected,
  openPage,
  paste,
  redo,
  removeProfile,
  renamePage,
  select,
  selectAll,
  undo,
  useCanUndo,
  useCurrent,
  useEditor,
} from "./editor";
import { Inspector, LAYOUT_NAME_ID } from "./Inspector";

/** "Pixel 4a", "Pixel 4a and Kitchen iPad", "a, b and c". */
const names = new Intl.ListFormat("en-GB", { type: "conjunction" });

/** Names of the devices showing `profileId`, directly or as the layout for new devices. */
function useShownOn(profileId: string): string[] {
  const devices = useApp((s) => s.config.devices);
  const defaultId = useApp((s) => s.config.settings.defaultProfileId);
  return useMemo(
    () => devices.filter((d) => (d.profileId ?? defaultId) === profileId).map((d) => d.name),
    [devices, defaultId, profileId],
  );
}

export function ProfilesScreen() {
  const current = useCurrent();
  useEditorShortcuts();
  if (!current) return <EmptyState icon="layout-grid" title="No layouts" />;
  const { profile, page } = current;
  return (
    <>
      <LayoutHeader profile={profile} />
      <div className="flex min-h-0 flex-1">
        <div className="flex min-w-0 flex-1 flex-col">
          <PageBar profile={profile} page={page} />
          <DeckCanvas profile={profile} page={page} />
        </div>
        <aside className="@container w-[clamp(272px,30vw,320px)] min-w-0 shrink-0 overflow-y-auto border-l border-line" aria-label="Inspector">
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

/** Layout name, its menu, which devices show it, undo and redo. */
function LayoutHeader({ profile }: { profile: Profile }) {
  const profiles = useProfiles();
  const history = useCanUndo();
  const mac = useIsMac();
  const shownOn = useShownOn(profile.id);
  const noDevices = useApp((s) => s.config.devices.length === 0);

  const remove = async () => {
    const { config } = useApp.getState();
    const others = config.profiles.filter((p) => p.id !== profile.id);
    const fallback = others.find((p) => p.id === config.settings.defaultProfileId) ?? others[0];
    const ok = await confirm({
      title: `Delete ${profile.name}?`,
      body: shownOn.length
        ? `${names.format(shownOn)} ${shownOn.length === 1 ? "shows" : "show"} this layout and will switch to ${fallback?.name ?? "another layout"}. This can't be undone.`
        : "Its pages and buttons are deleted. This can't be undone.",
      confirmLabel: "Delete",
      danger: true,
    });
    if (ok) void removeProfile(profile.id);
  };

  // Rename lives in the inspector's Layout panel: clear the selection so it shows, then focus it.
  const rename = () => {
    select([]);
    requestAnimationFrame(() => {
      const field = document.getElementById(LAYOUT_NAME_ID);
      const group = field?.closest("details");
      if (group) group.open = true;
      field?.focus();
    });
  };

  const items: MenuItem[] = [
    { label: "Rename", icon: "pencil", run: rename },
    { label: "New layout", icon: "plus", run: () => createProfile() },
    { label: "Duplicate", icon: "copy", run: duplicateProfile },
    { label: "Import from file…", icon: "download", run: () => void importProfile() },
    { label: "Export to file…", icon: "upload", run: () => void exportProfile(profile) },
    { label: "Delete layout…", icon: "trash-2", danger: true, disabled: profiles.length <= 1, run: () => void remove() },
  ];

  return (
    <header data-tauri-drag-region className="flex h-12 shrink-0 items-center gap-1 border-b border-line pr-4 pl-7">
      <h1 className="truncate text-[16px] font-semibold tracking-[-0.01em]">{profile.name}</h1>
      <MenuButton icon="ellipsis" label="Layout options" items={items} />
      <button
        type="button"
        onClick={() => navigate("decks", noDevices ? "connect" : null)}
        className="ml-auto truncate px-2 text-xs text-muted hover:text-fg"
      >
        {noDevices ? "Connect a phone or tablet to use these buttons" : shownOn.length ? `On ${names.format(shownOn)}` : "Not on any device"}
      </button>
      <IconButton icon="undo-2" label={`${history.undo ? `Undo: ${history.undo}` : "Undo"} (${mac ? "⌘Z" : "Ctrl+Z"})`} disabled={!history.undo} onClick={undo} />
      <IconButton icon="redo-2" label={`${history.redo ? `Redo: ${history.redo}` : "Redo"} (${mac ? "⇧⌘Z" : "Ctrl+Shift+Z"})`} disabled={!history.redo} onClick={redo} />
    </header>
  );
}

/** Page tabs, "Add page" and the page menu (also on right-click of a tab). */
function PageBar({ profile, page }: { profile: Profile; page: Page }) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const menu = useContextMenu();
  const tabs = useRef<HTMLDivElement>(null);
  const [overflow, setOverflow] = useState(false);

  useLayoutEffect(() => {
    const el = tabs.current;
    if (!el) return;
    const measure = () => setOverflow(el.scrollWidth > el.clientWidth);
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    measure();
    return () => observer.disconnect();
  }, [profile.pages, renaming]);

  const remove = async (target: Page) => {
    if (target.controls.length > 0) {
      const ok = await confirm({
        title: `Delete ${target.name || "this page"}?`,
        body: `Its ${buttonCount(target.controls.length)} ${target.controls.length === 1 ? "is" : "are"} deleted too. Undo brings them back.`,
        confirmLabel: "Delete",
        danger: true,
      });
      if (!ok) return;
    }
    deletePage(target.id);
  };

  const pageMenu = (target: Page): MenuItem[] => {
    const index = profile.pages.findIndex((p) => p.id === target.id);
    return [
      { label: "Rename page", icon: "pencil", run: () => setRenaming(target.id) },
      { label: "Move left", icon: "chevron-left", disabled: index <= 0, run: () => movePage(target.id, -1) },
      { label: "Move right", icon: "chevron-right", disabled: index >= profile.pages.length - 1, run: () => movePage(target.id, 1) },
      { label: "Delete page…", icon: "trash-2", danger: true, disabled: profile.pages.length <= 1, run: () => void remove(target) },
    ];
  };

  const index = profile.pages.findIndex((p) => p.id === page.id);
  return (
    <div className="flex h-10 shrink-0 items-center gap-1 pr-3 pl-5">
      <div className="relative min-w-0 flex-1">
        {/* Too many tabs for the width: show "Page 2 of 5" with arrows instead. */}
        {overflow && (
          <div className="flex items-center gap-1">
            <IconButton icon="chevron-left" label="Previous page" disabled={index <= 0} onClick={() => openPage(profile.pages[index - 1]?.id ?? page.id)} />
            {renaming === page.id ? (
              <TextInput
                autoFocus
                aria-label="Page name"
                className="h-7 w-32"
                defaultValue={page.name}
                onBlur={(e) => {
                  const name = e.target.value.trim();
                  if (name && name !== page.name) renamePage(page.id, name);
                  setRenaming(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Escape") e.currentTarget.value = page.name;
                  if (e.key === "Enter" || e.key === "Escape") e.currentTarget.blur();
                }}
              />
            ) : (
              <span className="truncate">
                {page.name || "Untitled page"} <span className="text-muted">({index + 1} of {profile.pages.length})</span>
              </span>
            )}
            <IconButton
              icon="chevron-right"
              label="Next page"
              disabled={index >= profile.pages.length - 1}
              onClick={() => openPage(profile.pages[index + 1]?.id ?? page.id)}
            />
          </div>
        )}
        <div ref={tabs} role="tablist" aria-label="Pages" aria-hidden={overflow || undefined} className={cx("flex w-full items-center gap-0.5 overflow-hidden", overflow && "invisible absolute inset-x-0 top-0")}>
          {profile.pages.map((p, i) =>
            !overflow && renaming === p.id ? (
              <TextInput
                key={p.id}
                autoFocus
                aria-label="Page name"
                className="h-7 w-32"
                defaultValue={p.name}
                onBlur={(e) => {
                  const name = e.target.value.trim();
                  if (name && name !== p.name) renamePage(p.id, name);
                  setRenaming(null);
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
                onDoubleClick={() => setRenaming(p.id)}
                onContextMenu={(e) => {
                  if (p.id !== page.id) openPage(p.id);
                  menu.open(e, pageMenu(p));
                }}
                onKeyDown={(e) => {
                  if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
                    const next = profile.pages[i + (e.key === "ArrowRight" ? 1 : -1)];
                    if (next) openPage(next.id);
                  }
                  if (e.key === "F2") setRenaming(p.id);
                }}
                title="Double-click to rename"
                className={cx(
                  "h-7 shrink-0 rounded-ui-sm px-2.5 whitespace-nowrap",
                  p.id === page.id ? "bg-hover text-fg" : "text-muted hover:text-fg",
                )}
              >
                {p.name || "Untitled page"}
              </button>
            ),
          )}
        </div>
      </div>
      <IconButton icon="plus" label="Add page" onClick={addPage} />
      <MenuButton icon="ellipsis" label="Page options" items={() => pageMenu(page)} />
      {menu.menu}
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
  // Pastes into the targeted empty square, if any.
  v: () => paste(useEditor.getState().target ?? undefined),
  d: duplicateSelected,
  a: selectAll,
};

const ARROWS: Record<string, [number, number] | undefined> = {
  ArrowLeft: [-1, 0],
  ArrowRight: [1, 0],
  ArrowUp: [0, -1],
  ArrowDown: [0, 1],
};

/** Editor keyboard shortcuts, active while the Buttons screen is mounted. */
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
        // Clears the selection and the empty-square target.
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
