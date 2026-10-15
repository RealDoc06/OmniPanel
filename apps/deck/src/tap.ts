// Tap handling for chrome (keypad, status bar, settings): fires on touchend so there is
// no 300 ms click delay, and on `click` for mouse / screen readers. Doesn't block scrolling.

const SLOP = 10;
/** Browsers fire a synthetic click after a touch; ignore clicks this soon after one. */
const GHOST_CLICK_MS = 700;

/**
 * Calls `fn` on a quick tap. Adds `.down` while a finger rests on the element.
 * The element owns its touches: they don't reach parent gesture handlers (deck swipes).
 */
export function onTap(node: HTMLElement, fn: () => void): void {
  let touchId = -1;
  let startX = 0;
  let startY = 0;
  let lastTouchEnd = 0;

  function cancel(): void {
    touchId = -1;
    node.classList.remove("down");
  }

  node.addEventListener("touchstart", function (e) {
    e.stopPropagation();
    if (touchId !== -1) return;
    const t = e.changedTouches[0];
    touchId = t.identifier;
    startX = t.clientX;
    startY = t.clientY;
    node.classList.add("down");
  }, false);

  node.addEventListener("touchmove", function (e) {
    for (let i = 0; i < e.changedTouches.length; i++) {
      const t = e.changedTouches[i];
      if (t.identifier === touchId && (Math.abs(t.clientX - startX) > SLOP || Math.abs(t.clientY - startY) > SLOP)) cancel();
    }
  }, false);

  node.addEventListener("touchend", function (e) {
    for (let i = 0; i < e.changedTouches.length; i++) {
      if (e.changedTouches[i].identifier !== touchId) continue;
      cancel();
      lastTouchEnd = Date.now();
      e.preventDefault(); // no synthetic click
      fn();
      return;
    }
  }, false);

  node.addEventListener("touchcancel", cancel, false);
  node.addEventListener("mousedown", function (e) {
    e.stopPropagation();
  }, false);
  node.addEventListener("click", function () {
    if (Date.now() - lastTouchEnd > GHOST_CLICK_MS) fn();
  }, false);
}

/** Enter or Space: the keys that activate a focused button. */
export function isActivationKey(e: KeyboardEvent): boolean {
  return e.keyCode === 13 || e.keyCode === 32;
}

/**
 * `onTap` for a control that keyboards and screen readers must reach too: focusable, with the
 * button role, activated by Enter / Space. `label` names icon-only controls; visible text names the rest.
 */
export function tapButton(node: HTMLElement, fn: () => void, label?: string): void {
  node.setAttribute("role", "button");
  node.tabIndex = 0;
  if (label) node.setAttribute("aria-label", label);
  node.addEventListener("keydown", function (e) {
    if (!isActivationKey(e)) return;
    e.preventDefault();
    e.stopPropagation(); // e.g. Enter on a pairing key must not also submit the code
    if (!e.repeat) fn();
  }, false);
  onTap(node, fn);
}
