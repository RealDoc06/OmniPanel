// Layout editor state: current layout (profile) and page, selection, the empty square
// picked for a new control, per-layout undo history and a clipboard that works across
// pages and layouts. Every edit goes through `commit()`, which records history and
// saves (debounced) through the app store.
import type { Control, NavTarget, Page, Profile } from "@omni/protocol";
import { useMemo } from "react";
import { create } from "zustand";
import { api, dialog } from "../../lib/backend";
import { cloneControl, followRenamed } from "../../lib/controls";
import { bestSize, fitToGrid, moveGroup, moveOrSwap, placeAll } from "../../lib/grid";
import { deleteProfile, flushProfile, run, saveProfile, useApp } from "../../lib/store";
import { toast, toastError } from "../../lib/toast";
import { moveItem, newId, uniqueName } from "../../lib/util";

interface EditorState {
  profileId: string | null;
  pageId: string | null;
  selected: string[];
  /** Empty square the action picker adds to. Nothing exists there until an entry is picked. */
  target: { x: number; y: number } | null;
  /** Draw the selected control in its active state. */
  previewActive: boolean;
}

export const useEditor = create<EditorState>()(() => ({
  profileId: null,
  pageId: null,
  selected: [],
  target: null,
  previewActive: false,
}));

/** "1 button", "3 buttons". */
export const buttonCount = (n: number) => `${n} button${n === 1 ? "" : "s"}`;

/** Resolves the edited profile and page (falls back to the first ones). */
function resolve(profiles: Profile[], profileId: string | null, pageId: string | null) {
  const profile = profiles.find((p) => p.id === profileId) ?? profiles[0];
  const page = profile?.pages.find((p) => p.id === pageId) ?? profile?.pages[0];
  return profile && page ? { profile, page } : null;
}

export function current(): { profile: Profile; page: Page } | null {
  const { profileId, pageId } = useEditor.getState();
  return resolve(useApp.getState().config.profiles, profileId, pageId);
}

export function useCurrent() {
  const profiles = useApp((s) => s.config.profiles);
  const profileId = useEditor((s) => s.profileId);
  const pageId = useEditor((s) => s.pageId);
  return useMemo(() => resolve(profiles, profileId, pageId), [profiles, profileId, pageId]);
}

// ---------------------------------------------------------------------------
// History

interface History {
  past: HistoryEntry[];
  future: HistoryEntry[];
  key: string | null;
  at: number;
}

interface HistoryEntry {
  profile: Profile;
  description: string;
}

/** Short names for the next undo/redo, including the layout it belongs to. */
function describeEdit(prev: Profile, next: Profile, key?: string): string {
  const field = key?.split(":")[0];
  const fields: Record<string, string> = {
    label: "change label", action: "change what a button does", bg: "change background",
    fg: "change label color", activeBg: "change active background", activeLabel: "change active label",
    "page-name": "rename page",
  };
  const before = prev.pages.flatMap((p) => p.controls);
  const after = next.pages.flatMap((p) => p.controls);
  const edit = (field && fields[field]) ||
    (prev.name !== next.name ? "rename layout" :
      prev.rows !== next.rows || prev.cols !== next.cols ? "resize grid" :
      prev.pages.length < next.pages.length ? "add page" :
      prev.pages.length > next.pages.length ? "delete page" :
      before.length < after.length ? "add buttons" :
      before.length > after.length ? "delete buttons" : "edit buttons");
  return `${edit} in ${next.name}`;
}

const histories = new Map<string, History>();
const LIMIT = 100;

function historyOf(id: string): History {
  let h = histories.get(id);
  if (!h) histories.set(id, (h = { past: [], future: [], key: null, at: 0 }));
  return h;
}

/**
 * Saves an edited profile and records the previous version for undo.
 * Edits sharing a `key` within 1.5 s (typing a label, dragging a color) merge into one step.
 */
export function commit(next: Profile, key?: string) {
  const prev = useApp.getState().config.profiles.find((p) => p.id === next.id);
  if (!prev || prev === next) return;
  const h = historyOf(next.id);
  const now = Date.now();
  if (!key || h.key !== key || now - h.at > 1500) {
    h.past.push({ profile: prev, description: describeEdit(prev, next, key) });
    if (h.past.length > LIMIT) h.past.shift();
  }
  h.key = key ?? null;
  h.at = now;
  h.future = [];
  saveProfile(next);
}

