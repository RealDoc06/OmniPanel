// Key recorder producing accelerator strings ("CmdOrCtrl+Shift+M"). Click the button,
// press the combination; Escape alone cancels.
// - "send" (the Press keys action): [Record keys], plus "Type them instead" with a text
//   field that only saves keys `isAccelerator` accepts.
// - "trigger" (a button's Keyboard shortcut): [Record shortcut], record only.
import { useEffect, useEffectEvent, useState } from "react";
import { formatAccelerator, isAccelerator, isModifierKey, toAccelerator } from "../lib/accelerator";
import { useIsMac } from "../lib/store";
import { cx } from "../lib/util";
import { Button, Disclosure, TextInput } from "./ui";

export function HotkeyInput({
  value,
  onChange,
  variant,
  id,
}: {
  value: string;
  onChange: (value: string) => void;
  variant: "send" | "trigger";
  id?: string;
}) {
  const mac = useIsMac();
  const [recording, setRecording] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const record = useEffectEvent((accelerator: string) => onChange(accelerator));

  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const bare = !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey;
      if (e.code === "Escape" && bare) return setRecording(false);
      const accelerator = toAccelerator(e, mac);
      if (!accelerator) {
        if (!isModifierKey(e.code)) setError("This key isn't supported. Try a different key.");
        return;
      }
      setError(null);
      record(accelerator);
      setRecording(false);
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
    };
  }, [recording, mac]);

  const text = recording
    ? "Press keys… (Esc cancels)"
    : value
      ? formatAccelerator(value, mac)
      : variant === "send"
        ? "Record keys"
        : "Record shortcut";

  return (
    <div className="flex min-w-0 flex-col gap-1">
      <div className="flex min-w-0 items-center gap-1.5">
        <Button
          id={id}
          variant={variant === "send" && !value && !recording ? "primary" : "default"}
          aria-pressed={recording}
          title={value && !recording ? "Click to record again" : undefined}
          onClick={() => {
            setError(null);
            setRecording((r) => !r);
          }}
          onBlur={() => setRecording(false)}
          className={cx("min-w-0", recording && "border-accent text-accent")}
        >
          <span className="truncate">{text}</span>
        </Button>
        {value && !recording && (
          <Button size="sm" variant="ghost" onClick={() => onChange("")}>
            Clear
          </Button>
        )}
      </div>
      {error && <p role="alert" className="text-xs text-danger">{error}</p>}
      {variant === "send" && <TypedKeys value={value} onChange={onChange} />}
    </div>
  );
}

/**
 * Hand-typed keys. Text that isn't a valid accelerator stays local (with an error)
 * and is never saved; a change from outside (recording, undo) replaces it.
 */
function TypedKeys({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const mac = useIsMac();
  // `base` is the saved value the draft belongs to.
  const [draft, setDraft] = useState({ text: value, base: value });
  if (draft.base !== value) setDraft({ text: value, base: value });
  const text = draft.base === value ? draft.text : value;
  const invalid = text.trim() !== "" && !isAccelerator(text, mac);
  return (
    <Disclosure label="Type them instead" defaultOpen={invalid}>
      <div className="flex flex-col gap-1 pb-1">
        <TextInput
          aria-label="Keys"
          aria-invalid={invalid}
          placeholder="CmdOrCtrl+Shift+M"
          className={cx("font-mono text-xs", invalid && "border-danger")}
          value={text}
          onChange={(e) => {
            const next = e.target.value;
            const valid = next.trim() === "" || isAccelerator(next, mac);
            setDraft({ text: next, base: valid ? next.trim() : value });
            if (valid && next.trim() !== value) onChange(next.trim());
          }}
        />
        {invalid ? (
          <p className="text-xs text-danger">These keys aren't supported on this computer. Try Record keys.</p>
        ) : (
          <p className="text-xs text-muted">Use + between keys. Modifiers: CmdOrCtrl, Ctrl, Alt, Shift, Super.</p>
        )}
      </div>
    </Disclosure>
  );
}
