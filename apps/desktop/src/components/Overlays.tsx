// Global overlays: toast stack and the confirm dialog.
import { settleConfirm, useConfirm } from "../lib/confirm";
import { dismissToast, useToasts } from "../lib/toast";
import { cx } from "../lib/util";
import { I } from "./Icon";
import { Button, Modal } from "./ui";

export function Toasts() {
  const toasts = useToasts((s) => s.toasts);
  return (
    <div aria-live="polite" className="pointer-events-none fixed right-4 bottom-4 z-50 flex w-[340px] flex-col gap-2">
      {toasts.map((t) => (
        <div
          key={t.id}
          role={t.kind === "error" ? "alert" : "status"}
          className="pointer-events-auto flex items-start gap-2 rounded-ui-sm border border-line bg-surface px-3 py-2.5 shadow-lg shadow-black/40"
        >
          <I
            name={t.kind === "error" ? "circle-x" : t.kind === "success" ? "circle-check" : "info"}
            className={cx("mt-px size-4 shrink-0", t.kind === "error" ? "text-danger" : t.kind === "success" ? "text-success" : "text-accent")}
          />
          <p className="min-w-0 flex-1 break-words whitespace-pre-wrap" data-selectable>
            {t.text}
          </p>
          {t.action && (
            <button
              type="button"
              className="shrink-0 font-medium text-accent hover:underline"
              onClick={() => {
                t.action?.run();
                dismissToast(t.id);
              }}
            >
              {t.action.label}
            </button>
          )}
          <button type="button" aria-label="Dismiss" className="shrink-0 text-muted hover:text-fg" onClick={() => dismissToast(t.id)}>
            <I name="x" className="size-3.5" />
          </button>
        </div>
      ))}
    </div>
  );
}

export function ConfirmHost() {
  const pending = useConfirm((s) => s.pending);
  if (!pending) return null;
  return (
    <Modal
      title={pending.title}
      width={400}
      onClose={() => settleConfirm(false)}
      footer={
        <>
          <Button onClick={() => settleConfirm(false)}>Cancel</Button>
          <Button variant={pending.danger ? "destructive" : "primary"} data-autofocus onClick={() => settleConfirm(true)}>
            {pending.confirmLabel ?? "OK"}
          </Button>
        </>
      }
    >
      {pending.body && <p className="px-4 py-3 text-fg">{pending.body}</p>}
    </Modal>
  );
}
