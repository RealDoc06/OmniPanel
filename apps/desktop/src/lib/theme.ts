// Theme -> CSS variables. index.css maps the variables into Tailwind (`bg-surface`,
// `text-muted`, `rounded-ui`...), so switching themes only rewrites a few variables.
import type { Theme, ThemeColors } from "@omni/protocol";

export const DEFAULT_THEME_ID = "midnight";

/** Built-in "midnight", used when a theme id doesn't resolve. */
export const MIDNIGHT: Theme = {
  id: "midnight",
  name: "Midnight",
  radius: 10,
  colors: {
    background: "#000000",
    surface: "#0b0b0b",
    surfaceHover: "#161616",
    border: "#1f1f1f",
    text: "#ffffff",
    textMuted: "#8c8c8c",
    accent: "#4f8cff",
    accentText: "#ffffff",
    success: "#22c55e",
    warning: "#f59e0b",
    danger: "#ef4444",
    button: "#121212",
    buttonText: "#ffffff",
  },
};

/** Labels for the theme editor, one per color. */
export const COLOR_LABELS: Record<keyof ThemeColors, string> = {
  background: "Background",
  surface: "Surface",
  surfaceHover: "Hover",
  border: "Border",
  text: "Text",
  textMuted: "Muted text",
  accent: "Accent",
  accentText: "Text on accent",
  success: "Success",
  warning: "Warning",
  danger: "Danger",
  button: "Deck button",
  buttonText: "Deck button text",
};

/** Inline style that scopes a theme to an element (deck previews, theme editor). */
export const themeStyle = ({ colors: c, radius }: Theme) => ({
  "--bg": c.background,
  "--surface": c.surface,
  "--hover": c.surfaceHover,
  "--line": c.border,
  "--fg": c.text,
  "--muted": c.textMuted,
  "--accent": c.accent,
  "--on-accent": c.accentText,
  "--success": c.success,
  "--warning": c.warning,
  "--danger": c.danger,
  "--button": c.button,
  "--button-fg": c.buttonText,
  "--radius": `${radius}px`,
  colorScheme: isLight(c.background) ? "light" : "dark",
});

/** Applies a theme to the whole app. */
export function applyTheme(theme: Theme) {
  const root = document.documentElement.style;
  for (const [name, value] of Object.entries(themeStyle(theme))) {
    if (name.startsWith("--")) root.setProperty(name, value);
  }
  root.colorScheme = isLight(theme.colors.background) ? "light" : "dark";
}

export function findTheme(themes: readonly Theme[], id: string | null | undefined): Theme {
  return themes.find((t) => t.id === id) ?? themes.find((t) => t.id === DEFAULT_THEME_ID) ?? MIDNIGHT;
}

/** Relative luminance check for `#rrggbb`. */
export function isLight(hex: string): boolean {
  const n = Number.parseInt(hex.slice(1, 7), 16);
  if (Number.isNaN(n)) return false;
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 150;
}

export const isHexColor = (value: string) => /^#[0-9a-f]{6}$/i.test(value);
