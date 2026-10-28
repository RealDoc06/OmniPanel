// "Ask AI" inside the action picker: turns a sentence into a button through the
// configured AI service. Build it -> preview -> Use this / Change it (keeps the
// conversation) / Cancel.
import type { Action, AiSettings, ChatMessage, GeneratedControl, Icon } from "@omni/protocol";
import { useRef, useState } from "react";
import { DeckControlView } from "../../components/DeckControl";
import { Button, Notice, TextArea } from "../../components/ui";
import { describeAction } from "../../lib/actions";
import { api } from "../../lib/backend";
import { newButton } from "../../lib/controls";
import { iconMarkup } from "../../lib/icons";
import type { PickResult } from "../../lib/recipes";
import { useVoicePresets } from "../../lib/store";
import { errorMessage } from "../../lib/util";

/** Whether the AI service has what it needs (Ollama and custom servers may run without a key). */
export const aiConfigured = (ai: AiSettings) => ai.apiKey.trim() !== "" || ai.provider === "ollama" || ai.provider === "custom";

/** Every script inside an action (multi steps and toggle branches included). */
function scriptsOf(action: Action): Extract<Action, { type: "runScript" }>[] {
  switch (action.type) {
    case "runScript":
      return [action];
    case "multi":
      return action.steps.flatMap(scriptsOf);
    case "toggle":
      return [...scriptsOf(action.on), ...scriptsOf(action.off)];
    default:
      return [];
  }
}

export function AiAssist({
  initialPrompt,
  current,
  fallbackIcon,
  onUse,
  onCancel,
}: {
  initialPrompt: string;
  /** Action already on the button, so the AI can edit it. */
  current: Action | null;
  /** Icon kept when the AI suggests none (or one that doesn't exist). */
  fallbackIcon: Icon | null;
  onUse: (result: PickResult) => void;
  onCancel: () => void;
}) {
  const presets = useVoicePresets();
  const names = { preset: (id: string) => presets.find((p) => p.id === id)?.name };
  const [prompt, setPrompt] = useState(initialPrompt);
  const [history, setHistory] = useState<ChatMessage[]>([]);
  const [result, setResult] = useState<GeneratedControl | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const build = async () => {
    const text = prompt.trim();
    if (!text || busy) return;
    setBusy(true);
    setError(null);
    try {
      const next = await api.aiGenerate({ prompt: text, history, current });
      setHistory((h) => [...h, { role: "user", content: text }, { role: "assistant", content: JSON.stringify(next) }]);
      setResult(next);
      setPrompt("");
      inputRef.current?.focus();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const pick: PickResult | null = result && {
    kind: { type: "button", action: result.action },
    style: {
      label: result.label,
      icon: result.icon && iconMarkup(result.icon) ? { type: "builtin", name: result.icon } : fallbackIcon,
      activeLabel: null,
      activeIcon: null,
      activeBg: null,
    },
  };
  const steps = result?.action.type === "multi" ? result.action.steps : [];

  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      <TextArea
        ref={inputRef}
        autoFocus
        aria-label="Describe the button"
        rows={2}
        readOnly={busy}
        placeholder={result ? "How should it change?" : "Describe the button, like: mute Discord and switch OBS to BRB"}
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            void build();
          } else if (e.key === "Escape") {
            e.preventDefault();
            onCancel();
          }
        }}
      />
      {!result && (
        <div className="flex gap-1.5">
          <Button size="sm" variant="primary" icon="sparkles" disabled={busy || !prompt.trim()} onClick={() => void build()}>
            {busy ? "Building…" : "Build it"}
          </Button>
          <Button size="sm" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      )}
      {error && <Notice tone="error">{error}</Notice>}
      {result && pick && (
        <div className="flex flex-col gap-2 border-t border-line pt-2">
          <div className="flex items-center gap-3">
            <div className="size-14 shrink-0">
              <DeckControlView control={newButton(0, 0, result.action, pick.style)} />
            </div>
            <div className="min-w-0">
              <p className="truncate font-semibold">{result.label}</p>
              <p className="truncate">{describeAction(result.action, names)}</p>
            </div>
          </div>
          {steps.length > 0 && (
            <ol className="flex flex-col gap-0.5">
              {steps.map((step, i) => (
                <li key={i} className="truncate">
                  {i + 1}. {describeAction(step, names)}
                </li>
              ))}
            </ol>
          )}
          <p data-selectable>{result.explanation}</p>
          {scriptsOf(result.action).map((s, i) => (
            <pre key={i} data-selectable className="max-h-40 overflow-auto border-l border-line pl-2 font-mono text-[11px] whitespace-pre-wrap">
              {s.script}
            </pre>
          ))}
          <div className="flex gap-1.5">
            <Button size="sm" variant="primary" icon="check" onClick={() => onUse(pick)}>
              Use this
            </Button>
            <Button size="sm" disabled={busy || !prompt.trim()} onClick={() => void build()}>
              {busy ? "Building…" : "Change it"}
            </Button>
            <Button size="sm" variant="ghost" onClick={onCancel}>
              Cancel
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
