// Live deck preview with drag & drop (move, swap, add from the palette), resize handles,
// selection and a context menu. Geometry is computed from the pointer, so no droppables.
import {
  DndContext,
  DragOverlay,
  PointerSensor,
  useDraggable,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragMoveEvent,
  type DragStartEvent,
  type UniqueIdentifier,
} from "@dnd-kit/core";
import type { Control, Page, Profile, Theme } from "@omni/protocol";
import { useLayoutEffect, useRef, useState, type MouseEvent, type PointerEvent, type RefObject } from "react";
import { useContextMenu, type MenuItem } from "../../components/ContextMenu";
import { DeckControlView } from "../../components/DeckControl";
import { I } from "../../components/Icon";
import { ACTION_META, ACTION_TYPES, isActionType, type ActionType } from "../../lib/actions";
import { newActionButton, newButton, newSlider } from "../../lib/controls";
import { bestSize, emptyCells, findFreeSpot, fits } from "../../lib/grid";
import { useIsMac } from "../../lib/store";
import { themeStyle } from "../../lib/theme";
import { clamp, cx } from "../../lib/util";
import {
  addControl,
  copySelected,
  cutSelected,
  deleteSelected,
  dropControl,
  duplicateSelected,
  hasClipboard,
  paste,
  planDrop,
  resizeControl,
  select,
  toggleSelect,
  useEditor,
} from "./editor";

type PaletteItem = ActionType | "slider";

type DragItem = { kind: "control"; id: string } | { kind: "palette"; item: PaletteItem };

const PALETTE_PREFIX = "palette:";

function parseDragId(id: UniqueIdentifier): DragItem {
  const value = String(id);
  if (!value.startsWith(PALETTE_PREFIX)) return { kind: "control", id: value };
  const item = value.slice(PALETTE_PREFIX.length);
  return { kind: "palette", item: isActionType(item) ? item : "slider" };
}

interface Ghost {
  x: number;
  y: number;
  w: number;
  h: number;
  valid: boolean;
}

interface Metrics {
  el: HTMLDivElement | null;
  /** Cell size + gap, in px. */
  pitch: number;
}

const sameGhost = (a: Ghost | null, b: Ghost | null) =>
  a === b || (!!a && !!b && a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h && a.valid === b.valid);

function newFromPalette(item: PaletteItem, x: number, y: number, h: number): Control {
  return item === "slider" ? newSlider(x, y, h) : newActionButton(x, y, item);
}

export function Canvas({ profile, page, theme }: { profile: Profile; page: Page; theme: Theme }) {
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 4 } }));
  const metrics = useRef<Metrics>({ el: null, pitch: 1 });
  const [ghost, setGhost] = useState<Ghost | null>(null);
  const [dragging, setDragging] = useState<DragItem | null>(null);
  const grid = { rows: profile.rows, cols: profile.cols };

  const sliderHeight = (x: number, y: number) => (fits({ x, y, w: 1, h: 2 }, page.controls, grid) ? 2 : 1);

  const targetOf = (e: DragMoveEvent | DragEndEvent): Ghost | null => {
    const { el, pitch } = metrics.current;
    if (!el) return null;
    const item = parseDragId(e.active.id);
    if (item.kind === "control") {
      const control = page.controls.find((c) => c.id === item.id);
      if (!control) return null;
      const x = clamp(control.x + Math.round(e.delta.x / pitch), 0, grid.cols - control.w);
      const y = clamp(control.y + Math.round(e.delta.y / pitch), 0, grid.rows - control.h);
      if (x === control.x && y === control.y) return null;
      return { x, y, w: control.w, h: control.h, valid: planDrop(page.controls, control.id, x, y, grid) !== null };
    }
    const rect = e.active.rect.current.translated;
    if (!rect) return null;
    const box = el.getBoundingClientRect();
    const px = rect.left + rect.width / 2 - box.left;
    const py = rect.top + rect.height / 2 - box.top;
    if (px < 0 || py < 0 || px > box.width || py > box.height) return null;
    const x = clamp(Math.floor(px / pitch), 0, grid.cols - 1);
    const y = clamp(Math.floor(py / pitch), 0, grid.rows - 1);
    const h = item.item === "slider" ? sliderHeight(x, y) : 1;
    return { x, y, w: 1, h, valid: fits({ x, y, w: 1, h }, page.controls, grid) };
  };

  const onDragStart = (e: DragStartEvent) => {
    const item = parseDragId(e.active.id);
    setDragging(item);
    if (item.kind === "control" && !useEditor.getState().selected.includes(item.id)) select([item.id]);
  };

  const onDragMove = (e: DragMoveEvent) => {
    const next = targetOf(e);
    setGhost((prev) => (sameGhost(prev, next) ? prev : next));
  };

  const onDragEnd = (e: DragEndEvent) => {
    const target = targetOf(e);
    const item = parseDragId(e.active.id);
    setGhost(null);
    setDragging(null);
    if (!target?.valid) return;
    if (item.kind === "control") dropControl(item.id, target.x, target.y);
    else addControl(newFromPalette(item.item, target.x, target.y, target.h));
  };

  /** Keyboard / click add from the palette: first free cell. */
  const addFromPalette = (item: PaletteItem) => {
    const tall = item === "slider" ? findFreeSpot(page.controls, grid, 1, 2) : null;
    const spot = tall ?? findFreeSpot(page.controls, grid);
    if (spot) addControl(newFromPalette(item, spot.x, spot.y, tall ? 2 : 1));
  };

  return (
    <DndContext
      sensors={sensors}
      autoScroll={false}
      onDragStart={onDragStart}
      onDragMove={onDragMove}
      onDragEnd={onDragEnd}
      onDragCancel={() => {
        setGhost(null);
        setDragging(null);
      }}
    >
      <div className="flex min-h-0 flex-1 flex-col">
        <DeckGrid profile={profile} page={page} theme={theme} ghost={ghost} metrics={metrics} />
        <Palette onAdd={addFromPalette} />
      </div>
      <DragOverlay dropAnimation={null}>
        {dragging?.kind === "palette" && (
          <div className="inline-flex h-7 items-center gap-1.5 rounded-ui-sm border border-line bg-hover px-2 text-xs whitespace-nowrap text-fg shadow-lg shadow-black/50">
            <ChipContent item={dragging.item} />
          </div>
        )}
      </DragOverlay>
    </DndContext>
  );
}

