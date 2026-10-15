// Small helpers shared across the UI.

const ID_CHARS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/** Random id with the same shape as `omni_core::new_id` (10 alphanumerics). */
export function newId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(10));
  return Array.from(bytes, (b) => ID_CHARS.charAt(b % ID_CHARS.length)).join("");
}

export const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

/** Joins truthy class names. */
export const cx = (...parts: Array<string | false | null | undefined>) => parts.filter(Boolean).join(" ");

/** Tauri commands reject with the Rust error string; everything else is an Error. */
export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return JSON.stringify(error) ?? "Unknown error";
}

/** Returns a copy with the item at `from` moved to `to`. */
export function moveItem<T>(items: readonly T[], from: number, to: number): T[] {
  const next = [...items];
  const [item] = next.splice(from, 1);
  if (item === undefined) return next;
  next.splice(clamp(to, 0, next.length), 0, item);
  return next;
}

/** Replaces the item with the same id, or appends it. */
export function upsert<T extends { id: string }>(items: readonly T[], item: T): T[] {
  return items.some((i) => i.id === item.id) ? items.map((i) => (i.id === item.id ? item : i)) : [...items, item];
}

/** "Copy of X" style name that doesn't collide with `taken`. */
export function uniqueName(base: string, taken: readonly string[]): string {
  if (!taken.includes(base)) return base;
  for (let n = 2; ; n++) {
    const name = `${base} ${n}`;
    if (!taken.includes(name)) return name;
  }
}

/** "just now", "5 min ago", "3 h ago", "2 d ago". */
export function relativeTime(ms: number, now = Date.now()): string {
  if (!ms) return "never";
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

export const isMacPlatform = () => /mac/i.test(navigator.platform || navigator.userAgent);

