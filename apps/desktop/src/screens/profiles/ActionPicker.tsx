// Chooses what a button does: a search field over task-named recipes ("Mute / unmute
// microphone"), grouped while the query is empty and ranked while searching, with
// "Ask AI" always last. Used for empty squares (current = null) and for "Change".
// Keyboard: Up / Down move, Enter picks, Escape cancels.
import type { ConnState, Control } from "@omni/protocol";
import { useEffect, useId, useMemo, useState, type ReactNode } from "react";
import { I } from "../../components/Icon";
import { Button } from "../../components/ui";
import type { ActionGroup } from "../../lib/actions";
import { api } from "../../lib/backend";
import { useResource } from "../../lib/hooks";
import { dynamicRecipes, pickOf, searchRecipes, type PickResult, type Recipe } from "../../lib/recipes";
import { navigate, useRuntime, useSettings } from "../../lib/store";
import { cx } from "../../lib/util";
import { AiAssist, aiConfigured } from "./AiAssist";

export function ActionPicker({
  current,
  onPick,
  onCancel,
}: {
  current: Control | null;
  onPick: (result: PickResult) => void;
  onCancel: () => void;
}) {
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const [asking, setAsking] = useState(false);
  const installed = useResource(() => api.installedApps());
  const mixer = useResource(() => api.getMixer());
  const dynamic = useMemo(() => dynamicRecipes(installed.data ?? [], mixer.data?.apps ?? []), [installed.data, mixer.data]);
  const results = useMemo(() => searchRecipes(query, dynamic), [query, dynamic]);
  const status = useGroupStatus();
  const aiReady = aiConfigured(useSettings().ai);
  const listId = useId();
  const optionId = (i: number) => `${listId}-${i}`;

  useEffect(() => {
    document.getElementById(`${listId}-${index}`)?.scrollIntoView({ block: "nearest" });
  }, [listId, index]);

  if (asking) {
    return (
      <AiAssist
        initialPrompt={query.trim()}
        current={current?.kind.type === "button" ? current.kind.action : null}
        fallbackIcon={current?.style.icon ?? null}
        onUse={onPick}
        onCancel={() => setAsking(false)}
      />
    );
  }

  // The AI row sits after the recipes, at index `results.length`.
  const choose = (i: number) => {
    const recipe = results[i];
    if (recipe) onPick(pickOf(recipe));
    else if (aiReady) setAsking(true);
    else navigate("settings", "ai");
  };
  const grouped = query.trim() === "";
  const aiLabel = !aiReady ? "Ask AI (set up first)" : query.trim() ? `Ask AI: "${query.trim()}"` : "Ask AI";

  return (
    <div className="flex flex-col pb-3">
      <div className="sticky top-0 z-10 bg-bg px-5 pt-1 pb-2">
        <input
          type="search"
          role="combobox"
          aria-label="Search what it does"
          aria-expanded
          aria-controls={listId}
          aria-activedescendant={optionId(index)}
          autoFocus
          spellCheck={false}
          autoComplete="off"
          className="field"
          placeholder="Search, like: mute my mic, Spotify, next page"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setIndex(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              const step = e.key === "ArrowDown" ? 1 : -1;
              setIndex((i) => (i + step + results.length + 1) % (results.length + 1));
            } else if (e.key === "Enter") {
              e.preventDefault();
              choose(index);
            } else if (e.key === "Escape") {
              e.preventDefault();
              onCancel();
            }
          }}
        />
      </div>
      <div id={listId} role="listbox" aria-label="What it does">
        {results.map((recipe, i) => (
          <div key={recipe.id} role="presentation">
            {grouped && recipe.group !== results[i - 1]?.group && (
              <div role="presentation" className="flex h-8 items-center gap-2 px-5 pt-2 text-xs font-semibold text-muted">
                {recipe.group}
                <span className="ml-auto text-xs font-normal">{status[recipe.group]}</span>
              </div>
            )}
            <Row id={optionId(i)} icon={recipe.icon} active={i === index} onHover={() => setIndex(i)} onPick={() => choose(i)}>
              {recipe.label}
            </Row>
          </div>
        ))}
        <div className="mt-1 border-t border-line pt-1">
          <Row
            id={optionId(results.length)}
            icon="sparkles"
            active={index === results.length}
            onHover={() => setIndex(results.length)}
            onPick={() => choose(results.length)}
          >
            {aiLabel}
          </Row>
        </div>
      </div>
      <div className="flex justify-end px-5 pt-2">
        <Button size="sm" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

function Row({
  id,
  icon,
  active,
  onHover,
  onPick,
  children,
}: {
  id: string;
  icon: Recipe["icon"];
  active: boolean;
  onHover: () => void;
  onPick: () => void;
  children: ReactNode;
}) {
  return (
    <div
      id={id}
      role="option"
      aria-selected={active}
      onMouseMove={active ? undefined : onHover}
      // Keep focus in the search field so the keyboard keeps working.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onPick}
      className={cx("flex h-7 cursor-default items-center gap-2.5 px-5", active && "bg-hover")}
    >
      <I name={icon} className="size-4 shrink-0" />
      <span className="min-w-0 truncate">{children}</span>
    </div>
  );
}

/** Right-aligned connection status for the groups that need setup first. */
function useGroupStatus(): Partial<Record<ActionGroup, ReactNode>> {
  const runtime = useRuntime();
  const voiceReady = useSettings().voice.outputDevice !== null;
  const connection = (state: ConnState, anchor: string) =>
    state === "connected" ? <span className="text-success">Connected</span> : <SetUp text="Not connected" onClick={() => navigate("settings", anchor)} />;
  return {
    "OBS Studio": connection(runtime.obs.state, "obs"),
    "Home Assistant": connection(runtime.homeAssistant.state, "ha"),
    "Voice changer": voiceReady ? null : <SetUp text="Needs setup" onClick={() => navigate("sound", "voice")} />,
  };
}

function SetUp({ text, onClick }: { text: string; onClick: () => void }) {
  return (
    <span className="text-muted">
      {text} ·{" "}
      <button type="button" className="text-accent hover:underline" onMouseDown={(e) => e.preventDefault()} onClick={onClick}>
        Set up
      </button>
    </span>
  );
}
