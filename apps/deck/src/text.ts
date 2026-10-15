// Label fitting: measures text with a canvas so labels can shrink / wrap to fit
// without touching the DOM layout.

const FONT_STACK = '-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif';

const MEASURE_SIZE = 100;
const widths: { [text: string]: number } = Object.create(null);
let ctx: CanvasRenderingContext2D | null | undefined;

function context(): CanvasRenderingContext2D | null {
  if (ctx !== undefined) return ctx;
  ctx = null;
  try {
    const c = document.createElement("canvas").getContext("2d");
    if (c) {
      c.font = "700 " + MEASURE_SIZE + "px " + FONT_STACK;
      // If the font string was rejected the default 10px font stays and widths would be 10x off.
      if (c.measureText("M").width > MEASURE_SIZE * 0.4) ctx = c;
    }
  } catch (_e) {
    // no canvas: fall back to the estimate below
  }
  return ctx;
}

/** Width of `text` in bold at 1px font size. */
function unitWidth(text: string): number {
  const cached = widths[text];
  if (cached !== undefined) return cached;
  const c = context();
  const width = c ? c.measureText(text).width / MEASURE_SIZE : text.length * 0.6;
  widths[text] = width;
  return width;
}

/**
 * Real text is wider than the measurement at small sizes: browsers without subpixel
 * positioning round every glyph advance to whole pixels. Budget this much per character.
 */
const GLYPH_SLACK_PX = 0.7;

/** Width of `text` in bold at `size` px, including the per-glyph rounding slack. */
export function textWidth(text: string, size: number): number {
  return unitWidth(text) * size + text.length * GLYPH_SLACK_PX;
}

function widest(lines: string[]): number {
  let max = 0;
  for (let i = 0; i < lines.length; i++) max = Math.max(max, unitWidth(lines[i]));
  return max;
}

/** Largest font size (<= maxSize) at which every line fits `maxWidth`. */
function sizeToFit(lines: string[], maxWidth: number, maxSize: number): number {
  let size = maxSize;
  for (let i = 0; i < lines.length; i++) {
    size = Math.min(size, (maxWidth - lines[i].length * GLYPH_SLACK_PX) / unitWidth(lines[i]));
  }
  return size;
}

/** Splits words into at most `n` lines, minimizing the widest line. */
function balance(words: string[], n: number): string[] {
  let best = [words.join(" ")];
  if (n <= 1 || words.length <= 1) return best;
  let bestWidth = widest(best);
  for (let i = 1; i < words.length; i++) {
    const candidate = [words.slice(0, i).join(" ")].concat(balance(words.slice(i), n - 1));
    const width = widest(candidate);
    if (width < bestWidth) {
      best = candidate;
      bestWidth = width;
    }
  }
  return best;
}

export interface FittedLabel {
  lines: string[];
  /** Font size in px. */
  size: number;
}

/**
 * Picks line breaks and a font size so `text` fits `maxWidth`.
 * Prefers fewer lines unless wrapping makes the text much larger.
 */
export function fitLabel(text: string, maxWidth: number, maxSize: number, minSize: number, maxLines: number): FittedLabel {
  const words = text.replace(/\s+/g, " ").replace(/^ | $/g, "").split(" ");
  let best = balance(words, 1);
  let bestSize = sizeToFit(best, maxWidth, maxSize);
  for (let n = 2; n <= maxLines; n++) {
    const lines = balance(words, n);
    const size = sizeToFit(lines, maxWidth, maxSize);
    if (size > bestSize * 1.3) {
      best = lines;
      bestSize = size;
    }
  }
  return { lines: best, size: Math.max(minSize, Math.floor(bestSize)) };
}

export function lineHeight(size: number): number {
  return Math.round(size * 1.15);
}
