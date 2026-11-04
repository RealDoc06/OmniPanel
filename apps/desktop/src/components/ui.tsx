// Form and layout primitives. Flat and dense: hairline borders, no card chrome.
import { useEffect, useId, useRef, useState, type ComponentProps, type ReactNode } from "react";
import type { IconName } from "../lib/icons";
import { isHexColor } from "../lib/theme";
import { clamp, cx } from "../lib/util";
import { I } from "./Icon";

// ---------------------------------------------------------------------------
// Buttons

const BUTTON_VARIANTS = {
  default: "btn-default",
  primary: "btn-primary",
  danger: "btn-danger",
  destructive: "btn-destructive",
  ghost: "btn-ghost",
};

const BUTTON_SIZES = { sm: "btn-sm", md: "" };

interface ButtonProps extends ComponentProps<"button"> {
  variant?: keyof typeof BUTTON_VARIANTS;
  size?: keyof typeof BUTTON_SIZES;
  icon?: IconName;
}

export function Button({ variant = "default", size = "md", icon, className, children, type = "button", ...rest }: ButtonProps) {
  return (
    <button
      type={type}
      className={cx(
        "btn",
        BUTTON_VARIANTS[variant],
        BUTTON_SIZES[size],
        className,
      )}
      {...rest}
    >
      {icon && <I name={icon} className={size === "sm" ? "size-3.5" : "size-4"} />}
      {children}
    </button>
  );
}

interface IconButtonProps extends ComponentProps<"button"> {
  icon: IconName;
  /** Accessible name, also shown as tooltip. */
  label: string;
  active?: boolean;
  tone?: "default" | "danger";
}

export function IconButton({ icon, label, active, tone = "default", className, type = "button", ...rest }: IconButtonProps) {
  return (
    <button
      type={type}
      title={label}
      aria-label={label}
      aria-pressed={active}
      className={cx("icon-btn", active ? "text-accent" : tone === "danger" && "text-danger", className)}
      {...rest}
    >
      <I name={icon} className="size-4" />
    </button>
  );
}

// ---------------------------------------------------------------------------
// Inputs

/** Base input look (index.css `.field`); utilities in `className` override it. */
export const inputClass = "field";

export function TextInput({ className, ...rest }: ComponentProps<"input">) {
  return <input type="text" spellCheck={false} autoComplete="off" className={cx(inputClass, className)} {...rest} />;
}

export function TextArea({ className, ...rest }: ComponentProps<"textarea">) {
  return (
    <textarea
      spellCheck={false}
      className={cx("field", className)}
      {...rest}
    />
  );
}

/** Number input that commits on blur / Enter, clamped to `min`..`max`. */
export function NumberField({
  value,
  onChange,
  min = -Infinity,
  max = Infinity,
  step = 1,
  className,
  id,
  suffix,
  ariaLabel,
}: {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  className?: string;
  id?: string;
  suffix?: string;
  ariaLabel?: string;
}) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  const commit = () => {
    const n = Number(draft);
    if (draft.trim() === "" || Number.isNaN(n)) return setDraft(String(value));
    const next = clamp(step >= 1 ? Math.round(n) : n, min, max);
    setDraft(String(next));
    if (next !== value) onChange(next);
  };
  return (
    <div className={cx("relative flex items-center", className)}>
      <input
        id={id}
        type="number"
        inputMode="decimal"
        aria-label={ariaLabel}
        className={cx(inputClass, "tabular-nums", suffix && "pr-8")}
        value={draft}
        min={Number.isFinite(min) ? min : undefined}
        max={Number.isFinite(max) ? max : undefined}
        step={step}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
      />
      {suffix && <span className="pointer-events-none absolute right-2 text-xs text-muted">{suffix}</span>}
    </div>
  );
}

export interface Option {
  value: string;
  label: string;
  disabled?: boolean;
}

