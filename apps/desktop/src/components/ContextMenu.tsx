// Right-click menu. Keyboard: arrows move, Enter runs, Escape closes.
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import type { IconName } from "../lib/icons";
import { I } from "./Icon";
import { Kbd } from "./ui";

export interface MenuItem {
  label: string;
  icon?: IconName;
  shortcut?: string;
  danger?: boolean;
  disabled?: boolean;
  run: () => void;
}

export interface MenuState {
  x: number;
  y: number;
  items: MenuItem[];
}

/** `open(event, items)` from an onContextMenu handler, render `menu` somewhere. */
export function useContextMenu() {
  const [state, setState] = useState<MenuState | null>(null);
  const open = (e: MouseEvent, items: MenuItem[]) => {
    e.preventDefault();
    e.stopPropagation();
    setState({ x: e.clientX, y: e.clientY, items });
  };
  const menu = state && <ContextMenu state={state} onClose={() => setState(null)} />;
  return { open, menu };
}

function ContextMenu({ state, onClose }: { state: MenuState; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x: state.x, y: state.y });

  // Keep the menu inside the window.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    setPos({ x: Math.min(state.x, innerWidth - width - 4), y: Math.min(state.y, innerHeight - height - 4) });
    el.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [state]);

  useEffect(() => {
    const close = (e: Event) => {
      if (e.target instanceof Node && ref.current?.contains(e.target)) return;
      onClose();
    };
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("resize", onClose);
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose]);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    e.stopPropagation();
    if (e.key === "Escape") return onClose();
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const buttons = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
    const i = buttons.findIndex((b) => b === document.activeElement);
    buttons[(i + (e.key === "ArrowDown" ? 1 : buttons.length - 1)) % buttons.length]?.focus();
  };

  return (
    <div
      ref={ref}
      role="menu"
      onKeyDown={onKeyDown}
      style={{ left: pos.x, top: pos.y }}
      className="fixed z-50 min-w-[190px] rounded-ui-sm border border-line bg-surface py-1 shadow-lg shadow-black/50"
    >
      {state.items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          disabled={item.disabled}
          onClick={() => {
            onClose();
            item.run();
          }}
          className={`flex h-7 w-full items-center gap-2 px-3 text-left hover:bg-hover focus-visible:bg-hover focus-visible:outline-none disabled:opacity-40 ${item.danger ? "text-danger" : "text-fg"}`}
        >
          {item.icon ? <I name={item.icon} className="size-3.5" /> : <span className="w-3.5" />}
          <span className="flex-1">{item.label}</span>
          {item.shortcut && <Kbd>{item.shortcut}</Kbd>}
        </button>
      ))}
    </div>
  );
}
