// App update state for the UI. The shell (src-tauri/src/updater.rs) checks in the
// background and pushes its status; Settings shows it and starts the install.
import { create } from "zustand";
import { updater, type UpdateStatus } from "./backend";
import { quitAfterFlush } from "./saver";
import { flushAll } from "./store";

export const useUpdate = create<{ status: UpdateStatus }>()(() => ({ status: { state: "idle" } }));

await updater.onStatus((status) => useUpdate.setState({ status }));
useUpdate.setState({ status: await updater.status() });

/** A newer version is known and not installed yet. */
export const useUpdateVersion = () => useUpdate(({ status }) => ("version" in status ? status.version : null));

/** Downloads the update unless that is done, saves pending edits, then installs it and restarts. */
export async function installUpdate() {
  if (useUpdate.getState().status.state !== "ready") await updater.download();
  await quitAfterFlush(flushAll, updater.install);
}