/** Native select over string options. */
export function Select({
  value,
  options,
  onChange,
  className,
  id,
  ariaLabel,
  placeholder,
}: {
  value: string;
  options: readonly Option[];
  onChange: (value: string) => void;
  className?: string;
  id?: string;
  ariaLabel?: string;
  /** Shown (disabled) when `value` matches no option. */
  placeholder?: string;
}) {
  const known = options.some((o) => o.value === value);
  return (
    <select
      id={id}
      aria-label={ariaLabel}
      value={known ? value : "__placeholder"}
      onChange={(e) => onChange(e.target.value)}
      className={cx(inputClass, className)}
    >
      {!known && (
        <option value="__placeholder" disabled>
          {placeholder ?? (value || "Choose…")}
        </option>
      )}
      {options.map((o) => (
        <option key={o.value} value={o.value} disabled={o.disabled}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

/** Select over a string union, typed by its label record. */
export function EnumSelect<T extends string>({
  value,
  labels,
  onChange,
  className,
  id,
  ariaLabel,
  only,
}: {
  value: T;
  labels: Record<T, string>;
  onChange: (value: T) => void;
  className?: string;
  id?: string;
  ariaLabel?: string;
  /** Restrict to these values (display order follows `labels`). */
  only?: readonly T[];
}) {
  const isKey = (k: string): k is T => Object.hasOwn(labels, k);
  const options = Object.entries<string>(labels)
    .filter(([k]) => !only || (isKey(k) && only.includes(k)))
    .map(([k, label]) => ({ value: k, label }));
  return (
    <Select
      id={id}
      ariaLabel={ariaLabel}
      className={className}
      value={value}
      options={options}
      onChange={(v) => isKey(v) && onChange(v)}
    />
  );
}

/** Text input with native suggestions (free text stays allowed). */
export function ComboInput({
  value,
  onChange,
  options,
  placeholder,
  id,
  ariaLabel,
  className,
}: {
  value: string;
  onChange: (value: string) => void;
  options: readonly Option[];
  placeholder?: string;
  id?: string;
  ariaLabel?: string;
  className?: string;
}) {
  const listId = useId();
  return (
    <>
      <TextInput
        id={id}
        aria-label={ariaLabel}
        list={listId}
        value={value}
        placeholder={placeholder}
        className={className}
        onChange={(e) => onChange(e.target.value)}
      />
      <datalist id={listId}>
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label !== o.value ? o.label : undefined}
          </option>
        ))}
      </datalist>
    </>
  );
}

export function Switch({
  checked,
  onChange,
  label,
  disabled,
  id,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
  id?: string;
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cx(
        "relative h-4 w-7 shrink-0 rounded-full border disabled:opacity-40",
        checked ? "border-accent bg-accent" : "border-line bg-hover",
      )}
    >
      <span
        className={cx(
          "absolute top-[1px] size-3 rounded-full",
          checked ? "left-[13px] bg-on-accent" : "left-[1px] bg-muted",
        )}
      />
    </button>
  );
}

/** Horizontal slider with its value shown on the right. */
export function Range({
  value,
  onChange,
  min,
  max,
  step = 0.01,
  format = (v) => String(Math.round(v * 100) / 100),
  label,
  id,
  className,
  disabled,
}: {
  value: number;
  onChange: (value: number) => void;
  min: number;
  max: number;
  step?: number;
  format?: (value: number) => string;
  label: string;
  id?: string;
  className?: string;
  disabled?: boolean;
}) {
  return (
    <div className={cx("flex min-w-0 items-center gap-2", className)}>
      <input
        id={id}
        type="range"
        aria-label={label}
        className="h-4 min-w-0 flex-1"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <span className="w-14 shrink-0 text-right text-xs text-muted tabular-nums">{format(value)}</span>
    </div>
  );
}

/** Color swatch + hex input. `null` means "theme default": the swatch shows `fallback` and the
 * hex field is empty with a "Default" placeholder. Only complete hex colors are committed. */
