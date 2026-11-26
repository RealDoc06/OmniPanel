# OmniPanel

Turn any phone, tablet or browser into a Stream Deck. Free, open source, for macOS, Windows and Linux.

OmniPanel runs quietly in your tray and serves a touch deck to every device on your network (or over USB on
Android). Old devices are first-class: the deck works on Android 4.4 and iOS 9 browsers.

## Features

- **Any device is a deck.** Open the deck URL in a browser, enter the 6-digit pairing code, done. Connect as many
  devices as you like, each with its own layout.
- **Wi-Fi or USB.** Android devices over USB work through `adb reverse`, which OmniPanel manages for you.
- **Global hotkeys.** Any button can also be triggered by a keyboard shortcut.
- **Actions**
  - Per-app volume with live sliders on the deck, master volume, mic mute
  - Per-app speaker and microphone: send Spotify to the headphones, give Discord another mic (see the table below)
  - Voice changer for your microphone (robot, chipmunk, deep, demon, radio, telephone, echo, cathedral, alien,
    megaphone, underwater, or build your own effect chain)
  - Open apps, files, folders and URLs
  - Hotkeys and text typing, media keys
  - Custom scripts: shell, bash, PowerShell, Python, Node, AppleScript, with output shown on the deck
  - OBS Studio (scenes, record, stream, replay buffer, virtual cam, mute, sources) with live state
  - Home Assistant (toggle entities, call any service)
  - HTTP requests / webhooks, lock, sleep, display off
  - Multi-actions, toggles, delays, pages and layout switching
- **AI assistant.** Describe a button ("mute Discord and switch OBS to BRB") and OmniPanel builds the action or the
  script. OpenAI (`gpt-6-luna` by default), Anthropic, Ollama, OpenRouter or any OpenAI-compatible endpoint.
- **Themes.** Built-in themes plus your own, for the app and per layout.
- **Light.** Rust core, 9 MB app. In the background (window closed) OmniPanel is a single ~30 MB process; the
  UI's webview is destroyed on close and recreated on open. Decks are plain HTML/JS (~40 KB).

## Install

> **Alpha.** Expect rough edges and back up your config between versions. Builds are unsigned.

Download the installer for your OS from the [releases page](https://github.com/RealDoc06/OmniPanel/releases) and run it.

| Platform | Package | Notes |
| --- | --- | --- |
| Windows 10/11 x64 | `*-setup.exe` | Installs WebView2 if missing (needs internet). Per-app volume works out of the box. |
| macOS 11+ (universal) | `.dmg` | Per-app volume needs macOS 14.2+ and the "System Audio Recording" permission. Hotkey/typing actions need Accessibility permission. |
| Linux x86_64 | `.deb`, `.rpm`, `.AppImage` | Needs PulseAudio or PipeWire (`pactl`). Global hotkeys and key simulation need X11 (or XWayland). |
| Linux arm64 | `.deb`, `.rpm`, `.AppImage` from the release workflow | Same as x86_64. |
| Android, iOS, anything with a browser | none | These are decks. Open the deck URL, nothing to install. |

**Speaker and microphone per app**

| Platform | An app's speaker | An app's microphone |
| --- | --- | --- |
| Linux | Yes | Yes, while the app is recording |
| macOS 14.2+ | Yes, for what the app plays on the default output | No: macOS cannot change it, pick it inside the app |
| Windows | Not yet | Not yet |

Pick them per app in **Sound**, or put them on a button ("Play an app on another speaker"). Where it does not work,
the app says so.

**Unsigned builds**

- **Windows:** SmartScreen may warn "Windows protected your PC". Click **More info**, then **Run anyway**.
- **macOS:** Gatekeeper blocks the first launch. Right-click the app and choose **Open**. If macOS says it is damaged, run
  `xattr -cr /Applications/OmniPanel.app`.
- Verify downloads against `SHA256SUMS` when the release includes it.

**Linux notes**

- The deb and rpm pull in their dependencies, including `pactl`. The AppImage bundles the rest except `pactl` and the graphics, font and sound libraries every desktop already has.
- Optional tools: `playerctl` (better media keys), `adb` (USB decks).

**Updates**

OmniPanel looks for a new version when it starts and once a day. When there is one, the tray menu and the sidebar
say so, and **Settings → Updates** installs it and restarts. **Check automatically** in the same place turns the
check off.

- Update packages are signed. One that doesn't match OmniPanel's key is refused.
- deb and rpm installs are left to your package manager: Settings links to the new package instead.
- 0.1.0-alpha.1 has no updater, so the step up from it is still by hand.

**Known limitations**

- Global hotkeys and key simulation do not work on pure Wayland.
- Voice effects need a virtual cable on macOS and Windows (see below).

### Connect a deck

1. In OmniPanel open **Devices** (on first launch it opens there) and click **Connect a device** if devices are already listed.
2. On the phone/tablet open the URL shown (or scan the QR code). Use the same Wi-Fi network.
3. Enter the 6-digit code. The device stays paired.
4. Tip: "Add to Home Screen" gives a full-screen deck. On Android, Developer options → "Stay awake" keeps the
   screen on while charging.

**USB (Android):** enable USB debugging on the device, plug it in, accept the prompt, then open
`http://localhost:7710` on the device. OmniPanel needs `adb` (Android platform-tools) installed and keeps
`adb reverse` active for every connected device. iOS devices connect over Wi-Fi.

Decks rotate the grid automatically when the screen orientation doesn't match the layout (a 5x3 layout shows as
3x5 on a portrait phone). Turn it off per device in **Devices**: click the device, then **Rotate to fit screen**.

