// Pull down on a list to ask again.
//
// Every list on this platform has it, and an app that updates only when it is
// reopened is an app whose lists are read with a small doubt. The elastic was
// already there — the page is a scroller that rubber-bands at the top — so what
// is added here is only the question "was that a pull?" and the answer.
//
// Deliberately light on the scroller: nothing is prevented, nothing is scrolled
// from JS, and no padding is animated. The native bounce moves the content, the
// indicator sits above the content and is revealed by that same bounce, and the
// one thing this hook writes while the finger is down is a progress number. Only
// after the finger lets go does the page move on its own, and by a transform.
//
// The reveal is read from `scrollTop`, which WebKit reports as negative while a
// box is rubber-banding — and, where it is clamped to zero instead, estimated
// from the finger, at the bounce's own half rate. One of the two is always
// there, and the threshold is the same in both.

import { useEffect, useRef } from "react";
import { haptic } from "../haptics";

/** How much of the indicator has to be uncovered for letting go to count. */
const FIRE_PX = 56;
/** What a touch has to travel before it is a direction at all. */
const SLOP_PX = 10;
/** The spinner stays at least this long: a refresh that answers in 40ms and
 *  snaps the list back reads as nothing having happened. */
const MIN_BUSY_MS = 600;

export function usePullToRefresh(
  el: React.RefObject<HTMLElement | null>,
  enabled: boolean,
  onRefresh: () => Promise<void>
) {
  const refresh = useRef(onRefresh);
  refresh.current = onRefresh;

  useEffect(() => {
    const node = el.current;
    if (!node || !enabled) return;

    /** The touch began at the top, and may become a pull. */
    let armed = false;
    /** Decided: vertical, downwards. */
    let pulling = false;
    /** Past the threshold: letting go now refreshes. */
    let fired = false;
    /** A refresh is running; the page is held down meanwhile. */
    let busy = false;
    let startX = 0;
    let startY = 0;

    const progress = (p: number) => node.style.setProperty("--prog", String(p));

    const onStart = (e: TouchEvent) => {
      if (busy) return;
      const t = e.touches[0];
      if (!t) return;
      armed = node.scrollTop <= 0;
      pulling = false;
      fired = false;
      startX = t.clientX;
      startY = t.clientY;
      node.classList.remove("ptr-settle");
    };
    const onMove = (e: TouchEvent) => {
      if (!armed) return;
      const t = e.touches[0];
      if (!t) return;
      const dy = t.clientY - startY;
      const dx = t.clientX - startX;
      if (!pulling) {
        // A sideways intent is somebody else's gesture (the edge, a row), and
        // an upward one is a scroll. Decided once, on the first real movement.
        if (Math.abs(dx) > SLOP_PX && Math.abs(dx) > Math.abs(dy)) {
          armed = false;
          return;
        }
        if (dy < -SLOP_PX) {
          armed = false;
          return;
        }
        if (dy < SLOP_PX) return;
        pulling = true;
      }
      const reveal = node.scrollTop < 0 ? -node.scrollTop : (dy - SLOP_PX) * 0.5;
      const p = Math.max(0, Math.min(1, reveal / FIRE_PX));
      progress(p);
      if (p >= 1 && !fired) {
        fired = true;
        // The knock that says "far enough": from here, letting go does it.
        haptic.lifted();
      } else if (p < 1 && fired) {
        fired = false;
      }
    };
    const onEnd = () => {
      if (!armed) return;
      armed = false;
      node.classList.add("ptr-settle");
      if (!fired) {
        progress(0);
        return;
      }
      fired = false;
      busy = true;
      node.classList.add("ptr-busy");
      const began = Date.now();
      Promise.resolve()
        .then(() => refresh.current())
        .catch(() => {
          // The store has already said what went wrong, in its own place.
        })
        .then(() => {
          const wait = Math.max(0, MIN_BUSY_MS - (Date.now() - began));
          window.setTimeout(() => {
            busy = false;
            node.classList.remove("ptr-busy");
            progress(0);
          }, wait);
        });
    };

    node.addEventListener("touchstart", onStart, { passive: true });
    node.addEventListener("touchmove", onMove, { passive: true });
    node.addEventListener("touchend", onEnd, { passive: true });
    node.addEventListener("touchcancel", onEnd, { passive: true });
    return () => {
      node.removeEventListener("touchstart", onStart);
      node.removeEventListener("touchmove", onMove);
      node.removeEventListener("touchend", onEnd);
      node.removeEventListener("touchcancel", onEnd);
      node.classList.remove("ptr-busy", "ptr-settle");
      node.style.removeProperty("--prog");
    };
  }, [el, enabled]);
}
