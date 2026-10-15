// Recent host log events (hotkey registration failures, action errors...).
import type { LogLevel } from "@omni/protocol";
import { useState } from "react";
import { Button, EmptyState, ScreenHeader, Segmented } from "../../components/ui";
import { clearLogs, useApp } from "../../lib/store";
import { cx } from "../../lib/util";

type Filter = "all" | LogLevel;

const LEVEL_CLASS: Record<LogLevel, string> = { info: "text-muted", warn: "text-warning", error: "text-danger" };

const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });

export function LogScreen() {
  const logs = useApp((s) => s.logs);
  const [filter, setFilter] = useState<Filter>("all");
  const shown = filter === "all" ? logs : logs.filter((l) => l.level === filter);
  return (
    <>
      <ScreenHeader title="Log">
        <Segmented
          label="Level"
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: "All" },
            { value: "info", label: "Info" },
            { value: "warn", label: "Warnings" },
            { value: "error", label: "Errors" },
          ]}
        />
        <Button size="sm" icon="trash-2" disabled={logs.length === 0} onClick={clearLogs}>
          Clear
        </Button>
      </ScreenHeader>
      <div className="flex-1 overflow-y-auto">
        {shown.length === 0 ? (
          <EmptyState icon="scroll-text" title={logs.length ? "Nothing at this level." : "Nothing logged since OmniPanel opened."} />
        ) : (
          <ol data-selectable className="font-mono text-xs">
            {shown.map((entry, i) => (
              <li key={`${entry.at}-${i}`} className="grid grid-cols-[72px_48px_1fr] gap-3 border-b border-line px-6 py-1.5">
                <span className="text-muted tabular-nums">{time(entry.at)}</span>
                <span className={cx("uppercase", LEVEL_CLASS[entry.level])}>{entry.level}</span>
                <span className="break-words whitespace-pre-wrap text-fg">{entry.message}</span>
              </li>
            ))}
          </ol>
        )}
      </div>
    </>
  );
}