/** Steps the history of `profileId` (default: the open layout), whichever layout is open. */
function travel(from: "past" | "future", profileId = current()?.profile.id) {
  const profile = useApp.getState().config.profiles.find((p) => p.id === profileId);
  if (!profile) return;
  const h = historyOf(profile.id);
  const target = h[from].pop();
  if (!target) return;
  (from === "past" ? h.future : h.past).push({ profile, description: target.description });
  h.key = null;
  saveProfile(target.profile);
  if (current()?.profile.id !== profile.id) return;
  const ids = new Set(target.profile.pages.flatMap((p) => p.controls.map((ctl) => ctl.id)));
  useEditor.setState((s) => ({ selected: s.selected.filter((id) => ids.has(id)), target: null }));
}

export const undo = () => travel("past");
export const redo = () => travel("future");
/** Undo bound to one layout, for toasts raised while it was open. */
export const undoIn = (profileId: string) => travel("past", profileId);

export function useCanUndo() {
  const c = useCurrent();
  // Re-evaluated on every profile change, which is when history changes.
  const h = c ? histories.get(c.profile.id) : undefined;
  return { undo: h?.past.at(-1)?.description ?? null, redo: h?.future.at(-1)?.description ?? null };
}

// ---------------------------------------------------------------------------
// Selection & navigation

/** Selecting (or clearing the selection) also drops the empty-square target. */
export const select = (ids: string[]) => useEditor.setState({ selected: ids, target: null, previewActive: false });

export function toggleSelect(id: string) {
  useEditor.setState((s) => ({
    selected: s.selected.includes(id) ? s.selected.filter((x) => x !== id) : [...s.selected, id],
    target: null,
    previewActive: false,
  }));
}

/** Targets an empty square: the inspector shows the action picker for it. */
export const pickAt = (x: number, y: number) => useEditor.setState({ target: { x, y }, selected: [], previewActive: false });
export const clearTarget = () => useEditor.setState({ target: null });

export function selectAll() {
  const c = current();
  if (c) select(c.page.controls.map((ctl) => ctl.id));
}

export const openProfile = (profileId: string) => useEditor.setState({ profileId, pageId: null, selected: [], target: null, previewActive: false });
export const openPage = (pageId: string) => useEditor.setState({ pageId, selected: [], target: null, previewActive: false });

/** Run now previews navigation in the editor without changing a device's layout. */
export function navigateEditor(to: NavTarget) {
  const c = current();
  if (!c) return;
  if (to.type === "profile") {
    if (useApp.getState().config.profiles.some((p) => p.id === to.profileId)) openProfile(to.profileId);
    return;
  }
  const index = c.profile.pages.findIndex((p) => p.id === c.page.id);
  const page = to.type === "page"
    ? c.profile.pages.find((p) => p.id === to.pageId)
    : c.profile.pages[index + (to.type === "nextPage" ? 1 : -1)];
  if (page) openPage(page.id);
}

// ---------------------------------------------------------------------------
// Control edits (current page)

type ControlsRecipe = (controls: Control[], profile: Profile) => Control[] | null;

function editControls(recipe: ControlsRecipe, key?: string): boolean {
  const c = current();
  if (!c) return false;
  const controls = recipe(c.page.controls, c.profile);
  if (!controls) return false;
  commit({ ...c.profile, pages: c.profile.pages.map((p) => (p.id === c.page.id ? { ...p, controls } : p)) }, key);
  return true;
}

export function updateControl(id: string, recipe: (control: Control) => Control, key?: string) {
  editControls((controls) => controls.map((ctl) => (ctl.id === id ? recipe(ctl) : ctl)), key && `${key}:${id}`);
}

export function addControl(control: Control) {
  if (editControls((controls) => [...controls, control])) select([control.id]);
}

const selectedSet = () => new Set(useEditor.getState().selected);

export function deleteSelected() {
  const ids = selectedSet();
  if (ids.size === 0) return;
  editControls((controls) => controls.filter((ctl) => !ids.has(ctl.id)));
  select([]);
}