export function ColorField({
  value,
  fallback,
  onChange,
  label,
  resettable = true,
}: {
  value: string | null;
  fallback: string;
  onChange: (value: string | null) => void;
  label: string;
  resettable?: boolean;
}) {
  const shown = value ?? fallback;
  const [draft, setDraft] = useState(value ?? "");
  useEffect(() => setDraft(value ?? ""), [value]);
  return (
    <div className="flex min-w-0 items-center gap-1.5">
      <input
        type="color"
        aria-label={label}
        value={isHexColor(shown) ? shown : "#000000"}
        onChange={(e) => onChange(e.target.value)}
        className="h-7 w-8 shrink-0 rounded-ui-sm"
      />
      <input
        type="text"
        aria-label={`${label} hex`}
        spellCheck={false}
        value={draft}
        placeholder="Default"
        onChange={(e) => {
          setDraft(e.target.value);
          if (isHexColor(e.target.value)) onChange(e.target.value.toLowerCase());
        }}
        onBlur={() => setDraft(value ?? "")}
        className={cx(inputClass, "h-7 w-[84px] font-mono text-xs")}
      />
      {resettable && value !== null && (
        <Button size="sm" variant="ghost" aria-label={`Reset ${label.toLowerCase()}`} onClick={() => onChange(null)}>
          Reset
        </Button>
      )}
    </div>
  );
}

