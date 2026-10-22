// A sheet you can drag, and where it comes to rest.
//
// Every bottom sheet on this platform follows the finger: pulled down, it goes
// away; pulled up, where it has a taller shape, it takes it; let go in between,
// it settles on the nearest of the shapes it has. The shapes are *detents* —
// fractions of the screen the sheet shows — and a sheet with one detent is a
// sheet that can only be dismissed, which is still worth the gesture.
//
// Two rules decide whether a touch is a drag at all, and both are about the
// list inside the sheet. A finger that starts on the grabber or the head is
// always a drag. A finger that starts on the body is a drag only when the body
// is scrolled to its top and the finger moves down — the direction a scroll
// could not take it — or when the sheet is below its tallest shape and the
// finger moves up. Otherwise the scroll keeps the gesture, which is what makes
// a sheet with a long list in it usable at all.
//
// `settle` is a pure function, so it can be tested on its own: where the sheet
// ends up for a given position and release speed.

import { useEffect, useRef } from "react";

/** Past this, the finger was still moving when it left: a flick. */
const FLICK_PX_PER_MS = 0.8;
/** How far ahead a moving finger is projected, in milliseconds of travel. */
const PROJECT_MS = 180;
/** Beyond the lowest detent by this much of what was showing there, letting go
 *  is "put it away" rather than "put it back". */
const DISMISS_FRACTION = 0.4;
/** What a touch has to travel before it is a direction at all. */
const SLOP_PX = 8;
/** Above the tallest shape the sheet follows at a fraction of the finger. */
const RESIST = 0.3;

export type Settled = { kind: "stop"; index: number } | { kind: "dismiss" };

/** Where a sheet released at `ty` (px below its tallest shape) and moving at
 *  `vy` (px/ms, down positive) comes to rest, given its `stops` (ty of each
 *  shape, ascending, 0 first) and `height` (the sheet's own box). */
export function settle(
  ty: number,
  vy: number,
  stops: number[],
  height: number,
  canDismiss: boolean
): Settled {
  const lowest = stops[stops.length - 1] ?? 0;
  const projected = ty + vy * PROJECT_MS;
  if (canDismiss) {
    const showing = Math.max(1, height - lowest);
    if (projected > lowest + showing * DISMISS_FRACTION) return { kind: "dismiss" };
    if (vy > FLICK_PX_PER_MS && ty >= lowest - 1) return { kind: "dismiss" };
  }
  const target = Math.max(0, Math.min(lowest, projected));
  let index = 0;
  let best = Infinity;
  stops.forEach((s, i) => {
    const d = Math.abs(s - target);
    if (d < best) {
      best = d;
      index = i;
    }
  });
  return { kind: "stop", index };
}

export interface SheetDragOptions {
  /** The shapes, as fractions of the window the sheet shows, ascending. One
   *  shape — the sheet's natural height — when omitted. */
  detents?: number[];
  /** Which shape the sheet is in now. The hook moves the sheet there whenever
   *  this changes and the finger is not on it. */
  index?: number;
  /** The finger chose another shape. */
  onIndex?: (index: number) => void;
  /** Pulled down past the lowest shape. Omitted, the lowest shape is the floor. */
  onDismiss?: () => void;
  /** Where a drag may start regardless of the body's scroll: the grabber, the
   *  head. Anything matching this selector inside the sheet. */
  handle?: string;
  enabled?: boolean;
}

