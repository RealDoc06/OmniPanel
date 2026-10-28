// Live deck preview: drag & drop (move, swap), resize handles, selection, empty squares
// that target the action picker, and context menus. Geometry is computed from the
// pointer delta, so no droppables.
import {
  DndContext,
  PointerSensor,
  useDraggable,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragMoveEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import type { Control, Page, Profile, Theme } from "@omni/protocol";
import { useLayoutEffect, useRef, useState, type MouseEvent, type PointerEvent, type RefObject } from "react";
import { useContextMenu, type MenuItem } from "../../components/ContextMenu";
import { DeckControlView } from "../../components/DeckControl";
import { I } from "../../components/Icon";
import { Button } from "../../components/ui";
import { newControlAt } from "../../lib/controls";
import { bestSize, emptyCells, MAX_GRID } from "../../lib/grid";
import { useIsMac } from "../../lib/store";
import { themeStyle } from "../../lib/theme";
import { clamp, cx } from "../../lib/util";
import {
  addControl,
  addPage,
  copySelected,
  cutSelected,
  deleteSelected,
  dropControl,
  duplicateSelected,
  hasClipboard,
  paste,
  pickAt,
  planDrop,
  resizeControl,
  select,
  setGrid,
  toggleSelect,
  useEditor,
} from "./editor";

interface Ghost {
  x: number;
  y: number;
  w: number;
  h: number;
  valid: boolean;
}

const sameGhost = (a: Ghost | null, b: Ghost | null) =>
  a === b || (!!a && !!b && a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h && a.valid === b.valid);

export function Canvas({ profile, page, theme }: { profile: Profile; page: Page; theme: Theme }) {
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 4 } }));
  /** Cell size + gap, in px, measured by DeckGrid. */
  const pitchRef = useRef(1);
  const [ghost, setGhost] = useState<Ghost | null>(null);
  const grid = { rows: profile.rows, cols: profile.cols };

  /** Where the dragged control would land. */
  const targetOf = (e: DragMoveEvent | DragEndEvent): Ghost | null => {
    const pitch = pitchRef.current;
    const control = page.controls.find((c) => c.id === String(e.active.id));
    if (!control) return null;
    const x = clamp(control.x + Math.round(e.delta.x / pitch), 0, grid.cols - control.w);
    const y = clamp(control.y + Math.round(e.delta.y / pitch), 0, grid.rows - control.h);
    if (x === control.x && y === control.y) return null;
    return { x, y, w: control.w, h: control.h, valid: planDrop(page.controls, control.id, x, y, grid) !== null };
  };

  const onDragStart = (e: DragStartEvent) => {
    const id = String(e.active.id);
    if (!useEditor.getState().selected.includes(id)) select([id]);
  };

  const onDragMove = (e: DragMoveEvent) => {
    const next = targetOf(e);
    setGhost((prev) => (sameGhost(prev, next) ? prev : next));
  };

  const onDragEnd = (e: DragEndEvent) => {
    const target = targetOf(e);
    setGhost(null);
    if (target?.valid) dropControl(String(e.active.id), target.x, target.y);
  };

  return (
    <DndContext
      sensors={sensors}
      autoScroll={false}
      onDragStart={onDragStart}
      onDragMove={onDragMove}
      onDragEnd={onDragEnd}
      onDragCancel={() => setGhost(null)}
    >
      <div className="flex min-h-0 flex-1 flex-col">
        <DeckGrid profile={profile} page={page} theme={theme} ghost={ghost} pitchRef={pitchRef} />
        {emptyCells(page.controls, grid).length === 0 && (
          <div className="flex shrink-0 items-center justify-center gap-1 px-4 pb-4">
            <span className="mr-1">This page is full.</span>
            {profile.rows < MAX_GRID && (
              <Button size="sm" onClick={() => setGrid(profile.rows + 1, profile.cols)}>
                Add a row
              </Button>
            )}
            <Button size="sm" onClick={addPage}>
              Add a page
            </Button>
          </div>
        )}
      </div>
    </DndContext>
  );
}

const GAP_RATIO = 0.1;

