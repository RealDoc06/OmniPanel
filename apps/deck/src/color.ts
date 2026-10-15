// Color helpers for `#rgb` / `#rrggbb` strings.

function parseHex(color: string): [number, number, number] | null {
  let hex = color.charAt(0) === "#" ? color.slice(1) : color;
  if (hex.length === 3) hex = hex.charAt(0) + hex.charAt(0) + hex.charAt(1) + hex.charAt(1) + hex.charAt(2) + hex.charAt(2);
  if (!/^[0-9a-fA-F]{6}/.test(hex)) return null;
  return [parseInt(hex.slice(0, 2), 16), parseInt(hex.slice(2, 4), 16), parseInt(hex.slice(4, 6), 16)];
}

function channel(v: number): number {
  const s = v / 255;
  return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

/** WCAG relative luminance, 0 (black) to 1 (white). Unparseable colors count as black. */
function luminance(color: string): number {
  const rgb = parseHex(color);
  if (!rgb) return 0;
  return 0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2]);
}

function contrast(a: string, b: string): number {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** Returns `fg` if it stays readable on `bg`, otherwise black or white, whichever reads better. */
export function readableOn(bg: string, fg: string): string {
  if (contrast(bg, fg) >= 3) return fg;
  return contrast(bg, "#000000") >= contrast(bg, "#ffffff") ? "#000000" : "#ffffff";
}
