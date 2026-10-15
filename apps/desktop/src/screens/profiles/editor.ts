// Profile editor state: current profile/page, selection, per-profile undo history
// and a clipboard that works across pages and profiles. Every edit goes through
// `commit()`, which records history and saves (debounced) through the app store.
import type { Control, Page, Profile } from "@omni/protocol";
import { useMemo } from "react";
import { create } from "zustand";
import { api, dialog } from "../../lib/backend";
import { cloneControl } from "../../lib/controls";
import { bestSize, fitToGrid, moveGroup, moveOrSwap, placeAll } from "../../lib/grid";
import { deleteProfile, run, saveProfile, useApp } from "../../lib/store";
import { toast, toastError } from "../../lib/toast";
import { moveItem, newId, uniqueName } from "../../lib/util";

interface EditorState {
  profileId: string | null;
  pageId: string | null;
  selected: string[];
  /** Draw the selected control in its active state. */
  previewActive: boolean;
}

export const useEditor = create<EditorState>()(() => ({
  profileId: null,
  pageId: null,
  selected: [],
  previewActive: false,
}));

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
  past: Profile[];
  future: Profile[];
  key: string | null;
  at: number;
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
    h.past.push(prev);
    if (h.past.length > LIMIT) h.past.shift();
  }
  h.key = key ?? null;
  h.at = now;
  h.future = [];
  saveProfile(next);
}

function travel(from: "past" | "future") {
  const c = current();
  if (!c) return;
  const h = historyOf(c.profile.id);
  const target = h[from].pop();
  if (!target) return;
  (from === "past" ? h.future : h.past).push(c.profile);
  h.key = null;
  saveProfile(target);
  const ids = new Set(target.pages.flatMap((p) => p.controls.map((ctl) => ctl.id)));
  useEditor.setState((s) => ({ selected: s.selected.filter((id) => ids.has(id)) }));
}

export const undo = () => travel("past");
export const redo = () => travel("future");

export function useCanUndo() {
  const c = useCurrent();
  // Re-evaluated on every profile change, which is when history changes.
  const h = c ? histories.get(c.profile.id) : undefined;
  return { undo: (h?.past.length ?? 0) > 0, redo: (h?.future.length ?? 0) > 0 };
}

// ---------------------------------------------------------------------------
// Selection & navigation

export const select = (ids: string[]) => useEditor.setState({ selected: ids });

export function toggleSelect(id: string) {
  useEditor.setState((s) => ({
    selected: s.selected.includes(id) ? s.selected.filter((x) => x !== id) : [...s.selected, id],
  }));
}

export function selectAll() {
  const c = current();
  if (c) select(c.page.controls.map((ctl) => ctl.id));
}

export const openProfile = (profileId: string) => useEditor.setState({ profileId, pageId: null, selected: [] });
export const openPage = (pageId: string) => useEditor.setState({ pageId, selected: [] });

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
  if (skipped) toast(skipped === source.length ? "No free space on this page" : `${skipped} control(s) didn't fit`, "error");
}

// ---------------------------------------------------------------------------
// Profile & page edits

export function editProfile(recipe: (profile: Profile) => Profile, key?: string) {
  const c = current();
  if (c) commit(recipe(c.profile), key);
}

/** Changes the grid size, fitting every page into it. */
export function setGrid(rows: number, cols: number) {
  let removed = 0;
  editProfile((p) => ({
    ...p,
    rows,
    cols,
    pages: p.pages.map((page) => {
      const fitted = fitToGrid(page.controls, { rows, cols });
      removed += fitted.removed;
      return { ...page, controls: fitted.controls };
    }),
  }));
  if (removed) toast(`${removed} control(s) no longer fit and were removed`, "info", { label: "Undo", run: undo });
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
  if (page) toast(`Deleted ${page.name || "page"}`, "info", { label: "Undo", run: undo });
}

export function createProfile() {
  const profiles = useApp.getState().config.profiles;
  const profile: Profile = {
    id: newId(),
    name: uniqueName("New profile", profiles.map((p) => p.name)),
    rows: 3,
    cols: 5,
    themeId: null,
    pages: [{ id: newId(), name: "Page 1", controls: [] }],
  };
  saveProfile(profile, 0);
  openProfile(profile.id);
}

export function duplicateProfile() {
  const c = current();
  if (!c) return;
  const profiles = useApp.getState().config.profiles;
  const copy: Profile = {
    ...structuredClone(c.profile),
    id: newId(),
    name: uniqueName(`${c.profile.name} copy`, profiles.map((p) => p.name)),
  };
  copy.pages = copy.pages.map((page) => ({ ...page, id: newId(), controls: page.controls.map(cloneControl) }));
  saveProfile(copy, 0);
  openProfile(copy.id);
}

export async function removeProfile(id: string) {
  histories.delete(id);
  const others = useApp.getState().config.profiles.filter((p) => p.id !== id);
  if (others[0]) openProfile(others[0].id);
  await deleteProfile(id);
}

const JSON_FILTER = [{ name: "OmniPanel profile", extensions: ["json"] }];

export async function exportProfile(profile: Profile) {
  try {
    const path = await dialog.save(`${profile.name.replace(/[\\/:*?"<>|]+/g, "-") || "profile"}.json`, JSON_FILTER);
    if (!path) return;
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