function DeckGrid({
  profile,
  page,
  theme,
  ghost,
  pitchRef,
}: {
  profile: Profile;
  page: Page;
  theme: Theme;
  ghost: Ghost | null;
  pitchRef: RefObject<number>;
}) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState({ w: 0, h: 0 });
  const [resizing, setResizing] = useState<{ id: string; w: number; h: number } | null>(null);
  const selected = useEditor((s) => s.selected);
  const target = useEditor((s) => s.target);
  const previewActive = useEditor((s) => s.previewActive);
  const menu = useContextMenu();
  const mac = useIsMac();
  const mod = mac ? "⌘" : "Ctrl+";
  const { rows, cols } = profile;
  const grid = { rows, cols };

  useLayoutEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry) setBox({ w: entry.contentRect.width, h: entry.contentRect.height });
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // Square cells with a gap proportional to the cell, like on the deck.
  const units = (n: number) => n + (n - 1) * GAP_RATIO + 2 * GAP_RATIO;
  const cell = Math.max(24, Math.floor(Math.min(box.w / units(cols), box.h / units(rows))));
  const gap = Math.max(4, Math.round(cell * GAP_RATIO));
  useLayoutEffect(() => {
    pitchRef.current = cell + gap;
  });

  const controlMenu = (e: MouseEvent, control: Control) => {
    if (!selected.includes(control.id)) select([control.id]);
    const items: MenuItem[] = [
      { label: "Cut", icon: "scissors", shortcut: `${mod}X`, run: cutSelected },
      { label: "Copy", icon: "copy", shortcut: `${mod}C`, run: copySelected },
      { label: "Paste", icon: "clipboard-paste", shortcut: `${mod}V`, disabled: !hasClipboard(), run: () => paste() },
      { label: "Duplicate", icon: "copy-plus", shortcut: `${mod}D`, run: duplicateSelected },
      { label: "Delete", icon: "trash-2", shortcut: "⌫", danger: true, run: deleteSelected },
    ];
    menu.open(e, items);
  };

  const cellMenu = (e: MouseEvent, x: number, y: number) =>
    menu.open(e, [
      { label: "Add button…", icon: "square-plus", run: () => pickAt(x, y) },
      {
        label: "Add volume slider",
        icon: "sliders-vertical",
        run: () => addControl(newControlAt(x, y, { kind: { type: "slider", target: { type: "master" }, vertical: true }, style: {} }, page, grid)),
      },
      { label: "Paste here", icon: "clipboard-paste", disabled: !hasClipboard(), run: () => paste({ x, y }) },
    ]);

  const placement = (r: { x: number; y: number; w: number; h: number }) => ({
    gridColumn: `${r.x + 1} / span ${r.w}`,
    gridRow: `${r.y + 1} / span ${r.h}`,
  });

  return (
    <div ref={wrapRef} className="flex min-h-0 flex-1 items-center justify-center overflow-hidden p-3 min-[1100px]:p-6">
      <div
        style={{ ...themeStyle(theme), padding: gap, borderRadius: theme.radius + gap / 2 }}
        className="border border-line bg-bg"
        onMouseDown={(e) => e.target === e.currentTarget && select([])}
      >
        <div
          role="grid"
          aria-label={`${page.name} layout, ${cols} by ${rows}`}
          className="relative grid"
          style={{ gridTemplateColumns: `repeat(${cols}, ${cell}px)`, gridTemplateRows: `repeat(${rows}, ${cell}px)`, gap }}
          onMouseDown={(e) => e.target === e.currentTarget && select([])}
        >
          {emptyCells(page.controls, grid).map(({ x, y }) => {
            const targeted = target?.x === x && target.y === y;
            return (
              <button
                key={`${x}:${y}`}
                type="button"
                aria-label={`Add button at row ${y + 1}, column ${x + 1}`}
                aria-pressed={targeted}
                style={placement({ x, y, w: 1, h: 1 })}
                onClick={() => pickAt(x, y)}
                onContextMenu={(e) => cellMenu(e, x, y)}
                className={cx(
                  "group flex items-center justify-center border text-muted hover:border-solid hover:bg-surface focus-visible:bg-surface",
                  targeted ? "border-solid border-accent" : "border-dashed border-line",
                )}
                data-cell
              >
                <I
                  name="plus"
                  className={cx("size-5 group-hover:opacity-100 group-focus-visible:opacity-100", targeted ? "opacity-100" : "opacity-40")}
                />
              </button>
            );
          })}
          {page.controls.map((control) => {
            const size = resizing?.id === control.id ? resizing : control;
            return (
              <GridControl
                key={control.id}
                control={control}
                style={placement({ x: control.x, y: control.y, w: size.w, h: size.h })}
                selected={selected.includes(control.id)}
                single={selected.length === 1}
                active={previewActive && selected.includes(control.id)}
                pitch={cell + gap}
                onContextMenu={(e) => controlMenu(e, control)}
                onResize={(w, h, done) => {
                  const best = bestSize(page.controls, control.id, w, h, grid);
                  if (!best) return;
                  if (done) {
                    setResizing(null);
                    if (best.w !== control.w || best.h !== control.h) resizeControl(control.id, best.w, best.h);
                  } else setResizing({ id: control.id, ...best });
                }}
              />
            );
          })}
          {ghost && (
            <div
              aria-hidden
              style={{ ...placement(ghost), borderRadius: "var(--radius)" }}
              className={cx("pointer-events-none border-2 border-dashed", ghost.valid ? "border-accent" : "border-danger")}
            />
          )}
        </div>
      </div>
      {menu.menu}
    </div>
  );
}

