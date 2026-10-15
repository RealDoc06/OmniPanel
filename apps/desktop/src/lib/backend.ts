// The one place that picks the backend:
// - inside the app: Tauri (`host` command + `host-event` events),
// - `?bridge=http://127.0.0.1:7711` in a browser: a real engine started with
//   `cargo run -p omni-host --example serve -- --bridge 7711` (lib/bridge.ts),
// - otherwise the in-memory mock (lib/mock.ts).
// Components import `api`, `onHostEvent`, `dialog` and `imageUrl` from here.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import type { HostEvent } from "@omni/protocol";
import { type Api, makeApi } from "./api";

export interface FileFilter {
  name: string;
  extensions: string[];
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
});

const bridgeUrl = new URLSearchParams(location.search).get("bridge");

export const { api, onHostEvent, dialog, imageUrl }: Backend =
  "__TAURI_INTERNALS__" in window
    ? tauri()
    : bridgeUrl
      ? (await import("./bridge")).bridge(bridgeUrl, deckImageUrl)
      : (await import("./mock")).backend;
