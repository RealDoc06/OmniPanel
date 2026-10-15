// Tiny DOM helpers. ES5-safe: no Object.assign, Array.from, for-of.

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Creates an element, optionally with a class and a parent to append to. */
export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  parent?: HTMLElement,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (parent) parent.appendChild(node);
  return node;
}

export function clearChildren(node: HTMLElement): void {
  while (node.firstChild) node.removeChild(node.firstChild);
}

export function clamp(value: number, lo: number, hi: number): number {
  return value < lo ? lo : value > hi ? hi : value;
}

export function px(value: number): string {
  return Math.round(value) + "px";
}

/** Absolutely positions an element at a rect (rounded to whole pixels). */
export function place(node: HTMLElement, r: Rect): void {
  const s = node.style;
  s.left = px(r.x);
  s.top = px(r.y);
  s.width = px(r.w);
  s.height = px(r.h);
}

export function setVisible(node: HTMLElement, visible: boolean): void {
  node.style.display = visible ? "" : "none";
}

/** Compile-time exhaustiveness check for switches over protocol unions. */
export function unreachable(_value: never): void {}

/** Creates a plain string-keyed map without prototype keys like "__proto__". */
export function newMap<T>(): Record<string, T> {
  return Object.create(null) as Record<string, T>;
}

interface PrefixedDocument extends Document {
  webkitHidden?: boolean;
}

/** True while the page is in the background (screen off, other tab). Android 4.4 WebView only has the prefixed flag. */
export function isPageHidden(): boolean {
  return document.hidden || (document as PrefixedDocument).webkitHidden === true;
}

/** Runs `fn` whenever the page is shown or hidden (also fires the prefixed event of old WebViews). */
export function onVisibilityChange(fn: () => void): void {
  document.addEventListener("visibilitychange", fn, false);
  document.addEventListener("webkitvisibilitychange", fn, false);
}
