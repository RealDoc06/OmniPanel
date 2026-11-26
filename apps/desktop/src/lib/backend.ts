// The one place that picks the backend:
// - inside the app: Tauri (`host` command + `host-event` events),
// - `?bridge=http://127.0.0.1:7711` in a browser: a real engine started with
//   `cargo run -p omni-host --example serve -- --bridge 7711` (lib/bridge.ts),
// - otherwise the in-memory mock (lib/mock.ts).
// Components import `api`, `onHostEvent`, `dialog`, `imageUrl` and `updater` from here.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import type { HostEvent } from "@omni/protocol";
import { type Api, makeApi } from "./api";

export interface FileFilter {
  name: string;
  extensions: string[];
}

/** App update state, pushed by the shell. Mirrors `Status` in src-tauri/src/updater.rs. */
export type UpdateStatus =
  | { state: "idle" | "checking" | "upToDate" }
  /** `canInstall` is false when this copy can't replace itself (deb, rpm): the user downloads the package. */
  | { state: "available"; version: string; canInstall: boolean }
  | { state: "downloading"; version: string; percent: number | null }
  /** `ready`: downloaded and verified, waiting for `install`. `installing`: a restart follows. */
  | { state: "ready" | "installing"; version: string };

/** The shell's updater. Failed calls reject with a message for the user. */
export interface Updater {
  status: () => Promise<UpdateStatus>;
  onStatus: (handler: (status: UpdateStatus) => void) => Promise<() => void>;
  check: () => Promise<void>;
  download: () => Promise<void>;
  /** Replaces the app with the downloaded update and restarts it. Save pending edits first. */
  install: () => Promise<void>;
}

export interface Backend {
  api: Api;
  /** Subscribe to host pushes (config, runtime, paired, log). Resolves the unsubscribe function. */
  onHostEvent: (handler: (event: HostEvent) => void) => Promise<() => void>;
  dialog: {
    /** "Open" dialog for one file (or folder). Resolves `null` when cancelled. */
    open: (options: { filters?: FileFilter[]; directory?: boolean }) => Promise<string | null>;
    /** "Save" dialog. Resolves the chosen path or `null`. */
    save: (defaultPath: string, filters: FileFilter[]) => Promise<string | null>;
  };
  /** URL of an image imported with `api.importImage`, served by the deck server. */
  imageUrl: (port: number, id: string) => string;
  updater: Updater;
}

const deckImageUrl = (port: number, id: string) => `http://127.0.0.1:${port}/img/${encodeURIComponent(id)}`;

const tauri = (): Backend => ({
  api: makeApi(
    (request) => invoke("host", { request }),
    () => invoke("quit_app"),
  ),
  onHostEvent: (handler) => listen<HostEvent>("host-event", (e) => handler(e.payload)),
  dialog: {
    open: (options) =>
      open({ multiple: false, directory: options.directory ?? false, filters: options.filters }),
    save: (defaultPath, filters) => save({ defaultPath, filters }),
  },
  imageUrl: deckImageUrl,
  updater: {
    status: () => invoke("update_status"),
    onStatus: (handler) => listen<UpdateStatus>("update-status", (e) => handler(e.payload)),
    check: () => invoke("check_for_update"),
    download: () => invoke("download_update"),
    install: () => invoke("install_update"),
  },
});

const bridgeUrl = new URLSearchParams(location.search).get("bridge");

export const { api, onHostEvent, dialog, imageUrl, updater }: Backend =
  "__TAURI_INTERNALS__" in window
    ? tauri()
    : bridgeUrl
      ? (await import("./bridge")).bridge(bridgeUrl, deckImageUrl)
      : (await import("./mock")).backend;
