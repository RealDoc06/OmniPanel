// Constructors for controls placed by the editor, and id rewriting for copies.
import type { Action, Control, Page, Style, VolumeTarget } from "@omni/protocol";
import { fits, type GridSize } from "./grid.ts";
import type { PickResult } from "./recipes";
import { newId } from "./util.ts";

export const emptyStyle = (): Style => ({
  label: "",
  icon: null,
  bg: null,
  fg: null,
  activeBg: null,
  activeLabel: null,
  activeIcon: null,
  labelPosition: "bottom",
});

export function newButton(x: number, y: number, action: Action | null = null, style: Partial<Style> = {}): Control {
  return {
    id: newId(),
    x,
    y,
    w: 1,
    h: 1,
    style: { ...emptyStyle(), ...style },
    kind: { type: "button", action },
    hotkey: null,
  };
}

export function newSlider(x: number, y: number, h = 1, target: VolumeTarget = { type: "master" }): Control {
  return {
    id: newId(),
    x,
    y,
    w: 1,
    h,
    style: { ...emptyStyle(), label: "Volume", icon: { type: "builtin", name: "volume-2" } },
    kind: { type: "slider", target, vertical: true },
    hotkey: null,
  };
}

/** The control a picker entry makes at an empty square. Sliders are 2 tall when that fits. */
export function newControlAt(x: number, y: number, { kind, style }: PickResult, page: Page, grid: GridSize): Control {
  if (kind.type === "button") return newButton(x, y, kind.action, style);
  const slider = newSlider(x, y, fits({ x, y, w: 1, h: 2 }, page.controls, grid) ? 2 : 1, kind.target);
  return { ...slider, style: { ...slider.style, ...style } };
}

/** Deep copy with a new id (paste, duplicate). The hotkey is dropped to avoid duplicates. */
export const cloneControl = (control: Control): Control => ({ ...structuredClone(control), id: newId(), hotkey: null });

/**
 * Points page and layout links at their copies (old id -> new id), including links
 * inside steps and branches. Mirrors `follow_renamed` in crates/omni-host/src/lib.rs.
 */
export function followRenamed(action: Action, renamed: ReadonlyMap<string, string>): Action {
  switch (action.type) {
    case "navigate": {
      const to = action.to;
      if (to.type === "page") return { ...action, to: { ...to, pageId: renamed.get(to.pageId) ?? to.pageId } };
      if (to.type === "profile") return { ...action, to: { ...to, profileId: renamed.get(to.profileId) ?? to.profileId } };
      return action;
    }
    case "multi":
      return { ...action, steps: action.steps.map((step) => followRenamed(step, renamed)) };
    case "toggle":
      return { ...action, on: followRenamed(action.on, renamed), off: followRenamed(action.off, renamed) };
    default:
      return action;
  }
}