/** Small segmented control for 2-4 choices. */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: readonly { value: T; label: string; icon?: IconName }[];
  onChange: (value: T) => void;
  label: string;
}) {
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex h-7 w-fit shrink-0 rounded-ui-sm border border-line p-0.5">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          onClick={() => onChange(o.value)}
          className={cx(
            "inline-flex items-center gap-1 rounded-[calc(var(--radius)*0.45)] px-2.5 text-xs font-medium",
            o.value === value ? "bg-hover text-fg" : "text-muted hover:text-fg",
          )}
        >
          {o.icon && <I name={o.icon} className="size-3.5" />}
          {o.label}
        </button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Layout

/** Label + control row used by settings-style forms. */
export function Field({
  label,
  hint,
  children,
  htmlFor,
  compact,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
  htmlFor?: string;
  /** Narrow label column (inspector, two-column sections). */
  compact?: boolean;
}) {
  return (
    <div className={cx("grid items-start gap-x-4 py-0.5", compact ? "grid-cols-[84px_minmax(0,1fr)] @max-[320px]:grid-cols-1" : "grid-cols-[160px_minmax(0,1fr)]")}>
      <label htmlFor={htmlFor} className="flex min-h-7 items-center leading-tight text-muted">
        {label}
      </label>
      <div className="min-w-0">
        <div className="grid min-h-7 grid-cols-[minmax(0,1fr)] items-center">{children}</div>
        {hint && <div className="mt-1 text-xs text-muted">{hint}</div>}
      </div>
    </div>
  );
}

export function Section({ title, actions, children, id }: { title: string; actions?: ReactNode; children: ReactNode; id?: string }) {
  return (
    <section id={id} className="border-b border-line px-7 py-5">
      <div className="mb-3 flex min-h-7 items-center gap-2">
        <h2 className="text-[15px] font-semibold tracking-[-0.01em]">{title}</h2>
        <div className="ml-auto flex items-center gap-2">{actions}</div>
      </div>
      {children}
    </section>
  );
}

/** Collapsible inspector group: bold title row with a chevron, hairline below. `padded: false`
 * lets the content manage its own horizontal padding (the action picker's sticky search). */
export function Group({ title, children, open = true, aside, padded = true }: { title: string; children: ReactNode; open?: boolean; aside?: ReactNode; padded?: boolean }) {
  return (
    <details open={open} className="group border-b border-line">
      <summary className="flex h-10 items-center gap-1.5 px-5 font-semibold hover:bg-hover">
        {title}
        <span className="ml-auto text-xs font-normal text-muted">{aside}</span>
        <I name="chevron-down" className="size-3.5 text-muted group-open:rotate-180" />
      </summary>
      <div className={cx("flex flex-col pb-3", padded && "px-5")}>{children}</div>
    </details>
  );
}

export function ScreenHeader({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <header data-tauri-drag-region className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-7">
      <h1 className="text-[16px] font-semibold tracking-[-0.01em]">{title}</h1>
      <div className="ml-auto flex items-center gap-2">{children}</div>
    </header>
  );
}

export function EmptyState({ icon, title, children }: { icon: IconName; title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-4 px-6 py-20 text-center">
      <I name={icon} className="size-6 text-muted" />
      <p className="max-w-sm text-[15px] text-fg">{title}</p>
      {children}
    </div>
  );
}

const DOT_TONES = { ok: "bg-success", warn: "bg-warning", error: "bg-danger", off: "bg-muted/50" };

export function StatusDot({ tone, label }: { tone: keyof typeof DOT_TONES; label?: string }) {
  return <span role={label ? "img" : undefined} aria-label={label} className={cx("inline-block size-2 shrink-0 rounded-full", DOT_TONES[tone])} />;
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="rounded border border-line px-1 font-sans text-[11px] text-muted">{children}</kbd>;
}

/** Inline notice (errors, hints): icon plus tone-colored text, no box. */
export function Notice({ tone = "info", children }: { tone?: "info" | "warn" | "error"; children: ReactNode }) {
  return (
    <div
      role={tone === "error" ? "alert" : undefined}
      className={cx("flex gap-2 text-[12.5px]", tone === "error" ? "text-danger" : tone === "warn" ? "text-warning" : "text-fg")}
    >
      <I name={tone === "info" ? "info" : "triangle-alert"} className="mt-px size-4 shrink-0" />
      <div className="min-w-0 flex-1" data-selectable>
        {children}
      </div>
    </div>
  );
}

/** Collapsible block on a native <details>: chevron and label above a hairline, no box.
 * `aside` sits right-aligned on the label row. Uncontrolled; `defaultOpen` only sets the first state. */
export function Disclosure({
  label,
  defaultOpen = false,
  aside,
  children,
}: {
  label: string;
  defaultOpen?: boolean;
  aside?: ReactNode;
  children: ReactNode;
}) {
  return (
    <details open={defaultOpen} className="border-t border-line">
      <summary className="flex min-h-8 items-center gap-1.5 text-muted hover:text-fg">
        {/* Rotates only for its own <details>, so nested disclosures don't follow an open parent. */}
        <I name="chevron-right" className="size-3.5 shrink-0 [[open]>summary>&]:rotate-90" />
        <span className="min-w-0 flex-1 truncate">{label}</span>
        {aside && <span className="shrink-0 text-xs text-muted">{aside}</span>}
      </summary>
      <div className="pb-2">{children}</div>
    </details>
  );
}

// ---------------------------------------------------------------------------
// Modal

/** Native modal <dialog>: focus trap, Escape and top layer for free. Mount it to open it.
 * The first `[data-autofocus]` element inside gets focus. */
export function Modal({
  title,
  onClose,
  children,
  width = 520,
  footer,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  width?: number;
  footer?: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    if (!dialog || dialog.open) return;
    dialog.showModal();
    // React's autoFocus runs before the dialog is shown; mark the target with data-autofocus.
    dialog.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, []);
  return (
    <dialog
      ref={ref}
      onClose={onClose}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      onMouseDown={(e) => e.target === ref.current && onClose()}
      style={{ width: `min(${width}px, calc(100vw - 48px))` }}
      className="max-h-[calc(100vh-48px)] overflow-hidden rounded-ui border border-line bg-surface p-0 text-fg"
    >
      <div className="flex max-h-[calc(100vh-50px)] flex-col">
        <div className="flex h-11 shrink-0 items-center border-b border-line pr-2 pl-4">
          <h2 className="text-[14px] font-semibold">{title}</h2>
          <IconButton icon="x" label="Close" className="ml-auto" onClick={onClose} />
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto">{children}</div>
        {footer && <div className="flex shrink-0 items-center justify-end gap-2 border-t border-line px-4 py-2.5">{footer}</div>}
      </div>
    </dialog>
  );
}