function GridControl({
  control,
  style,
  selected,
  single,
  active,
  pitch,
  onResize,
  onContextMenu,
}: {
  control: Control;
  style: { gridColumn: string; gridRow: string };
  selected: boolean;
  single: boolean;
  active: boolean;
  pitch: number;
  onResize: (w: number, h: number, done: boolean) => void;
  onContextMenu: (e: MouseEvent) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, isDragging } = useDraggable({ id: control.id });
  const label = control.style.label || (control.kind.type === "slider" ? "Slider" : "Button");
  return (
    <div
      ref={setNodeRef}
      {...attributes}
      {...listeners}
      role="gridcell"
      aria-selected={selected}
      aria-label={`${label}, ${control.w} by ${control.h}`}
      tabIndex={0}
      onClick={(e) => (e.shiftKey || e.metaKey || e.ctrlKey ? toggleSelect(control.id) : select([control.id]))}
      onFocus={(e) => {
        // Keyboard focus selects; pointer focus is handled by onClick (keeps Shift+click working).
        if (e.currentTarget.matches(":focus-visible") && !useEditor.getState().selected.includes(control.id)) select([control.id]);
      }}
      onContextMenu={onContextMenu}
      style={{
        ...style,
        transform: transform ? `translate3d(${transform.x}px, ${transform.y}px, 0)` : undefined,
        zIndex: isDragging ? 20 : selected ? 2 : undefined,
        opacity: isDragging ? 0.85 : undefined,
        borderRadius: "var(--radius)",
      }}
      className={cx("relative touch-none outline-offset-2", selected && "outline-2 outline-accent outline-solid")}
    >
      <DeckControlView control={control} active={active} />
      {selected && single && !isDragging && <ResizeHandle control={control} pitch={pitch} onResize={onResize} />}
    </div>
  );
}

/** Bottom-right handle: drag to change the span in whole cells. */
function ResizeHandle({ control, pitch, onResize }: { control: Control; pitch: number; onResize: (w: number, h: number, done: boolean) => void }) {
  const start = useRef<{ x: number; y: number } | null>(null);
  const sizeAt = (e: PointerEvent) => {
    const origin = start.current;
    if (!origin) return null;
    return {
      w: Math.max(1, control.w + Math.round((e.clientX - origin.x) / pitch)),
      h: Math.max(1, control.h + Math.round((e.clientY - origin.y) / pitch)),
    };
  };
  return (
    <div
      aria-hidden
      title="Drag to resize"
      onPointerDown={(e) => {
        e.stopPropagation();
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        start.current = { x: e.clientX, y: e.clientY };
      }}
      onPointerMove={(e) => {
        const size = sizeAt(e);
        if (size) onResize(size.w, size.h, false);
      }}
      onPointerUp={(e) => {
        const size = sizeAt(e);
        start.current = null;
        if (size) onResize(size.w, size.h, true);
      }}
      onClick={(e) => e.stopPropagation()}
      className="absolute -right-1.5 -bottom-1.5 z-10 size-3.5 cursor-nwse-resize rounded-sm border-2 border-bg bg-accent"
    />
  );
}
