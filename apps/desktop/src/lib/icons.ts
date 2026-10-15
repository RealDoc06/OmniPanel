// Lucide markup shared with the host and the deck (packages/icons).
// Tags are only needed by the icon picker, so they load on demand.
import icons from "@omni/icons/icons.json";

/** Every Lucide icon name, so UI code can't reference a missing icon. */
export type IconName = keyof typeof icons;

const markup: Record<string, string | undefined> = icons;

export const ICON_NAMES = Object.keys(icons);

/** Inner SVG markup for a 24x24 viewBox, or undefined for unknown names. */
export const iconMarkup = (name: string) => markup[name];

export interface IconEntry {
  name: string;
  /** name + tags, lowercased, for substring search. */
  haystack: string;
}

let index: Promise<IconEntry[]> | null = null;

/** Search index (name + tags), built once on first use. */
export function loadIconIndex(): Promise<IconEntry[]> {
  index ??= import("@omni/icons/tags.json").then(({ default: tags }) => {
    const byName: Record<string, string[] | undefined> = tags;
    return ICON_NAMES.map((name) => ({
      name,
      haystack: `${name} ${(byName[name] ?? []).join(" ")}`.toLowerCase(),
    }));
  });
  return index;
}

/** Icons matching every word of `query`, best name matches first. */
export function searchIcons(entries: readonly IconEntry[], query: string): string[] {
  const q = query.trim().toLowerCase();
  if (!q) return entries.map((e) => e.name);
  const words = q.split(/\s+/);
  const rank = (name: string) => (name === q ? 0 : name.startsWith(q) ? 1 : name.includes(q) ? 2 : 3);
  return entries
    .filter((e) => words.every((w) => e.haystack.includes(w)))
    .map((e) => e.name)
    .sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
}
