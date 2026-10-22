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

/** Where a drag has to start to count as "from the edge".
 *
 *  Wider than it looks: what a person calls "the edge" is where their thumb
 *  lands when they reach for it, not the first column of pixels, and this
 *  platform's own recogniser is about this wide. Too narrow and the gesture
 *  simply does not happen, which is indistinguishable from not existing. */
const EDGE_PX = 40;
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

    /** What this gesture is pushing off the screen.
     *
     *  Asked at the start of each gesture rather than once, because the answer
     *  changes with what is on top. A person's card and a mailbox thread *look*
     *  pushed — a back button in the bar, a title, a screen you leave the same
     *  way — but they are sheets, drawn through a portal into the body and not
     *  inside the shell at all. Moving the shell under them moved everything
     *  except the thing the finger was on.
     *
     *  Otherwise the whole shell, bar included: that is what being pushed off
     *  looks like, and moving only the content leaves the title sitting still
     *  above a screen that is leaving. */
    const target = (): HTMLElement => {
      // Both kinds, because there are two: a `Sheet` drawn as a pushed screen,
      // and the mailbox thread, which is not a `Sheet` at all and says so. The
      // thread is the screen this gesture is used on most, and it was the one
      // being left behind — the swipe went back, and what slid was the shell
      // underneath, which is to say nothing anybody could see.
      const layers = document.querySelectorAll<HTMLElement>(
        ".sheet.pushscreen, .sheet-full"
      );
      return layers[layers.length - 1] ?? node.closest<HTMLElement>(".main") ?? node;
    };
    let moving: HTMLElement = target();

    let startX = 0;
    let startY = 0;
    let startedAt = 0;
    /** Started at the edge, and not yet decided. */
    let armed = false;
    /** Decided: this gesture is the way back, and the scroll cannot have it. */
    let dragging = false;

    const paint = (dx: number) => {
      moving.style.transform = dx ? `translateX(${dx}px)` : "";
      moving.style.transition = "";
    };
    const release = (commit: boolean) => {
      moving.style.transition = "transform 350ms cubic-bezier(0.32,0.72,0,1)";
      moving.style.transform = commit ? "translateX(100%)" : "";
      if (commit) {
        // After the slide, not before: going back mid-animation swaps the
        // content under the finger and the screen appears to jump.
        window.setTimeout(() => {
          moving.style.transition = "";
          moving.style.transform = "";
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
      armed = true;
      dragging = false;
      moving = target();
    };
    const onMove = (e: TouchEvent) => {
      if (!armed) return;
      const touch = e.touches[0];
      if (!touch) return;
      const dx = touch.clientX - startX;
      const dy = Math.abs(touch.clientY - startY);
      if (!dragging) {
        // A vertical intent is a scroll, and stealing it would make the list
        // feel stuck whenever somebody starts a scroll near the edge. Decided
        // once, on the first movement with a direction in it.
        if (dy > 10 && dy > Math.abs(dx)) {
          armed = false;
          return;
        }
        if (dx < 8) return;
        dragging = true;
      }
      // Held from here on, whichever way the finger wanders: without this the
      // first drift back to the left hands the gesture to the scroller, which
      // then keeps it, and the screen stops following a finger that is still
      // on it.
      e.preventDefault();
      paint(Math.max(0, dx));
    };
    const onEnd = (e: TouchEvent) => {
      const wasDragging = dragging;
      armed = false;
      dragging = false;
      if (!wasDragging) return;
      const touch = e.changedTouches[0];
      const dx = touch ? touch.clientX - startX : 0;
      const speed = dx / Math.max(1, e.timeStamp - startedAt);
      release(dx > node.clientWidth * COMMIT_FRACTION || speed > FLICK_PX_PER_MS);
    };

    // On the document, in the capture phase, rather than on one box: a touch
    // that starts at the edge lands on whatever happens to be drawn there — a
    // row, a scroller, a sheet — and a listener attached to one of them is a
    // listener the next screen does not have. The capture phase is what gets it
    // before a scroller decides the gesture is its own.
    const opts = { passive: false, capture: true } as const;
    document.addEventListener("touchstart", onStart, { passive: true, capture: true });
    document.addEventListener("touchmove", onMove, opts);
    document.addEventListener("touchend", onEnd, { passive: true, capture: true });
    document.addEventListener("touchcancel", onEnd, { passive: true, capture: true });
    return () => {
      document.removeEventListener("touchstart", onStart, { capture: true });
      document.removeEventListener("touchmove", onMove, opts);
      document.removeEventListener("touchend", onEnd, { capture: true });
      document.removeEventListener("touchcancel", onEnd, { capture: true });
      moving.style.transform = "";
      moving.style.transition = "";
    };
  }, [el, enabled]);
}
