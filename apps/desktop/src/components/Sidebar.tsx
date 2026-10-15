// Left navigation. Arrow keys move between items, Cmd/Ctrl+1..6 jump directly (App.tsx).
import type { KeyboardEvent } from "react";
import type { IconName } from "../lib/icons";
import { navigate, useApp, type Screen } from "../lib/store";
import { cx } from "../lib/util";
import { I } from "./Icon";
import { StatusDot } from "./ui";

export const SCREENS: { screen: Screen; label: string; icon: IconName }[] = [
  { screen: "decks", label: "Decks", icon: "tablet-smartphone" },
  { screen: "profiles", label: "Profiles", icon: "layout-grid" },
  { screen: "audio", label: "Audio", icon: "sliders-horizontal" },
  { screen: "integrations", label: "Integrations", icon: "plug" },
  { screen: "settings", label: "Settings", icon: "settings" },
  { screen: "log", label: "Log", icon: "scroll-text" },
];

const MOD = /mac/i.test(navigator.platform) ? "⌘" : "Ctrl+";

export function Sidebar() {
  const current = useApp((s) => s.screen);
  const unread = useApp((s) => s.unreadErrors);
  const online = useApp((s) => new Set(s.runtime.sessions.map((x) => x.deviceId)).size);
  const server = useApp((s) => s.runtime.server);

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
          <li key={item.screen} className={cx(item.screen === "log" && "mt-auto")}>
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
              <span className="flex-1">{item.label}</span>
              {item.screen === "log" && unread > 0 && (
                <span className="min-w-5 rounded-full bg-danger px-1.5 text-center text-[11px] font-semibold text-white">
                  {unread}
                </span>
              )}
            </button>
          </li>
        ))}
      </ul>
      <button
        type="button"
        onClick={() => navigate("decks")}
        className="m-2 flex items-center gap-2 rounded-ui-sm px-2.5 py-2 text-left text-xs text-muted hover:bg-hover"
      >
        <StatusDot tone={server.error ? "error" : server.running ? (online ? "ok" : "off") : "warn"} />
        <span className="truncate">
          {server.error ? "Deck server error" : server.running ? `${online} deck${online === 1 ? "" : "s"} online` : "Server starting"}
        </span>
      </button>
    </nav>
  );
}