/** Arrow-key nudge of the selection. */
export function moveSelected(dx: number, dy: number) {
  const ids = selectedSet();
  editControls((controls, p) => moveGroup(controls, ids, dx, dy, p));
}

/**
 * Result of dropping control `id` at cell `x`,`y`: a multi-selection moves together,
 * a single control moves or swaps with a same-size one. Null when the drop is invalid.
 */
export function planDrop(controls: Control[], id: string, x: number, y: number, grid: { rows: number; cols: number }) {
  const ids = selectedSet();
  const moving = controls.find((ctl) => ctl.id === id);
  if (!moving) return null;
  if (ids.size > 1 && ids.has(id)) return moveGroup(controls, ids, x - moving.x, y - moving.y, grid);
  return moveOrSwap(controls, id, x, y, grid);
}

export function dropControl(id: string, x: number, y: number) {
  editControls((controls, p) => planDrop(controls, id, x, y, p));
}

export function resizeControl(id: string, w: number, h: number) {
  editControls((controls, p) => {
    const size = bestSize(controls, id, w, h, p);
    return size ? controls.map((ctl) => (ctl.id === id ? { ...ctl, ...size } : ctl)) : null;
  });
}

let clipboard: Control[] = [];

export function copySelected(): number {
  const c = current();
  const ids = selectedSet();
  if (!c || ids.size === 0) return 0;
  clipboard = structuredClone(c.page.controls.filter((ctl) => ids.has(ctl.id)));
  return clipboard.length;
}

export function cutSelected() {
  if (copySelected()) deleteSelected();
}

export const hasClipboard = () => clipboard.length > 0;

/** Pastes the clipboard, keeping positions where free, or anchored at a cell. */
export function paste(at?: { x: number; y: number }) {
  if (clipboard.length === 0) return;
  if (!at) return insertCopies(clipboard);
  const dx = at.x - Math.min(...clipboard.map((ctl) => ctl.x));
  const dy = at.y - Math.min(...clipboard.map((ctl) => ctl.y));
  insertCopies(clipboard.map((ctl) => ({ ...ctl, x: ctl.x + dx, y: ctl.y + dy })));
}

export function duplicateSelected() {
  const c = current();
  const ids = selectedSet();
  if (c) insertCopies(c.page.controls.filter((ctl) => ids.has(ctl.id)));
}

function insertCopies(source: Control[]) {
  let placedIds: string[] = [];
  let skipped = 0;
  editControls((controls, p) => {
    const result = placeAll(controls, source.map(cloneControl), p);
    placedIds = result.placed.map((ctl) => ctl.id);
    skipped = result.skipped;
    return result.placed.length ? [...controls, ...result.placed] : null;
  });
  if (placedIds.length) select(placedIds);
  if (skipped) toast(skipped === source.length ? "No free space on this page" : `${buttonCount(skipped)} didn't fit`, "error");
}

// ---------------------------------------------------------------------------
// Profile & page edits

export function editProfile(recipe: (profile: Profile) => Profile, key?: string) {
  const c = current();
  if (c) commit(recipe(c.profile), key);
}

/** `profile` resized to rows x cols with every page fitted in; `removed` counts what didn't fit. */
function resized(profile: Profile, rows: number, cols: number) {
  let removed = 0;
  const pages = profile.pages.map((page) => {
    const fitted = fitToGrid(page.controls, { rows, cols });
    removed += fitted.removed;
    return { ...page, controls: fitted.controls };
  });
  return { profile: { ...profile, rows, cols, pages }, removed };
}

/** How many controls `setGrid(rows, cols)` would remove from the open layout, so the caller can confirm. */
export function planGrid(rows: number, cols: number): number {
  const c = current();
  return c ? resized(c.profile, rows, cols).removed : 0;
}

/** Changes the grid size, fitting every page into it. Returns how many controls were removed. */
export function setGrid(rows: number, cols: number): number {
  const c = current();
  if (!c) return 0;
  const { profile, removed } = resized(c.profile, rows, cols);
  commit(profile);
  if (removed) toast(`Removed ${buttonCount(removed)}.`, "info", { label: "Undo", run: () => undoIn(profile.id) });
  return removed;
}

