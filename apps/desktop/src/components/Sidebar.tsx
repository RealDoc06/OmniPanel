// Left navigation: the four screens, with the layouts listed under Buttons once there are
// several. The Devices row carries a dot while a device is connected. Arrow keys move between
// items, Cmd/Ctrl+1..4 jump directly (App.tsx). The footer only appears for a newer OmniPanel
// version (opens Settings, Updates) and for unread warnings or errors (opens the Activity log).
import type { KeyboardEvent, ReactNode } from "react";
import type { IconName } from "../lib/icons";
import { navigate, useApp, useProfiles, type Screen } from "../lib/store";
import { useUpdateVersion } from "../lib/updates";
import { cx } from "../lib/util";
import { createProfile, openProfile, useEditor } from "../screens/profiles/editor";
import { I } from "./Icon";
import { IconButton, StatusDot } from "./ui";

export const SCREENS: { screen: Screen; label: string; icon: IconName }[] = [
  { screen: "decks", label: "Devices", icon: "tablet-smartphone" },
  { screen: "profiles", label: "Buttons", icon: "layout-grid" },
  { screen: "sound", label: "Sound", icon: "sliders-horizontal" },
  { screen: "settings", label: "Settings", icon: "settings" },
];

const MOD = /mac/i.test(navigator.platform) ? "⌘" : "Ctrl+";

export function Sidebar() {
  const screen = useApp((s) => s.screen);
  // The Activity log is reached from Settings, so Settings stays highlighted there.
  const current = screen === "log" ? "settings" : screen;
  const problems = useApp((s) => s.unreadErrors);
  const update = useUpdateVersion();
  // With several layouts listed, the open layout's row is the current page, not "Buttons" itself.
  const layoutRows = useProfiles().length > 1;

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button[data-nav]")];
    const index = items.findIndex((b) => b === document.activeElement);
    items[(index + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
  };

  return (
    <nav aria-label="Main" onKeyDown={onKeyDown} className="flex w-[184px] shrink-0 flex-col border-r border-line">
      <div data-tauri-drag-region className="flex h-12 items-center gap-2 px-5">
        <img src="/logo.svg" alt="" className="size-[18px]" draggable={false} />
        <span className="text-[14px] font-semibold tracking-[-0.01em]">OmniPanel</span>
      </div>
      <ul className="flex flex-1 flex-col gap-px px-3 pt-2">
        {SCREENS.map((item, i) => (
          <li key={item.screen} className="relative">
            <button
              type="button"
              data-nav
              aria-current={current === item.screen && !(item.screen === "profiles" && layoutRows) ? "page" : undefined}
              title={`${item.label} (${MOD}${i + 1})`}
              onClick={() => navigate(item.screen)}
              className={cx(
                "flex h-7 w-full items-center gap-2.5 rounded-ui-sm px-2 text-left",
                current === item.screen ? "bg-hover text-fg" : "text-muted hover:text-fg",
              )}
            >
              <I name={item.icon} className="size-4" />
              <span className="flex-1 truncate">{item.label}</span>
              {item.screen === "decks" && <OnlineDot />}
            </button>
            {item.screen === "profiles" && <Layouts active={current === "profiles"} />}
          </li>
        ))}
      </ul>
      <div className="flex flex-col gap-px p-3 empty:hidden">
        {update && (
          <FooterLink tone="ok" onClick={() => navigate("settings", "updates")}>
            Update available
          </FooterLink>
        )}
        {problems > 0 && (
          <FooterLink tone="warn" onClick={() => navigate("log")}>
            {problems} {problems === 1 ? "problem" : "problems"}
          </FooterLink>
        )}
      </div>
    </nav>
  );
}

function FooterLink({ tone, onClick, children }: { tone: "ok" | "warn"; onClick: () => void; children: ReactNode }) {
  return (
    <button type="button" onClick={onClick} className="flex h-7 items-center gap-2 rounded-ui-sm px-2 text-left text-xs text-muted hover:bg-hover hover:text-fg">
      <StatusDot tone={tone} />
      <span className="truncate">{children}</span>
    </button>
  );
}

/** Green while any device is connected, red when the server can't start. Details live on Devices. */
function OnlineDot() {
  const server = useApp((s) => s.runtime.server);
  const online = useApp((s) => s.runtime.sessions.length > 0);
  if (server.error) return <StatusDot tone="error" label="Devices can't connect" />;
  return online ? <StatusDot tone="ok" label="A device is connected" /> : null;
}

/** "New layout" on the Buttons row, plus one row per layout while there are several. */
function Layouts({ active }: { active: boolean }) {
  const profiles = useProfiles();
  const openId = useEditor((s) => s.profileId) ?? profiles[0]?.id;
  return (
    <>
      <IconButton
        icon="plus"
        label="New layout"
        className="absolute top-0 right-0 size-7"
        onClick={() => {
          createProfile();
          navigate("profiles");
        }}
      />
      {profiles.length > 1 && (
        <ul className="my-0.5 flex flex-col gap-px">
          {profiles.map((p) => {
            const open = active && p.id === openId;
            return (
              <li key={p.id}>
                <button
                  type="button"
                  data-nav
                  aria-current={open ? "page" : undefined}
                  onClick={() => {
                    openProfile(p.id);
                    navigate("profiles");
                  }}
                  className={cx("flex h-6 w-full items-center rounded-ui-sm pr-2 pl-[34px] text-left", open ? "text-fg" : "text-muted hover:text-fg")}
                >
                  <span className="truncate">{p.name}</span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </>
  );
}
