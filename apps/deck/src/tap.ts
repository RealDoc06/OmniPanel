// Tap handling for chrome (keypad, status bar, settings): fires on touchend so there is
// no 300 ms click delay, and on `click` for mouse / keyboard. Doesn't block scrolling.

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