### Voice changer

Other apps need a virtual microphone to receive the processed voice:

- **macOS:** install [BlackHole 2ch](https://existential.audio/blackhole/) (`brew install blackhole-2ch`), select it
  as OmniPanel's voice output, then pick "BlackHole 2ch" as the microphone in Discord / OBS / Zoom.
- **Windows:** install [VB-CABLE](https://vb-audio.com/Cable/), select "CABLE Input" as output, pick
  "CABLE Output" as the microphone in your apps.
- **Linux:** click **Create virtual mic** in **Sound**, then pick "OmniPanel_Virtual_Mic" in your apps.

### Permissions (macOS)

| Feature | Permission |
| --- | --- |
| Per-app volume | System Settings → Privacy & Security → Screen & System Audio Recording (asked on first use) |
| Hotkey / type text actions | Privacy & Security → Accessibility |
| Voice changer | Microphone (asked on first use) |
| AppleScript actions | Automation (asked per target app) |

## Development

Requirements: Rust (stable), Node 24, pnpm 10, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/)
for your OS (Linux also needs `libasound2-dev`, `libxdo-dev`, `libxkbcommon-dev` and `libssl-dev`).

```sh
pnpm install
pnpm dev            # desktop app with hot reload (builds the deck client first)
pnpm build          # release installers for this machine in target/release/bundle
pnpm build:release all   # cross-build Linux and Windows installers with Docker, see below
pnpm test           # Rust + TypeScript tests
pnpm gen:types      # regenerate TS types after changing crates/omni-core
cargo run -p omni-host --example serve   # engine without the desktop UI (--bridge N: UI in a browser, see docs)
```

### Release builds

From Linux x86_64 with Docker, Node and pnpm:

```sh
pnpm build:release <linux-x64|linux-arm64|windows-x64|all>   # release-artifacts/<version>/ plus SHA256SUMS
scripts/verify-release.sh                                    # install and launch them in clean containers
```

macOS cannot be cross-built. Tagging `v*` runs the release workflow, which builds every platform into a draft prerelease.

Publishing that draft is what sends the update to installed apps: the workflow checks every package once more
(`scripts/check-update-manifest.sh`) and copies the release's `latest.json` to the `updater` branch, which they read.
A failed or cancelled "publish-update" job means the update did not go out: rerun it. A published version is never rebuilt, so fix a bad
release with a new version.

Only workflow builds can be updates. They are signed with the `TAURI_SIGNING_PRIVATE_KEY` repository secret, whose
public half is `plugins.updater.pubkey` in `tauri.conf.json`; `pnpm build` and `pnpm build:release` make plain
installers. Keep a copy of that private key: installed apps accept nothing signed with another one.

deb and rpm sort `0.1.0-alpha.N` above `0.1.0`, so the first stable release must be `0.2.0` or later.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces fit together.

## License

MIT. Icons from [Lucide](https://lucide.dev) (ISC).
