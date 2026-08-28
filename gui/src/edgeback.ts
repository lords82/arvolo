// Going back with a thumb, from the left edge.
//
// The button in the bar is the way back you can see; this is the one people
// actually use, and on this platform every screen that was pushed has it. It is
// not an extra: a stack with no gesture out of it is a stack that has to be
// climbed by aiming at a small target in the far corner, one-handed, at the top
// of a tall phone.
//
// Deliberately small. It follows the finger so the screen answers immediately —
// the whole point of an interactive gesture is that you can see it working and
// change your mind — and it commits past a third of the width or a flick, which
// are the two ways people mean it.

import { useEffect, useRef } from "react";

/** Where a drag has to start to count as "from the edge". */
const EDGE_PX = 28;
/** Past this, letting go goes back. */
const COMMIT_FRACTION = 0.32;
/** Or this, if the finger was still moving when it left. */
const FLICK_PX_PER_MS = 0.5;

export function useEdgeBack(
  el: React.RefObject<HTMLElement | null>,
  enabled: boolean,
  onBack: () => void
) {
  // The callback changes every render; the listeners must not.
  const back = useRef(onBack);
  back.current = onBack;

  useEffect(() => {
    const node = el.current;
    if (!node || !enabled) return;

    let startX = 0;
    let startY = 0;
    let startedAt = 0;
    let dragging = false;

    const paint = (dx: number) => {
      node.style.transform = dx ? `translateX(${dx}px)` : "";
      node.style.transition = "";
    };
    const release = (commit: boolean) => {
      node.style.transition = "transform 350ms cubic-bezier(0.32,0.72,0,1)";
      node.style.transform = commit ? "translateX(100%)" : "";
      if (commit) {
        // After the slide, not before: going back mid-animation swaps the
        // content under the finger and the screen appears to jump.
        window.setTimeout(() => {
          node.style.transition = "";
          node.style.transform = "";
          back.current();
        }, 340);
      }
    };

    const onStart = (e: TouchEvent) => {
      const touch = e.touches[0];
      if (!touch || touch.clientX > EDGE_PX) return;
      startX = touch.clientX;
      startY = touch.clientY;
      startedAt = e.timeStamp;
      dragging = true;
    };
    const onMove = (e: TouchEvent) => {
      if (!dragging) return;
      const touch = e.touches[0];
      if (!touch) return;
      const dx = touch.clientX - startX;
      const dy = Math.abs(touch.clientY - startY);
      // A vertical intent is a scroll, and stealing it would make the list feel
      // stuck whenever somebody starts a scroll near the edge.
      if (dy > 24 && dy > Math.abs(dx)) {
        dragging = false;
        paint(0);
        return;
      }
      if (dx > 0) {
        e.preventDefault();
        paint(dx);
      }
    };
    const onEnd = (e: TouchEvent) => {
      if (!dragging) return;
      dragging = false;
      const touch = e.changedTouches[0];
      const dx = touch ? touch.clientX - startX : 0;
      const speed = dx / Math.max(1, e.timeStamp - startedAt);
      release(dx > node.clientWidth * COMMIT_FRACTION || speed > FLICK_PX_PER_MS);
    };

    node.addEventListener("touchstart", onStart, { passive: true });
    node.addEventListener("touchmove", onMove, { passive: false });
    node.addEventListener("touchend", onEnd, { passive: true });
    node.addEventListener("touchcancel", onEnd, { passive: true });
    return () => {
      node.removeEventListener("touchstart", onStart);
      node.removeEventListener("touchmove", onMove);
      node.removeEventListener("touchend", onEnd);
      node.removeEventListener("touchcancel", onEnd);
      node.style.transform = "";
      node.style.transition = "";
    };
  }, [el, enabled]);
}
