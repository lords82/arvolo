// What the system's back takes away, and in what order.
//
// On this platform the back gesture belongs to the system: it reaches the app
// as a history pop, and anything that wants a turn at it has to have left an
// entry behind. Screens do — `push` leaves one — but for a while nothing else
// did, so a back press with a sheet, an alert or a menu open walked straight
// past it and popped the screen underneath: from the first screen, that means
// leaving the app while a dialog is on it.
//
// The fix is one register rather than a rule repeated in every overlay. What is
// open says so here and says how to close itself; a pop takes the topmost. It
// lives outside the store deliberately — half of what can be on top of a screen
// is component state (a menu, a confirmation inside a view), and a mechanism
// that only knew about the store would have covered half the cases and looked
// like it covered all of them.
//
// The subtlety is the entry taken back. Closing by hand — the Cancel button,
// picking a menu item — calls `history.back()` to keep the entry count honest,
// and that fires a popstate which is *not* somebody pressing back. So it is
// counted and swallowed. The invariant that makes this safe: `leave` and
// `consume` are called in pairs by the same layer, and a genuine pop takes the
// layer and its entry together, which is why the release below checks whether
// it is still registered before consuming anything.

import { useEffect, useRef } from "react";

/** Pops we asked for ourselves, waiting to be ignored. */
let swallow = 0;
/** How to close what is open, oldest first. */
const layers: Array<() => void> = [];

/** Leave an entry for back to pop. */
function leave(): void {
  try {
    window.history.pushState({ arvolo: "layer" }, "");
  } catch {
    // A renderer with no history: the buttons still work, the gesture does not.
  }
}

/** Take back the entry left by something now being closed by hand. */
function consume(): void {
  swallow++;
  try {
    window.history.back();
  } catch {
    swallow--;
  }
}

/** Register something that is open. Returns the release. */
export function hold(close: () => void): () => void {
  leave();
  layers.push(close);
  return () => {
    const i = layers.indexOf(close);
    // Gone already: a back press took it, and its entry with it.
    if (i < 0) return;
    layers.splice(i, 1);
    consume();
  };
}

/** Did this pop belong to a layer rather than to the app's own stack? */
export function tookBack(): boolean {
  if (swallow > 0) {
    swallow--;
    return true;
  }
  const close = layers.pop();
  if (!close) return false;
  close();
  return true;
}

/** Hold the back gesture for as long as `active`. */
export function useBackDismiss(active: boolean, close: () => void): void {
  // The callback is a new function every render; the registration must not be.
  const latest = useRef(close);
  latest.current = close;
  useEffect(() => {
    if (!active) return;
    return hold(() => latest.current());
  }, [active]);
}

/** Test seam: forget everything held. */
export function resetBackstack(): void {
  swallow = 0;
  layers.length = 0;
}
