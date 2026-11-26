# OmniPanel architecture

OmniPanel is a free, open-source Stream Deck alternative for macOS, Windows and
Linux. Any device with a browser (old phones, tablets, laptops) becomes a deck.

```
                 ┌─────────────────────────── desktop app (Tauri 2) ───────────────────────────┐
  phone/tablet   │  omni-host                                                                   │
  browser  ──ws──┼─▶ HTTP + WebSocket server (axum) ─▶ dispatcher ─▶ omni-actions ─▶ OS / OBS / HA │
  (apps/deck)    │   pairing, sessions, layouts,      (multi/toggle/   ├─ omni-audio  (volumes)  │
  USB: adb       │   state feedback, adb reverse      nav/voice)       └─ omni-voice  (voice fx) │
  reverse ───────┤                                                     omni-ai (LLM → action)   │
                 │  tray, global hotkeys, autostart ◀── apps/desktop/src-tauri (thin shell)     │
                 │  React UI (apps/desktop/src) ◀── Tauri commands + `host-event` events        │
                 └──────────────────────────────────────────────────────────────────────────────┘
```

## Repository

| Path | What |
| --- | --- |
| `crates/omni-core` | Domain model (config, profiles, controls, actions), deck protocol, UI runtime types, built-in themes / voice presets, config persistence. Every cross-boundary type derives `ts_rs::TS`. |
| `crates/omni-audio` | Master / mic / per-app volume. macOS CoreAudio + process taps, Windows WASAPI, Linux `pactl`. |
| `crates/omni-voice` | Real-time voice changer: mic → effect chain → output device (virtual cable). |
| `crates/omni-actions` | Executes leaf actions: keys, apps, scripts, media, volume, system, HTTP, OBS (obs-websocket v5), Home Assistant. Installed-app discovery. |
| `crates/omni-ai` | LLM providers (OpenAI default `gpt-6-luna`, Anthropic, Ollama, OpenRouter, custom OpenAI-compatible) that turn a description into a `GeneratedControl`. |
| `crates/omni-host` | The engine: config store, deck server, pairing, sessions, dispatcher, state feedback, USB (adb) bridge. No Tauri dependency, testable headless. |
| `apps/desktop/src-tauri` | Tauri shell: commands, tray, window lifecycle, global shortcuts, autostart, app updates. |
| `apps/desktop/src` | Desktop UI (React 19, Vite, Tailwind 4, Zustand). |
| `apps/deck` | Deck web client. Vanilla TypeScript compiled to ES5, served by the host. |
| `packages/protocol` | TypeScript types generated from `omni-core` (`pnpm gen:types`). |
| `packages/icons` | Lucide icon markup (`icons.json`) + search tags, shared by host, UI and deck. |

## Key decisions

- **Low resource usage.** Rust host; the UI webview is destroyed when the window
  closes (app keeps running in the tray, ~15-30 MB). The deck client is
  dependency-free vanilla JS. No polling unless something on screen needs it.
- **Old devices.** Deck client output is ES5 (Android 4.4 / iOS 9 baseline): no
  Promise/fetch/Map/Set/CSS variables/CSS grid; layout is computed in JS.
- **Pairing.** The desktop shows a 6-digit code (valid 10 min). The deck sends it
  once and receives a random token (stored on the device; the host only keeps
  its SHA-256). Every WebSocket must authenticate with the token.
- **USB.** Android devices over USB use `adb reverse tcp:<port> tcp:<port>`;
  the deck opens `http://localhost:<port>`. iOS uses Wi-Fi.
- **Per-device presets.** Each device is assigned a profile (grid + pages).
- **Voice effects** need a virtual cable on macOS/Windows (BlackHole, VB-CABLE);
  on Linux OmniPanel creates a PipeWire/PulseAudio virtual mic itself.
- **Updates** live in the shell (`updater.rs`, `tauri-plugin-updater`), because the daily check has to run
  while the webview is gone. Apps read `latest.json` from the `updater` branch, which the release workflow
  rewrites when a release is published and its packages verify (GitHub's "latest release" link skips
  prereleases). The user starts every install from Settings; deb and rpm installs only get a link, the
  package manager owns those files.
- **Flow-control actions** (`multi`, `toggle`, `delay`, `navigate`, `voice`)
  live in `omni-host`; all other actions are leaves run by `omni-actions`.

## Deck protocol

See `crates/omni-core/src/protocol.rs`. JSON over `ws://<host>:<port>/ws`,
tagged by `t`. Host also serves: `/` (deck client from `apps/deck/dist`),
`/img/<id>` (imported button images), `/api/info`.

## Desktop IPC

The UI sends `HostRequest`s (`crates/omni-core/src/api.rs`, generated to TS) to
one async Tauri command, `host`, which calls `Host::handle` on the engine's
runtime. `apps/desktop/src/lib/api.ts` pins each request's result type. The
host pushes `HostEvent`s on the `host-event` channel.

To test the UI against a real engine in a browser, run
`cargo run -p omni-host --example serve -- --port 7800 --bridge 7801` and
`pnpm --filter @omni/desktop dev`, then open
`http://localhost:1420/?bridge=http://127.0.0.1:7801`. Without `?bridge` the
browser UI uses an in-memory mock.

## Data

`config.json` in the OS app-data dir (`~/Library/Application Support/dev.omnipanel.desktop`,
`%APPDATA%\dev.omnipanel.desktop`, `~/.local/share/dev.omnipanel.desktop`), written
atomically, owner-only permissions. Imported images in `images/`.
