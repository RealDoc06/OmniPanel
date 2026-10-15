// "Describe it": turns a sentence into a control through the configured LLM.
// Generate -> preview -> Apply / Refine (keeps the conversation) / Discard.
import type { Action, ChatMessage, Control, GeneratedControl } from "@omni/protocol";
import { useRef, useState } from "react";
import { DeckControlView } from "../../components/DeckControl";
import { Button, Notice, TextArea } from "../../components/ui";
import { describeAction } from "../../lib/actions";
import { api } from "../../lib/backend";
import { newButton } from "../../lib/controls";
import { iconMarkup } from "../../lib/icons";
import { navigate, useSettings } from "../../lib/store";
import { errorMessage } from "../../lib/util";

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

export function AiAssist({ control, onApply }: { control: Control; onApply: (result: GeneratedControl) => void }) {
  const { ai } = useSettings();
  const configured = ai.apiKey.trim() !== "" || ai.provider === "ollama" || ai.provider === "custom";
  const [prompt, setPrompt] = useState("");
  const [history, setHistory] = useState<ChatMessage[]>([]);
  const [result, setResult] = useState<GeneratedControl | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  if (!configured) {
    return (
      <p className="text-muted">
        Connect an AI provider to build controls from a description.{" "}
        <button type="button" className="text-accent hover:underline" onClick={() => navigate("settings", "ai")}>
          Set up AI
        </button>
      </p>
    );
  }

  const reset = () => {
    setResult(null);
    setHistory([]);
    setPrompt("");
    setError(null);
  };

  const generate = async () => {
    const text = prompt.trim();
    if (!text || busy) return;
    setBusy(true);
    setError(null);
    try {
      const current = control.kind.type === "button" ? control.kind.action : null;
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

  const preview =
    result &&
    newButton(0, 0, result.action, {
      label: result.label,
      icon: result.icon && iconMarkup(result.icon) ? { type: "builtin", name: result.icon } : control.style.icon,
    });

  return (
    <div className="flex flex-col gap-2">
      <TextArea
        ref={inputRef}
        aria-label="Describe the control"
        rows={2}
        readOnly={busy}
        placeholder={result ? "How should it change?" : "Mute Discord and switch OBS to the BRB scene"}
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            void generate();
          }
        }}
      />
      <div className="flex items-center gap-2">
        <Button size="sm" variant="primary" icon="sparkles" disabled={busy || !prompt.trim()} onClick={() => void generate()}>
          {busy ? "Thinking…" : result ? "Refine" : "Generate"}
        </Button>
        <span className="text-xs text-muted">{ai.model}</span>
      </div>
      {error && <Notice tone="error">{error}</Notice>}
      {result && preview && (
        // Refining = typing a follow-up above; the conversation is kept until Apply or Discard.
        <div className="flex flex-col gap-2 rounded-ui-sm border border-line bg-bg p-3">
          <div className="flex items-center gap-3">
            <div className="size-14 shrink-0">
              <DeckControlView control={preview} />
            </div>
            <div className="min-w-0">
              <p className="truncate font-semibold">{result.label}</p>
              <p className="truncate text-xs text-muted">{describeAction(result.action)}</p>
            </div>
          </div>
          <p data-selectable>{result.explanation}</p>
          {scriptsOf(result.action).map((s, i) => (
            <pre key={i} data-selectable className="max-h-40 overflow-auto rounded-ui-sm border border-line p-2 font-mono text-[11px] whitespace-pre-wrap">
              {s.script}
            </pre>
          ))}
          <div className="flex gap-1.5">
            <Button
              size="sm"
              variant="primary"
              icon="check"
              onClick={() => {
                onApply(result);
                reset();
              }}
            >
              Apply
            </Button>
            <Button size="sm" variant="ghost" onClick={reset}>
              Discard
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
