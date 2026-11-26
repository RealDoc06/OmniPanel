# Changelog

## 0.1.0-alpha.3

- The macOS app is signed with a Developer ID and notarized by Apple, so it opens without a Gatekeeper warning.

## 0.1.0-alpha.2

- Auto-update: OmniPanel checks for a new version at launch and once a day, and installs it from Settings → Updates.
  deb and rpm installs get a link to the new package.
- Choose the speaker and the microphone of each app in Sound, or with a button. Linux does both, macOS the speaker;
  Windows not yet.
- Share an app's sound into your microphone, from the Sound screen or a deck button, so a call hears what you are
  hearing (macOS 14.2+ and Linux; not on Windows yet).

## 0.1.0-alpha.1

First public alpha.

- Any phone, tablet or browser works as a deck over Wi-Fi, or USB on Android.
- Redesigned desktop app with four places (Devices, Buttons, Sound, Settings) and a guided first run.
- Actions: per-app and master volume, mic mute, voice changer, apps and URLs, hotkeys and text, media keys, scripts,
  OBS Studio, Home Assistant, HTTP requests, system actions, multi-actions, toggles, pages and profiles.
- AI assistant that builds a button from a description (OpenAI, Anthropic, Ollama, OpenRouter, or OpenAI-compatible).
- Installers for Windows (NSIS), Linux x86_64 and arm64 (deb, rpm) and Linux x86_64 (AppImage). macOS builds come
  from the release workflow.

Builds are unsigned. See the README for install notes and known limitations.
