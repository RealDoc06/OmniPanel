// Constructors for controls placed by the editor.
import type { Action, Control, Style, VolumeTarget } from "@omni/protocol";
import { ACTION_META, defaultAction, type ActionType } from "./actions";
import { newId } from "./util";

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

/** A button preconfigured for an action type (palette drops). */
export function newActionButton(x: number, y: number, type: ActionType): Control {
  const meta = ACTION_META[type];
  return newButton(x, y, defaultAction(type), { label: meta.label, icon: { type: "builtin", name: meta.icon } });
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

/** Deep copy with a new id (paste, duplicate). The hotkey is dropped to avoid duplicates. */
export const cloneControl = (control: Control): Control => ({ ...structuredClone(control), id: newId(), hotkey: null });
