// Dev bridge: drives a real engine from a plain browser, for testing the UI
// against real state (`cargo run -p omni-host --example serve -- --bridge 7711`,
// then open the UI with `?bridge=http://127.0.0.1:7711`). File dialogs become prompts.
import type { HostEvent } from "@omni/protocol";
import { makeApi } from "./api";
import type { Backend } from "./backend";

export function bridge(base: string, imageUrl: Backend["imageUrl"]): Backend {
  const send = async (request: unknown) => {
    const response = await fetch(`${base}/invoke`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    });
    const text = await response.text();
    if (!response.ok) throw text; // Tauri rejects with the error string too
    return text ? JSON.parse(text) : null;
  };
  return {
    api: makeApi(send, async () => {}),
    onHostEvent: async (handler) => {
      const events = new EventSource(`${base}/events`);
      events.onmessage = (e) => handler(JSON.parse(e.data) as HostEvent);
      return () => events.close();
    },
    dialog: {
      open: async ({ directory }) => window.prompt(directory ? "Folder path" : "File path"),
      save: async (defaultPath) => window.prompt("Save to", defaultPath),
    },
    imageUrl,
  };
}
