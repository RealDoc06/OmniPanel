// Promise-based confirmation dialog: `if (await confirm({...})) doIt()`.
// Rendered by <ConfirmHost /> (native window.confirm isn't available in every webview).
import { create } from "zustand";

export interface ConfirmRequest {
  title: string;
  body?: string;
  confirmLabel?: string;
  danger?: boolean;
}

interface Pending extends ConfirmRequest {
  resolve: (ok: boolean) => void;
}

export const useConfirm = create<{ pending: Pending | null }>()(() => ({ pending: null }));

export function confirm(request: ConfirmRequest): Promise<boolean> {
  useConfirm.getState().pending?.resolve(false);
  return new Promise((resolve) => useConfirm.setState({ pending: { ...request, resolve } }));
}

export function settleConfirm(ok: boolean) {
  useConfirm.getState().pending?.resolve(ok);
  useConfirm.setState({ pending: null });
}