export function useSheetDrag(
  ref: React.RefObject<HTMLElement | null>,
  { detents = [1], index = 0, onIndex, onDismiss, handle = ".sheet-grab, .sheet-head, .exch-grab", enabled = true }: SheetDragOptions
) {
  const latest = useRef({ onIndex, onDismiss, index });
  latest.current = { onIndex, onDismiss, index };
  const detentKey = detents.join(",");

  // Rest at the shape asked for, whenever it changes without a finger.
  useEffect(() => {
    const el = ref.current;
    if (!el || !enabled) return;
    const stops = stopsFor(el, detents);
    el.classList.add("settling");
    el.style.transform = `translateY(${stops[toStop(index, detents)] ?? 0}px)`;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ref, enabled, index, detentKey]);

  useEffect(() => {
    const el = ref.current;
    if (!el || !enabled) return;

    let startY = 0;
    let startX = 0;
    let startTy = 0;
    let startedAt = 0;
    let lastY = 0;
    let lastAt = 0;
    let vy = 0;
    /** `"?"` undecided, `"y"` this is a drag, `"off"` the scroll or a swipe has it. */
    let axis: "?" | "y" | "off" = "?";
    let fromHandle = false;
    let scroller: HTMLElement | null = null;
    let stops: number[] = [];

    const onStart = (e: TouchEvent) => {
      const t = e.touches[0];
      if (!t) return;
      stops = stopsFor(el, detents);
      startY = lastY = t.clientY;
      startX = t.clientX;
      startedAt = lastAt = e.timeStamp;
      startTy = stops[toStop(latest.current.index, detents)] ?? 0;
      vy = 0;
      axis = "?";
      const target = e.target as HTMLElement;
      fromHandle = !!target.closest(handle);
      scroller = fromHandle ? null : scrollerOf(target, el);
      el.classList.remove("settling");
    };
    const onMove = (e: TouchEvent) => {
      if (axis === "off") return;
      const t = e.touches[0];
      if (!t) return;
      const dy = t.clientY - startY;
      const dx = t.clientX - startX;
      if (axis === "?") {
        if (Math.abs(dx) > SLOP_PX && Math.abs(dx) > Math.abs(dy)) {
          axis = "off";
          return;
        }
        if (Math.abs(dy) < SLOP_PX) return;
        // The scroll keeps a gesture it can use: down with the list not at its
        // top, or up with the sheet already at its tallest.
        const atTop = !scroller || scroller.scrollTop <= 0;
        const canGrow = startTy > 0;
        if (!fromHandle && ((dy > 0 && !atTop) || (dy < 0 && !canGrow))) {
          axis = "off";
          return;
        }
        axis = "y";
      }
      e.preventDefault();
      const now = e.timeStamp;
      if (now > lastAt) vy = (t.clientY - lastY) / (now - lastAt);
      lastY = t.clientY;
      lastAt = now;
      let ty = startTy + dy - Math.sign(dy) * SLOP_PX;
      if (ty < 0) ty *= RESIST;
      el.style.transform = `translateY(${ty}px)`;
    };
    const onEnd = (e: TouchEvent) => {
      if (axis !== "y") {
        axis = "off";
        return;
      }
      axis = "off";
      const t = e.changedTouches[0];
      const dy = t ? t.clientY - startY : 0;
      const ty = startTy + dy;
      // A finger that paused before leaving is not flicking, however fast it
      // went earlier.
      if (e.timeStamp - lastAt > 80) vy = 0;
      const where = settle(ty, vy, stops, el.offsetHeight, !!latest.current.onDismiss);
      el.classList.add("settling");
      if (where.kind === "dismiss") {
        el.style.transform = `translateY(${el.offsetHeight + 40}px)`;
        latest.current.onDismiss?.();
        return;
      }
      el.style.transform = `translateY(${stops[where.index]}px)`;
      const shape = toStop(where.index, detents);
      if (shape !== latest.current.index) latest.current.onIndex?.(shape);
      void startedAt;
    };

    el.addEventListener("touchstart", onStart, { passive: true });
    el.addEventListener("touchmove", onMove, { passive: false });
    el.addEventListener("touchend", onEnd, { passive: true });
    el.addEventListener("touchcancel", onEnd, { passive: true });
    return () => {
      el.removeEventListener("touchstart", onStart);
      el.removeEventListener("touchmove", onMove);
      el.removeEventListener("touchend", onEnd);
      el.removeEventListener("touchcancel", onEnd);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ref, enabled, detentKey, handle]);
}

/** The ty of each shape, ascending — so the *tallest* shape comes first here,
 *  while `detents` lists the shortest first. `toStop` maps between the two. */
function stopsFor(el: HTMLElement, detents: number[]): number[] {
  const h = el.offsetHeight;
  const win = window.innerHeight || h;
  return detents.map((d) => Math.max(0, h - d * win)).reverse();
}
/** Shape index ↔ stop index: the same list read from the other end. */
function toStop(i: number, detents: number[]): number {
  return detents.length - 1 - i;
}

/** The nearest scrolling box between the touch and the sheet, if any. */
function scrollerOf(target: HTMLElement, sheet: HTMLElement): HTMLElement | null {
  let n: HTMLElement | null = target;
  while (n && n !== sheet) {
    const { overflowY } = getComputedStyle(n);
    if ((overflowY === "auto" || overflowY === "scroll") && n.scrollHeight > n.clientHeight) return n;
    n = n.parentElement;
  }
  return null;
}
