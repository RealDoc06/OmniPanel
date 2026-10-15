// Left navigation. Arrow keys move between items, Cmd/Ctrl+1..5 jump directly (App.tsx).
// The footer shows who is connected and, when there are unread warnings or errors, a link to the Activity log.
import type { KeyboardEvent } from "react";
import type { IconName } from "../lib/icons";
import { navigate, useApp, type Screen } from "../lib/store";
import { cx } from "../lib/util";
import { I } from "./Icon";
import { StatusDot } from "./ui";

export const SCREENS: { screen: Screen; label: string; icon: IconName }[] = [
  { screen: "decks", label: "Devices", icon: "tablet-smartphone" },
  { screen: "profiles", label: "Buttons", icon: "layout-grid" },
  { screen: "audio", label: "Volume", icon: "sliders-horizontal" },
  { screen: "voice", label: "Voice changer", icon: "audio-waveform" },
  { screen: "settings", label: "Settings", icon: "settings" },
];

const MOD = /mac/i.test(navigator.platform) ? "⌘" : "Ctrl+";

export function Sidebar() {
  const screen = useApp((s) => s.screen);
  // The Activity log is reached from Settings, so Settings stays highlighted there.
  const current = screen === "log" ? "settings" : screen;
  const problems = useApp((s) => s.unreadErrors);

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button[data-nav]")];
    const index = items.findIndex((b) => b === document.activeElement);
    items[(index + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
  };

  return (
    <nav aria-label="Main" onKeyDown={onKeyDown} className="flex w-[188px] shrink-0 flex-col border-r border-line bg-surface">
      <div data-tauri-drag-region className="flex h-12 items-center gap-2 px-4">
        <img src="/logo.svg" alt="" className="size-5" draggable={false} />
        <span className="font-semibold tracking-tight">OmniPanel</span>
      </div>
      <ul className="flex flex-1 flex-col gap-px px-2">
        {SCREENS.map((item, i) => (
          <li key={item.screen}>
            <button
              type="button"
              data-nav
              aria-current={current === item.screen ? "page" : undefined}
              title={`${item.label} (${MOD}${i + 1})`}
              onClick={() => navigate(item.screen)}
              className={cx(
                "flex h-8 w-full items-center gap-2.5 rounded-ui-sm px-2.5 text-left",
                current === item.screen ? "bg-hover text-fg" : "text-muted hover:bg-hover hover:text-fg",
              )}
            >
              <I name={item.icon} className="size-4" />
              <span className="flex-1 truncate">{item.label}</span>
            </button>
          </li>
        ))}
      </ul>
      <div className="m-2 flex flex-col text-xs">
        <ConnectionStatus />
        {problems > 0 && (
          <button
            type="button"
            onClick={() => navigate("log")}
            className="rounded-ui-sm px-2.5 py-1.5 text-left text-danger hover:bg-hover"
          >
            {problems} {problems === 1 ? "problem" : "problems"}
          </button>
        )}
      </div>
    </nav>
  );
}

/** Footer line 1: whether devices can connect, and who is connected. Opens Devices. */
function ConnectionStatus() {
  const server = useApp((s) => s.runtime.server);
  const sessions = useApp((s) => s.runtime.sessions);
  const devices = useApp((s) => s.config.devices);
  // A device can have several sessions (two tabs); count devices, not sessions.
  const online = new Set(sessions.map((x) => x.deviceId));
  const only = online.size === 1 ? devices.find((d) => online.has(d.id)) : undefined;
  const text = server.error
    ? "Devices can't connect"
    : !server.running
      ? "Starting…"
      : online.size === 0
        ? "No device connected"
        : online.size === 1
          ? `${only?.name || "Device"} connected`
          : `${online.size} devices connected`;
  return (
    <button
      type="button"
      onClick={() => navigate("decks")}
      className="flex items-center gap-2 rounded-ui-sm px-2.5 py-1.5 text-left text-muted hover:bg-hover"
    >
      <StatusDot tone={server.error ? "error" : !server.running ? "warn" : online.size ? "ok" : "off"} />
      <span className="truncate">{text}</span>
    </button>
  );
}
