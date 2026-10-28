// Last-resort error screen, for render crashes (ErrorBoundary) and startup failures (main.tsx).
// main.tsx imports this before the store loads, so it must not import anything that pulls in
// the store (ui.tsx does, through Icon.tsx). The button uses the plain index.css classes.
import { Component, type ReactNode } from "react";
import { errorMessage } from "../lib/util";

/** Full-window error with the message and a Reload button. Reload also runs the store's
 * confirmed save flush when the store loaded successfully. */
export function ErrorScreen({ title = "Something went wrong", error }: { title?: string; error: unknown }) {
  return (
    <div role="alert" className="flex h-full flex-col items-start justify-center gap-3 bg-black p-10 text-white">
      <h1 className="text-[15px] font-semibold">{title}</h1>
      <p>OmniPanel hit an error it couldn't recover from.</p>
      <pre data-selectable className="max-h-60 max-w-full overflow-auto font-mono text-xs break-words whitespace-pre-wrap">
        {errorMessage(error)}
      </pre>
      <button type="button" className="btn btn-primary" onClick={() => void reload()}>
        Reload
      </button>
    </div>
  );
}

/** Startup failures can leave the store unavailable; render failures can still flush it. */
async function reload() {
  const store = await import("../lib/store").catch(() => null);
  if (!store) {
    location.reload();
    return;
  }
  if (await store.run(store.flushAll())) location.reload();
}

interface State {
  failed: boolean;
  error: unknown;
}

/** Shows `ErrorScreen` instead of a blank window when rendering throws. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  override state: State = { failed: false, error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { failed: true, error };
  }

  override render() {
    return this.state.failed ? <ErrorScreen error={this.state.error} /> : this.props.children;
  }
}
