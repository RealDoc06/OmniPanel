// App update state for the UI. The shell (src-tauri/src/updater.rs) checks in the
// background and pushes its status; Settings shows it and starts the install.
import { create } from "zustand";
import { updater, type UpdateStatus } from "./backend";
import { flushAll } from "./store";

export const useUpdate = create<{ status: UpdateStatus; installing: boolean }>()(() => ({
  status: { state: "idle" },
  /** `installUpdate` is running: from the click until the shell reports progress, and between its steps. */
  installing: false,
}));

// A status pushed while the first one is being fetched is the newer of the two.
let pushed = false;
await updater.onStatus((status) => {
  pushed = true;
  useUpdate.setState({ status });
});
const initial = await updater.status();
if (!pushed) useUpdate.setState({ status: initial });

/** A newer version is known and not installed yet. */
export const useUpdateVersion = () => useUpdate(({ status }) => ("version" in status ? status.version : null));

/**
 * Downloads the update unless that is done, saves pending edits, then installs it and restarts.
 * Unlike Quit, an update can wait: when a save fails this stops, and the download stays ready
 * for the next try.
 */
export async function installUpdate() {
  if (useUpdate.getState().installing) return;
  useUpdate.setState({ installing: true });
  try {
    if (useUpdate.getState().status.state !== "ready") await updater.download();
    await flushAll();
    await updater.install();
  } finally {
    useUpdate.setState({ installing: false });
  }
}