const GAP_RATIO = 0.1;

function DeckGrid({
  profile,
  page,
  theme,
  ghost,
  metrics,
}: {
  profile: Profile;
  page: Page;
  theme: Theme;
  ghost: Ghost | null;
  metrics: RefObject<Metrics>;
}) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState({ w: 0, h: 0 });
  const [resizing, setResizing] = useState<{ id: string; w: number; h: number } | null>(null);
  const selected = useEditor((s) => s.selected);
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
    metrics.current = { el: gridRef.current, pitch: cell + gap };
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
      { label: "Add button", icon: "square-plus", run: () => addControl(newButton(x, y)) },
      {
        label: "Add slider",
        icon: "sliders-vertical",
        run: () => addControl(newSlider(x, y, fits({ x, y, w: 1, h: 2 }, page.controls, grid) ? 2 : 1)),
      },
      { label: "Paste here", icon: "clipboard-paste", disabled: !hasClipboard(), run: () => paste({ x, y }) },
    ]);

  const placement = (r: { x: number; y: number; w: number; h: number }) => ({
    gridColumn: `${r.x + 1} / span ${r.w}`,
    gridRow: `${r.y + 1} / span ${r.h}`,
  });

  return (
    <div ref={wrapRef} className="flex min-h-0 flex-1 items-center justify-center overflow-hidden p-6">
      <div
        style={{ ...themeStyle(theme), padding: gap, borderRadius: theme.radius + gap / 2 }}
        className="border border-line bg-bg"
        onMouseDown={(e) => e.target === e.currentTarget && select([])}
      >
        <div
          ref={gridRef}
          role="grid"
          aria-label={`${page.name} layout, ${cols} by ${rows}`}
          className="relative grid"
          style={{ gridTemplateColumns: `repeat(${cols}, ${cell}px)`, gridTemplateRows: `repeat(${rows}, ${cell}px)`, gap }}
          onMouseDown={(e) => e.target === e.currentTarget && select([])}
        >
          {emptyCells(page.controls, grid).map(({ x, y }) => (
            <button
              key={`${x}:${y}`}
              type="button"
              aria-label={`Add button at row ${y + 1}, column ${x + 1}`}
              style={placement({ x, y, w: 1, h: 1 })}
              onClick={() => addControl(newButton(x, y))}
              onContextMenu={(e) => cellMenu(e, x, y)}
              className="group flex items-center justify-center border border-dashed border-line text-muted hover:border-solid hover:bg-surface focus-visible:bg-surface"
              data-cell
            >
              <I name="plus" className="size-5 opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100" />
            </button>
          ))}
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

function Palette({ onAdd }: { onAdd: (item: PaletteItem) => void }) {
  const items: PaletteItem[] = [...ACTION_TYPES, "slider"];
  return (
    <div className="flex shrink-0 flex-wrap justify-center gap-1 border-t border-line px-4 py-2.5" aria-label="Add control">
      {items.map((item) => (
        <PaletteChip key={item} item={item} onAdd={onAdd} />
      ))}
    </div>
  );
}

function PaletteChip({ item, onAdd }: { item: PaletteItem; onAdd: (item: PaletteItem) => void }) {
  const { attributes, listeners, setNodeRef } = useDraggable({ id: `${PALETTE_PREFIX}${item}` });
  return (
    <button
      ref={setNodeRef}
      type="button"
      {...attributes}
      {...listeners}
      title="Drag onto the deck, or click to add"
      onClick={() => onAdd(item)}
      className="inline-flex h-7 touch-none items-center gap-1.5 rounded-ui-sm border border-line bg-surface px-2 text-xs whitespace-nowrap text-muted hover:bg-hover hover:text-fg"
    >
      <ChipContent item={item} />
    </button>
  );
}

function ChipContent({ item }: { item: PaletteItem }) {
  const meta = item === "slider" ? { label: "Slider", icon: "sliders-vertical" as const } : ACTION_META[item];
  return (
    <>
      <I name={meta.icon} className="size-3.5" />
      {meta.label}
    </>
  );
}
