// Grid placement rules for the deck editor: bounds, collisions, moves, swaps,
// resizes and fitting controls into a smaller grid. Pure functions, unit tested.

/** Largest rows or columns count. Mirrors omni_core::MAX_GRID. */
export const MAX_GRID = 12;

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface GridSize {
  rows: number;
  cols: number;
}

type Placed = Rect & { id: string };

export const overlaps = (a: Rect, b: Rect) =>
  a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

export const inBounds = (r: Rect, grid: GridSize) =>
  r.x >= 0 && r.y >= 0 && r.w >= 1 && r.h >= 1 && r.x + r.w <= grid.cols && r.y + r.h <= grid.rows;

/** `rect` is inside the grid and free, ignoring the controls in `ignore`. */
export function fits(rect: Rect, controls: readonly Placed[], grid: GridSize, ignore: ReadonlySet<string> = new Set()) {
  return inBounds(rect, grid) && controls.every((c) => ignore.has(c.id) || !overlaps(c, rect));
}

/** First free top-left position (row-major) for a `w` x `h` control. */
export function findFreeSpot(controls: readonly Placed[], grid: GridSize, w = 1, h = 1): { x: number; y: number } | null {
  for (let y = 0; y + h <= grid.rows; y++) {
    for (let x = 0; x + w <= grid.cols; x++) {
      if (fits({ x, y, w, h }, controls, grid)) return { x, y };
    }
  }
  return null;
}

/** Cells not covered by any control. */
export function emptyCells(controls: readonly Placed[], grid: GridSize): { x: number; y: number }[] {
  const cells: { x: number; y: number }[] = [];
  for (let y = 0; y < grid.rows; y++) {
    for (let x = 0; x < grid.cols; x++) {
      const cell = { x, y, w: 1, h: 1 };
      if (!controls.some((c) => overlaps(c, cell))) cells.push({ x, y });
    }
  }
  return cells;
}

/**
 * Moves one control to `x`,`y`. When the target is taken by exactly one control
 * of the same size, the two swap places. Returns null when the move is invalid.
 */
export function moveOrSwap<T extends Placed>(controls: readonly T[], id: string, x: number, y: number, grid: GridSize): T[] | null {
  const moving = controls.find((c) => c.id === id);
  if (!moving) return null;
  if (moving.x === x && moving.y === y) return null;
  const target = { x, y, w: moving.w, h: moving.h };
  if (!inBounds(target, grid)) return null;
  if (fits(target, controls, grid, new Set([id]))) {
    return controls.map((c) => (c.id === id ? { ...c, x, y } : c));
  }
  const hit = controls.filter((c) => c.id !== id && overlaps(c, target));
  const other = hit[0];
  if (hit.length !== 1 || !other || other.w !== moving.w || other.h !== moving.h) return null;
  if (other.x !== x || other.y !== y) return null;
  const swapped = { x: moving.x, y: moving.y, w: other.w, h: other.h };
  if (!fits(swapped, controls, grid, new Set([id, other.id]))) return null;
  return controls.map((c) =>
    c.id === id ? { ...c, x, y } : c.id === other.id ? { ...c, x: moving.x, y: moving.y } : c,
  );
}

/** Moves a group of controls by a delta. Returns null if any of them would collide or leave the grid. */
export function moveGroup<T extends Placed>(controls: readonly T[], ids: ReadonlySet<string>, dx: number, dy: number, grid: GridSize): T[] | null {
  if ((dx === 0 && dy === 0) || ids.size === 0) return null;
  const moved = controls.map((c) => (ids.has(c.id) ? { ...c, x: c.x + dx, y: c.y + dy } : c));
  const group = moved.filter((c) => ids.has(c.id));
  const others = moved.filter((c) => !ids.has(c.id));
  // Group members keep their relative layout, so they can only collide with the rest.
  const ok = group.every((c) => inBounds(c, grid) && others.every((o) => !overlaps(o, c)));
  return ok ? moved : null;
}

/** Largest size up to `w` x `h` that fits for control `id`, preferring the bigger area. */
export function bestSize(controls: readonly Placed[], id: string, w: number, h: number, grid: GridSize): { w: number; h: number } | null {
  const control = controls.find((c) => c.id === id);
  if (!control) return null;
  const maxW = Math.max(1, Math.min(w, grid.cols - control.x));
  const maxH = Math.max(1, Math.min(h, grid.rows - control.y));
  let best: { w: number; h: number } | null = null;
  for (let cw = maxW; cw >= 1; cw--) {
    for (let ch = maxH; ch >= 1; ch--) {
      if (best && cw * ch <= best.w * best.h) break;
      if (fits({ x: control.x, y: control.y, w: cw, h: ch }, controls, grid, new Set([id]))) best = { w: cw, h: ch };
    }
  }
  return best;
}

/**
 * Places `incoming` controls next to `existing`: each keeps its position when free,
 * otherwise goes to the first free spot (shrunk to 1x1 if needed). Controls that
 * don't fit at all are returned in `skipped`.
 */
export function placeAll<T extends Placed>(existing: readonly T[], incoming: readonly T[], grid: GridSize): { placed: T[]; skipped: number } {
  const placed: T[] = [];
  let skipped = 0;
  for (const c of incoming) {
    const all = [...existing, ...placed];
    const w = Math.min(c.w, grid.cols);
    const h = Math.min(c.h, grid.rows);
    if (fits({ x: c.x, y: c.y, w, h }, all, grid)) {
      placed.push({ ...c, w, h });
      continue;
    }
    const spot = findFreeSpot(all, grid, w, h) ?? findFreeSpot(all, grid, 1, 1);
    if (!spot) {
      skipped++;
      continue;
    }
    const sized = fits({ ...spot, w, h }, all, grid) ? { w, h } : { w: 1, h: 1 };
    placed.push({ ...c, ...spot, ...sized });
  }
  return { placed, skipped };
}

/** Fits a page into a resized grid. Controls that no longer fit anywhere are dropped. */
export function fitToGrid<T extends Placed>(controls: readonly T[], grid: GridSize): { controls: T[]; removed: number } {
  const inside: T[] = [];
  const outside: T[] = [];
  for (const c of controls) {
    const clamped = { ...c, w: Math.min(c.w, grid.cols - c.x), h: Math.min(c.h, grid.rows - c.y) };
    if (c.x < grid.cols && c.y < grid.rows && fits(clamped, inside, grid)) inside.push(clamped);
    else outside.push(c);
  }
  const { placed, skipped } = placeAll(inside, outside, grid);
  return { controls: [...inside, ...placed], removed: skipped };
}
