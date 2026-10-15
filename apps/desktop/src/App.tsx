import { useEffect, useLayoutEffect } from "react";
import { ConfirmHost, Toasts } from "./components/Overlays";
import { Sidebar, SCREENS } from "./components/Sidebar";
import { navigate, useApp, useAppTheme, type Screen } from "./lib/store";
import { applyTheme } from "./lib/theme";
import { AudioScreen } from "./screens/audio/AudioScreen";
import { DecksScreen } from "./screens/decks/DecksScreen";
import { IntegrationsScreen } from "./screens/integrations/IntegrationsScreen";
import { LogScreen } from "./screens/log/LogScreen";
import { ProfilesScreen } from "./screens/profiles/ProfilesScreen";
import { SettingsScreen } from "./screens/settings/SettingsScreen";

export function App() {
  const screen = useApp((s) => s.screen);
  const theme = useAppTheme();
  useLayoutEffect(() => applyTheme(theme), [theme]);

  // Cmd/Ctrl + 1..6 switches screens.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return;
      const target = SCREENS[Number(e.key) - 1];
      if (target) {
        e.preventDefault();
        navigate(target.screen);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="flex h-full">
      <Sidebar />
      <main className="flex min-w-0 flex-1 flex-col">
        <ScreenView screen={screen} />
      </main>
      <Toasts />
      <ConfirmHost />
    </div>
  );
}

function ScreenView({ screen }: { screen: Screen }) {
  switch (screen) {
    case "decks":
      return <DecksScreen />;
    case "profiles":
      return <ProfilesScreen />;
    case "audio":
      return <AudioScreen />;
    case "integrations":
      return <IntegrationsScreen />;
    case "settings":
      return <SettingsScreen />;
    case "log":
      return <LogScreen />;
  }
}
