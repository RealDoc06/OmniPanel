# OmniPanel

Turn any phone, tablet or browser into a Stream Deck. Free, open source, for macOS, Windows and Linux.

OmniPanel runs quietly in your tray and serves a touch deck to every device on your network (or over USB on
Android). Old devices are first-class: the deck works on Android 4.4 and iOS 9 browsers.

## Features

- **Any device is a deck.** Open the deck URL in a browser, enter the 6-digit pairing code, done. Connect as many
  devices as you like, each with its own profile (layout).
- **Wi-Fi or USB.** Android devices over USB work through `adb reverse`, which OmniPanel manages for you.
- **Global hotkeys.** Any button can also be triggered by a keyboard shortcut.
- **Actions**
  - Per-app volume with live sliders on the deck, master volume, mic mute
  - Voice changer for your microphone (robot, chipmunk, deep, demon, radio, telephone, echo, cathedral, alien,
    megaphone, underwater, or build your own effect chain)
  - Open apps, files, folders and URLs
  - Hotkeys and text typing, media keys
  - Custom scripts: shell, bash, PowerShell, Python, Node, AppleScript, with output shown on the deck
  - OBS Studio (scenes, record, stream, replay buffer, virtual cam, mute, sources) with live state
  - Home Assistant (toggle entities, call any service)
  - HTTP requests / webhooks, lock, sleep, display off
  - Multi-actions, toggles, delays, pages and profile switching
- **AI assistant.** Describe a button ("mute Discord and switch OBS to BRB") and OmniPanel builds the action or the
  script. OpenAI (`gpt-6-luna` by default), Anthropic, Ollama, OpenRouter or any OpenAI-compatible endpoint.
- **Themes.** Built-in themes plus your own, for the app and per deck profile.
- **Light.** Rust core, 9 MB app. In the background (window closed) OmniPanel is a single ~30 MB process; the
  UI's webview is destroyed on close and recreated on open. Decks are plain HTML/JS (~40 KB).

## Install

Download the installer for your OS from the releases page and run it.

| OS | Notes |
| --- | --- |
| macOS 11+ | Per-app volume needs macOS 14.2+ and the "System Audio Recording" permission. Hotkey/typing actions need Accessibility permission. |
| Windows 10/11 | Per-app volume works out of the box. |
| Linux | Needs PulseAudio or PipeWire (`pactl`). Global hotkeys and key simulation need X11 (or XWayland). |

### Connect a deck

1. In OmniPanel open **Decks → Pair a device**.
2. On the phone/tablet open the URL shown (or scan the QR code). Use the same Wi-Fi network.
3. Enter the 6-digit code. The device stays paired.
4. Tip: "Add to Home Screen" gives a full-screen deck. On Android, Developer options → "Stay awake" keeps the
   screen on while charging.

**USB (Android):** enable USB debugging on the device, plug it in, accept the prompt, then open
`http://localhost:7710` on the device. OmniPanel needs `adb` (Android platform-tools) installed and keeps
`adb reverse` active for every connected device. iOS devices connect over Wi-Fi.

Decks rotate the grid automatically when the screen orientation doesn't match the profile (a 5x3 profile shows as
3x5 on a portrait phone). Turn it off per device in **Decks**.

### Voice changer

Other apps need a virtual microphone to receive the processed voice:

- **macOS:** install [BlackHole 2ch](https://existential.audio/blackhole/) (`brew install blackhole-2ch`), select it
  as OmniPanel's voice output, then pick "BlackHole 2ch" as the microphone in Discord / OBS / Zoom.
- **Windows:** install [VB-CABLE](https://vb-audio.com/Cable/), select "CABLE Input" as output, pick
  "CABLE Output" as the microphone in your apps.
- **Linux:** click **Create virtual mic** in OmniPanel, then pick "OmniPanel Virtual Mic" in your apps.

### Permissions (macOS)

| Feature | Permission |
| --- | --- |
| Per-app volume | System Settings → Privacy & Security → Screen & System Audio Recording (asked on first use) |
| Hotkey / type text actions | Privacy & Security → Accessibility |
| Voice changer | Microphone (asked on first use) |
| AppleScript actions | Automation (asked per target app) |

## Development

Requirements: Rust (stable), Node 22+, pnpm 10, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/)
for your OS (Linux also needs `libasound2-dev`, `libxdo-dev` and `libxkbcommon-dev`).

```sh
pnpm install
pnpm dev            # desktop app with hot reload (builds the deck client first)
pnpm build          # release installers in target/release/bundle
pnpm test           # Rust + TypeScript tests
pnpm gen:types      # regenerate TS types after changing crates/omni-core
cargo run -p omni-host --example serve   # engine without the desktop UI (--bridge N: UI in a browser, see docs)
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces fit together.

## License

MIT. Icons from [Lucide](https://lucide.dev) (ISC).
