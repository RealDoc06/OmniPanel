// Activity log: recent host log events (shortcut registration failures, action errors...).
// Reached from the sidebar's "N problems" line and Settings > Advanced.
import type { LogLevel } from "@omni/protocol";
import { useState } from "react";
import { Button, EmptyState, ScreenHeader, Segmented } from "../../components/ui";
import { clearLogs, navigate, useApp } from "../../lib/store";

type Filter = "all" | LogLevel;

const LEVELS: Record<LogLevel, { label: string; className: string }> = {
  info: { label: "Info", className: "text-muted" },
  warn: { label: "Warning", className: "text-warning" },
  error: { label: "Error", className: "text-danger" },
};

const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });

export function LogScreen() {
  const logs = useApp((s) => s.logs);
  const [filter, setFilter] = useState<Filter>("all");
  const shown = filter === "all" ? logs : logs.filter((l) => l.level === filter);
  return (
    <>
      <ScreenHeader title="Activity log">
        <Segmented
          label="Filter"
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: "All" },
            { value: "info", label: "Info" },
            { value: "warn", label: "Warnings" },
            { value: "error", label: "Errors" },
          ]}
        />
        <Button size="sm" disabled={logs.length === 0} onClick={clearLogs}>
          Clear list
        </Button>
        <Button size="sm" onClick={() => navigate("settings", "advanced")}>
          Back to Settings
        </Button>
      </ScreenHeader>
      <div className="flex-1 overflow-y-auto">
        <p className="border-b border-line px-6 py-2 text-xs">Warnings show something that may need your attention. Errors show what couldn't finish. The problem count includes new warnings and errors; opening this log clears the count.</p>
        {shown.length === 0 ? (
          <EmptyState icon="scroll-text" title={filter === "all" ? "Nothing to report since OmniPanel opened." : "Nothing at this level."} />
        ) : (
          <ol data-selectable className="font-mono text-xs">
            {shown.map((entry, i) => (
              <li key={`${entry.at}-${i}`} className="grid grid-cols-[72px_56px_1fr] gap-3 border-b border-line px-6 py-1.5">
                <span className="text-muted tabular-nums">{time(entry.at)}</span>
                <span className={LEVELS[entry.level].className}>{LEVELS[entry.level].label}</span>
                <span className="break-words whitespace-pre-wrap text-fg">{entry.message}</span>
              </li>
            ))}
          </ol>
        )}
      </div>
    </>
  );
}
