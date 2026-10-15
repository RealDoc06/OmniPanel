// Slim global notifications. `toast()` from anywhere, <Toasts /> renders them.
import { create } from "zustand";
import { errorMessage } from "./util";

export type ToastKind = "info" | "success" | "error";

export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
  action?: { label: string; run: () => void };
}

export const useToasts = create<{ toasts: Toast[] }>()(() => ({ toasts: [] }));

let seq = 0;

export function toast(text: string, kind: ToastKind = "info", action?: Toast["action"]) {
  const id = ++seq;
  useToasts.setState((s) => ({ toasts: [...s.toasts.slice(-3), { id, kind, text, action }] }));
  setTimeout(() => dismissToast(id), kind === "error" ? 7000 : action ? 6000 : 3500);
}

export const toastError = (error: unknown) => toast(errorMessage(error), "error");

export const dismissToast = (id: number) => useToasts.setState((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