export function addPage() {
  const page: Page = { id: newId(), name: "", controls: [] };
  editProfile((p) => ({ ...p, pages: [...p.pages, { ...page, name: uniqueName(`Page ${p.pages.length + 1}`, p.pages.map((x) => x.name)) }] }));
  openPage(page.id);
}

export function renamePage(id: string, name: string) {
  editProfile((p) => ({ ...p, pages: p.pages.map((page) => (page.id === id ? { ...page, name } : page)) }), `page-name:${id}`);
}

export function movePage(id: string, delta: number) {
  editProfile((p) => {
    const from = p.pages.findIndex((page) => page.id === id);
    return { ...p, pages: moveItem(p.pages, from, from + delta) };
  });
}

export function deletePage(id: string) {
  const c = current();
  if (!c || c.profile.pages.length <= 1) return;
  const index = c.profile.pages.findIndex((p) => p.id === id);
  const page = c.profile.pages[index];
  editProfile((p) => ({ ...p, pages: p.pages.filter((x) => x.id !== id) }));
  const neighbour = c.profile.pages[index + 1] ?? c.profile.pages[index - 1];
  if (neighbour) openPage(neighbour.id);
  const profileId = c.profile.id;
  if (page) toast(`Deleted ${page.name || "page"}`, "info", { label: "Undo", run: () => undoIn(profileId) });
}

/** Creates an empty layout, opens it in the editor and returns its id. */
export function createProfile(name = "New layout"): string {
  const profiles = useApp.getState().config.profiles;
  const profile: Profile = {
    id: newId(),
    name: uniqueName(name, profiles.map((p) => p.name)),
    rows: 3,
    cols: 5,
    themeId: null,
    pages: [{ id: newId(), name: "Page 1", controls: [] }],
  };
  saveProfile(profile, 0);
  openProfile(profile.id);
  return profile.id;
}

/** Copies the open layout with fresh ids. Links to its own pages, or to itself, follow the copy. */
export function duplicateProfile() {
  const c = current();
  if (!c) return;
  const profiles = useApp.getState().config.profiles;
  const id = newId();
  const profilesRenamed = new Map([[c.profile.id, id]]);
  const pagesRenamed = new Map(c.profile.pages.map((page) => [page.id, newId()]));
  const relink = (control: Control): Control => {
    const { kind } = control;
    return kind.type === "button" && kind.action ? { ...control, kind: { ...kind, action: followRenamed(kind.action, profilesRenamed, pagesRenamed) } } : control;
  };
  const copy: Profile = {
    ...c.profile,
    id,
    name: uniqueName(`${c.profile.name} copy`, profiles.map((p) => p.name)),
    pages: c.profile.pages.map((page) => ({
      ...page,
      id: pagesRenamed.get(page.id)!,
      controls: page.controls.map((control) => relink(cloneControl(control))),
    })),
  };
  saveProfile(copy, 0);
  openProfile(copy.id);
}

export async function removeProfile(id: string) {
  histories.delete(id);
  const others = useApp.getState().config.profiles.filter((p) => p.id !== id);
  if (others[0]) openProfile(others[0].id);
  await deleteProfile(id);
}

const JSON_FILTER = [{ name: "OmniPanel layout", extensions: ["json"] }];

/** Saves any pending edit of the layout first: the host exports what it has stored (R6-7). */
export async function exportProfile(profile: Profile) {
  try {
    const path = await dialog.save(`${profile.name.replace(/[\\/:*?"<>|]+/g, "-") || "layout"}.json`, JSON_FILTER);
    if (!path) return;
    await flushProfile(profile.id);
    if (await run(api.exportProfile(profile.id, path))) toast(`Exported ${profile.name}`, "success");
  } catch (error) {
    toastError(error);
  }
}

export async function importProfile() {
  try {
    const path = await dialog.open({ filters: JSON_FILTER });
    if (!path) return;
    const profile = await api.importProfile(path);
    openProfile(profile.id);
    toast(`Imported ${profile.name}`, "success");
  } catch (error) {
    toastError(error);
  }
}
