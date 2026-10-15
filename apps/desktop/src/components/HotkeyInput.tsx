// Key recorder producing Tauri accelerator strings ("CmdOrCtrl+Shift+M").
// Click "Record", press the combination; Escape alone cancels.
import { useEffect, useEffectEvent, useState } from "react";
import { formatAccelerator, modifiers, toAccelerator } from "../lib/accelerator";
import { useIsMac } from "../lib/store";
import { cx } from "../lib/util";
import { Button, IconButton, TextInput } from "./ui";

export function HotkeyInput({
  value,
  onChange,
  editable = false,
  placeholder = "None",
  id,
}: {
  value: string;
  onChange: (value: string) => void;
  /** Also allow typing the accelerator by hand (hotkey action). */
  editable?: boolean;
  placeholder?: string;
  id?: string;
}) {
  const mac = useIsMac();
  const [recording, setRecording] = useState(false);
  const [held, setHeld] = useState<string[]>([]);
  const record = useEffectEvent((accelerator: string) => onChange(accelerator));

  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.type === "keyup") return setHeld(modifiers(e, mac));
      const bare = !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey;
      if (e.code === "Escape" && bare) return setRecording(false);
      const accelerator = toAccelerator(e, mac);
      if (!accelerator) return setHeld(modifiers(e, mac));
      record(accelerator);
      setRecording(false);
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("keyup", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("keyup", onKey, true);
      setHeld([]);
    };
  }, [recording, mac]);

  const recorder = (
    <Button
      id={editable ? undefined : id}
      size="md"
      icon={recording ? "circle-dot" : "keyboard"}
      aria-pressed={recording}
      onClick={() => setRecording((r) => !r)}
      onBlur={() => setRecording(false)}
      aria-label={editable ? "Record keys" : undefined}
      title={editable ? "Record keys" : undefined}
      className={cx(recording && "border-accent text-accent", editable ? "w-8 px-0" : "min-w-0 flex-1 justify-start")}
    >
      {editable ? null : (
        <span className={cx("truncate", !value && !recording && "text-muted")}>
          {recording
            ? held.length
              ? `${formatAccelerator(held.join("+"), mac)}…`
              : "Press keys…"
            : value
              ? formatAccelerator(value, mac)
              : placeholder}
        </span>
      )}
    </Button>
  );

  return (
    <div className="flex min-w-0 items-center gap-1.5">
      {editable && (
        <TextInput
          id={id}
          value={recording ? held.map((m) => `${m}+`).join("") : value}
          placeholder={recording ? "Press keys…" : "CmdOrCtrl+Shift+M"}
          readOnly={recording}
          className="font-mono text-xs"
          onChange={(e) => onChange(e.target.value)}
        />
      )}
      {recorder}
      {value && !recording && <IconButton icon="x" label="Clear" onClick={() => onChange("")} />}
    </div>
  );
}
