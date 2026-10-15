// WebSocket connection to the host: hello handshake, reconnect with backoff,
// keep-alive pings and dead-socket detection.

import type { DeckToHost, HostToDeck } from "@omni/protocol";
import { isPageHidden, onVisibilityChange } from "./dom";

export type ConnStatus = "connecting" | "online" | "offline";

const BACKOFF_START_MS = 500;
const BACKOFF_MAX_MS = 8000;
const PING_EVERY_MS = 20000;
/** No message for this long while "open" means the socket is dead (sleeping phone, Wi-Fi switch). */
const SILENCE_LIMIT_MS = 50000;
const PROBE_TIMEOUT_MS = 4000;

export interface Connection {
  /** Sends a message; false if the socket isn't open. */
  send(msg: DeckToHost): boolean;
  isOpen(): boolean;
  /** Drops the current socket and connects again right away (e.g. after unpairing). */
  restart(): void;
}

export interface ConnHandlers {
  /** `failures` counts consecutive failed attempts since the last time the socket was open. */
  onStatus(status: ConnStatus, failures: number): void;
  /** `raw` is the original JSON text (lets callers skip re-rendering identical layouts). */
  onMessage(msg: HostToDeck, raw: string): void;
}

function socketUrl(): string {
  return (location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/ws";
}

/** Opens and maintains the connection. `hello` builds the first message of every new socket. */
export function connect(handlers: ConnHandlers, hello: () => DeckToHost): Connection {
  let ws: WebSocket | null = null;
  let status: ConnStatus = "connecting";
  let failures = 0;
  let backoff = BACKOFF_START_MS;
  let retryTimer = 0;
  let lastRx = Date.now();

  function setStatus(next: ConnStatus): void {
    status = next;
    handlers.onStatus(next, failures);
  }

  /** Detaches and closes a socket without waiting for the (possibly never-arriving) close event. */
  function abandon(socket: WebSocket): void {
    socket.onopen = socket.onmessage = socket.onclose = socket.onerror = null;
    try {
      socket.close();
    } catch (_e) {
      // already closed
    }
  }

  function scheduleRetry(): void {
    if (retryTimer) return;
    const delay = backoff * (0.85 + Math.random() * 0.3);
    backoff = Math.min(backoff * 2, BACKOFF_MAX_MS);
    retryTimer = window.setTimeout(function () {
      retryTimer = 0;
      open();
    }, delay);
  }

  function lost(): void {
    if (ws) abandon(ws);
    ws = null;
    failures++;
    setStatus("offline");
    scheduleRetry();
  }

  function open(): void {
    if (ws) abandon(ws);
    setStatus(failures === 0 ? "connecting" : "offline");
    let socket: WebSocket;
    try {
      socket = new WebSocket(socketUrl());
    } catch (_e) {
      lost();
      return;
    }
    ws = socket;
    socket.onopen = function () {
      lastRx = Date.now();
      failures = 0;
      backoff = BACKOFF_START_MS;
      send(hello());
      setStatus("online");
    };
    socket.onmessage = function (event) {
      lastRx = Date.now();
      if (typeof event.data !== "string") return;
      let msg: HostToDeck;
      try {
        msg = JSON.parse(event.data) as HostToDeck;
      } catch (_e) {
        return;
      }
      if (msg && typeof msg.t === "string") handlers.onMessage(msg, event.data);
    };
    socket.onclose = function () {
      if (ws === socket) lost();
    };
  }

  function send(msg: DeckToHost): boolean {
    if (!ws || ws.readyState !== 1) return false;
    try {
      ws.send(JSON.stringify(msg));
      return true;
    } catch (_e) {
      return false;
    }
  }

  /** Pings now; if nothing at all comes back soon, the socket is dead: reconnect. */
  function probe(): void {
    if (!ws || ws.readyState !== 1) return;
    const sentAt = Date.now();
    const socket = ws;
    send({ t: "ping" });
    window.setTimeout(function () {
      if (ws === socket && lastRx < sentAt) lost();
    }, PROBE_TIMEOUT_MS);
  }

  function reconnectNow(): void {
    window.clearTimeout(retryTimer);
    retryTimer = 0;
    backoff = BACKOFF_START_MS;
    open();
  }

  window.setInterval(function () {
    if (!ws || ws.readyState !== 1) return;
    if (Date.now() - lastRx > SILENCE_LIMIT_MS) lost();
    else send({ t: "ping" });
  }, PING_EVERY_MS);

  // Coming back to the page (screen on, tab switch) or the network returning: don't wait out the backoff.
  onVisibilityChange(function () {
    if (isPageHidden()) return;
    if (status === "online") probe();
    else reconnectNow();
  });
  window.addEventListener("online", function () {
    if (status !== "online") reconnectNow();
  }, false);

  open();

  return {
    send,
    isOpen: function () {
      return status === "online";
    },
    restart: reconnectNow,
  };
}
