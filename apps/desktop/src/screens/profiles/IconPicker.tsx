// Icon picker: common icons first, then Lucide search (name + tags, paginated),
// emoji, or an imported image.
import type { Icon } from "@omni/protocol";
import { useDeferredValue, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ControlIcon, Svg } from "../../components/Icon";
import { Button, Modal, Segmented, TextInput } from "../../components/ui";
import { api, dialog } from "../../lib/backend";
import { iconMarkup, loadIconIndex, searchIcons, type IconEntry, type IconName } from "../../lib/icons";
import { toastError } from "../../lib/toast";
import { cx } from "../../lib/util";

type Tab = "icon" | "emoji" | "image";

const PAGE = 96;
const COLS = 8;

const EMOJI = "🎤 🔇 🔊 🔉 🎧 🎵 🎶 ⏯️ ⏭️ ⏮️ ⏹️ ⏺️ 📷 🎥 🎬 📺 🎮 🕹️ 💡 🏠 🔒 🔓 💻 🖥️ ⌨️ 🖱️ 📁 📝 ✉️ 💬 📞 🔔 🔕 ⭐ ❤️ 👍 👋 😀 😂 😎 🤖 👻 💀 🔥 ✅ ❌ ⚠️ ⏰ ☕ 🍕 🌙 ☀️ ⚡ 🚀 🎉 🛠️ 🧪 📈 🗑️ 🎙️".split(" ");

/** Shown before any search, so the full list of 2000+ icons is opt-in. */
const COMMON_ICONS: readonly IconName[] = [
  "mic", "mic-off", "volume-2", "volume-x", "play", "pause", "skip-forward", "skip-back",
  "music", "headphones", "video", "camera", "radio", "monitor", "monitor-off", "lock",
  "power", "house", "lamp-desk", "lightbulb", "keyboard", "globe", "folder", "file",
  "message-circle", "phone", "gamepad-2", "sparkles", "star", "heart", "coffee", "timer",
  "chevron-left", "chevron-right", "arrow-up", "arrow-down", "scissors", "clapperboard", "circle", "square",
  "zap", "bell", "settings", "trash-2", "plus", "check", "x", "smile",
];

const ARROW_STEP: Record<string, number | undefined> = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: COLS, ArrowUp: -COLS };

export function IconPicker({ value, onPick, onClose }: { value: Icon | null; onPick: (icon: Icon | null) => void; onClose: () => void }) {
  const [tab, setTab] = useState<Tab>(value?.type === "emoji" ? "emoji" : value?.type === "image" ? "image" : "icon");
  return (
    <Modal title="Choose icon" width={460} onClose={onClose}>
      <div className="flex items-center gap-2 border-b border-line px-4 py-2.5">
        <Segmented
          label="Icon type"
          value={tab}
          onChange={setTab}
          options={[
            { value: "icon", label: "Icons", icon: "shapes" },
            { value: "emoji", label: "Emoji", icon: "smile" },
            { value: "image", label: "Your image", icon: "image" },
          ]}
        />
        {value && (
          <Button size="sm" variant="ghost" className="ml-auto" onClick={() => onPick(null)}>
            No icon
          </Button>
        )}
      </div>
      {tab === "icon" && <IconSearch selected={value?.type === "builtin" ? value.name : null} onPick={onPick} />}
      {tab === "emoji" && <EmojiTab current={value?.type === "emoji" ? value.value : ""} onPick={onPick} />}
      {tab === "image" && <ImageTab current={value?.type === "image" ? value : null} onPick={onPick} />}
    </Modal>
  );
}

