import assert from "node:assert/strict";
import { test } from "node:test";
import { bestSize, emptyCells, findFreeSpot, fitToGrid, fits, moveGroup, moveOrSwap, placeAll } from "./grid.ts";

const grid = { rows: 3, cols: 4 };
const c = (id: string, x: number, y: number, w = 1, h = 1) => ({ id, x, y, w, h });

test("fits respects bounds and collisions", () => {
  const controls = [c("a", 0, 0, 2, 1)];
  assert.equal(fits({ x: 1, y: 0, w: 1, h: 1 }, controls, grid), false);
  assert.equal(fits({ x: 2, y: 0, w: 2, h: 1 }, controls, grid), true);
  assert.equal(fits({ x: 3, y: 0, w: 2, h: 1 }, controls, grid), false);
  assert.equal(fits({ x: 0, y: 0, w: 1, h: 1 }, controls, grid, new Set(["a"])), true);
});

test("findFreeSpot scans row by row", () => {
  const controls = [c("a", 0, 0), c("b", 1, 0, 3, 1)];
  assert.deepEqual(findFreeSpot(controls, grid), { x: 0, y: 1 });
  assert.deepEqual(findFreeSpot(controls, grid, 4, 2), { x: 0, y: 1 });
  assert.equal(findFreeSpot(controls, grid, 4, 3), null);
});

test("emptyCells lists uncovered cells", () => {
  const cells = emptyCells([c("a", 0, 0, 4, 2)], grid);
  assert.equal(cells.length, 4);
  assert.deepEqual(cells[0], { x: 0, y: 2 });
});

test("moveOrSwap moves into free space", () => {
  const moved = moveOrSwap([c("a", 0, 0), c("b", 1, 0)], "a", 2, 2, grid);
  assert.deepEqual(moved?.find((x) => x.id === "a"), c("a", 2, 2));
});

test("moveOrSwap swaps same-size controls", () => {
  const moved = moveOrSwap([c("a", 0, 0), c("b", 3, 2)], "a", 3, 2, grid);
  assert.deepEqual(moved, [c("a", 3, 2), c("b", 0, 0)]);
});

test("moveOrSwap rejects partial overlaps and out of bounds", () => {
  assert.equal(moveOrSwap([c("a", 0, 0, 2, 1), c("b", 2, 1, 2, 1)], "a", 1, 1, grid), null);
  assert.equal(moveOrSwap([c("a", 0, 0, 2, 1)], "a", 3, 0, grid), null);
  assert.equal(moveOrSwap([c("a", 0, 0, 1, 2), c("b", 1, 0)], "a", 1, 0, grid), null);
});

test("moveGroup moves selections together", () => {
  const controls = [c("a", 0, 0), c("b", 1, 0), c("x", 3, 0)];
  const moved = moveGroup(controls, new Set(["a", "b"]), 2, 0, grid);
  assert.equal(moved, null, "b would hit x");
  const down = moveGroup(controls, new Set(["a", "b"]), 0, 1, grid);
  assert.deepEqual(down?.slice(0, 2), [c("a", 0, 1), c("b", 1, 1)]);
  assert.equal(moveGroup(controls, new Set(["a"]), -1, 0, grid), null);
});

test("bestSize shrinks around neighbours", () => {
  const controls = [c("a", 0, 0), c("b", 2, 1)];
  assert.deepEqual(bestSize(controls, "a", 4, 3, grid), { w: 2, h: 3 });
  assert.deepEqual(bestSize(controls, "a", 4, 1, grid), { w: 4, h: 1 });
  assert.deepEqual(bestSize([c("a", 0, 0), c("b", 1, 1)], "a", 3, 3, grid), { w: 3, h: 1 });
  assert.deepEqual(bestSize(controls, "a", 9, 9, { rows: 2, cols: 2 }), { w: 2, h: 2 });
});

test("placeAll keeps free positions and relocates the rest", () => {
  const { placed, skipped } = placeAll([c("a", 0, 0)], [c("p", 0, 0), c("q", 3, 2)], grid);
  assert.equal(skipped, 0);
  assert.deepEqual(placed, [c("p", 1, 0), c("q", 3, 2)]);
  const full = Array.from({ length: 12 }, (_, i) => c(`f${i}`, i % 4, Math.floor(i / 4)));
  assert.equal(placeAll(full, [c("z", 0, 0)], grid).skipped, 1);
});

test("fitToGrid clamps spans and relocates overflow", () => {
  const { controls, removed } = fitToGrid([c("a", 0, 0, 1, 3), c("b", 3, 0), c("c", 1, 0)], { rows: 2, cols: 3 });
  assert.equal(removed, 0);
  assert.deepEqual(controls.find((x) => x.id === "a"), c("a", 0, 0, 1, 2));
  const b = controls.find((x) => x.id === "b");
  assert.ok(b && b.x < 3 && b.y < 2);
  const tiny = fitToGrid([c("a", 0, 0), c("b", 1, 0)], { rows: 1, cols: 1 });
  assert.equal(tiny.removed, 1);
});