function IconSearch({ selected, onPick }: { selected: string | null; onPick: (icon: Icon) => void }) {
  const [entries, setEntries] = useState<IconEntry[] | null>(null);
  const [query, setQuery] = useState("");
  const [limit, setLimit] = useState(PAGE);
  const [showAll, setShowAll] = useState(false);
  const deferred = useDeferredValue(query);
  const gridRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    void loadIconIndex().then(setEntries);
  }, []);

  const common = !deferred.trim() && !showAll;
  const results = useMemo(() => (common ? COMMON_ICONS : entries ? searchIcons(entries, deferred) : []), [common, entries, deferred]);
  const shown = results.slice(0, limit);
  const pick = (name: string) => onPick({ type: "builtin", name });

  const focusCell = (i: number) => gridRef.current?.querySelectorAll<HTMLButtonElement>("button[data-icon]")[i]?.focus();

  const onGridKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const buttons = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button[data-icon]")];
    const i = buttons.findIndex((b) => b === document.activeElement);
    if (i < 0) return;
    const delta = ARROW_STEP[e.key];
    if (delta === undefined) return;
    e.preventDefault();
    focusCell(Math.max(0, Math.min(buttons.length - 1, i + delta)));
  };

  return (
    <div className="flex flex-col">
      <div className="px-4 pt-3">
        <TextInput
          autoFocus
          data-autofocus
          type="search"
          aria-label="Search icons"
          placeholder={entries ? "Search icons, like music or light" : "Loading icons…"}
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setLimit(PAGE);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              focusCell(0);
            }
            if (e.key === "Enter" && query.trim() && shown[0]) pick(shown[0]);
          }}
        />
      </div>
      <div className="h-[380px] overflow-y-auto">
      <div ref={gridRef} onKeyDown={onGridKey} className="grid grid-cols-8 gap-1 p-4" role="listbox" aria-label="Icons">
        {shown.map((name) => (
          <button
            key={name}
            type="button"
            data-icon
            role="option"
            aria-selected={name === selected}
            title={name}
            onClick={() => pick(name)}
            className={cx(
              "flex aspect-square items-center justify-center rounded-ui-sm hover:bg-hover focus-visible:bg-hover",
              name === selected ? "bg-hover text-accent" : "text-fg",
            )}
          >
            <Svg markup={iconMarkup(name)} className="size-5" />
          </button>
        ))}
      </div>
      {entries && !common && results.length === 0 && (
        <p className="px-4 pb-6 text-center">No icon for "{query}". Try a simpler word, or use Emoji or Your image.</p>
      )}
      {common && (
        <div className="flex justify-center px-4 pb-4">
          <Button size="sm" onClick={() => setShowAll(true)}>
            Show all icons
          </Button>
        </div>
      )}
      {results.length > limit && (
        <div className="flex items-center justify-center gap-3 px-4 pb-4 text-xs text-muted">
          {shown.length} of {results.length}
          <Button size="sm" onClick={() => setLimit((l) => l + PAGE * 2)}>
            Show more
          </Button>
        </div>
      )}
      </div>
    </div>
  );
}

function EmojiTab({ current, onPick }: { current: string; onPick: (icon: Icon) => void }) {
  const [text, setText] = useState(current);
  const pick = (value: string) => value.trim() && onPick({ type: "emoji", value: value.trim() });
  return (
    <div className="flex flex-col gap-3 p-4">
      <div className="flex gap-2">
        <TextInput
          autoFocus
          data-autofocus
          aria-label="Emoji"
          placeholder="Type or paste any emoji"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && pick(text)}
        />
        <Button variant="primary" disabled={!text.trim()} onClick={() => pick(text)}>
          Use
        </Button>
      </div>
      <div className="grid grid-cols-10 gap-1">
        {EMOJI.map((emoji) => (
          <button
            key={emoji}
            type="button"
            onClick={() => pick(emoji)}
            aria-label={emoji}
            className={cx("flex aspect-square items-center justify-center rounded-ui-sm text-xl hover:bg-hover", emoji === current && "bg-hover")}
          >
            {emoji}
          </button>
        ))}
      </div>
    </div>
  );
}

function ImageTab({ current, onPick }: { current: Extract<Icon, { type: "image" }> | null; onPick: (icon: Icon) => void }) {
  const [busy, setBusy] = useState(false);
  const choose = async () => {
    try {
      const path = await dialog.open({ filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "gif", "webp", "svg"] }] });
      if (!path) return;
      setBusy(true);
      onPick({ type: "image", id: await api.importImage(path) });
    } catch (error) {
      toastError(error);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="flex flex-col items-center gap-3 p-6">
      {current && (
        <div className="flex size-20 items-center justify-center rounded-ui border border-line">
          <ControlIcon icon={current} size="64px" />
        </div>
      )}
      <Button variant="primary" icon="upload" disabled={busy} onClick={() => void choose()}>
        {busy ? "Importing…" : "Choose image…"}
      </Button>
      <p className="text-xs text-muted">PNG, JPG, GIF, WebP or SVG. Large images are downscaled.</p>
    </div>
  );
}
