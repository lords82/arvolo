// Dragging a row to a new place in its list.
//
// Pointer events, not HTML5 drag-and-drop. Two reasons, and either alone would
// decide it: the window already owns `drop` for files (App.tsx cancels it
// document-wide so a stray file cannot navigate the webview away), and on
// Windows the WebView2 host intercepts drag gestures for the OS with
// `dragDropEnabled`, so a `dragstart` inside the page never arrives. Pointer
// events are untouched by both.
//
// The geometry is measured once, at pointer-down, and every later decision is
// computed from those frozen rects. Reading the DOM again mid-drag would see
// the rows *already displaced* by the drag itself, and the target index would
// oscillate between two values a pixel apart.

import { useCallback, useEffect, useRef, useState } from "react";
import type {
  CSSProperties,
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
} from "react";

type Geo = { top: number; height: number };
type Drag = {
  key: string;
  from: number;
  /** Where it would land if released now. */
  to: number;
  dy: number;
  height: number;
};

/** Pointer travel, in px, before a press becomes a drag. Below it the gesture
 *  is still a click — which on these rows means "open me", not "move me". */
const SLOP = 3;

export type ReorderBits = {
  /** Put this on the element that contains the rows, in order. */
  listRef: (el: HTMLDivElement | null) => void;
  /** Spread onto the drag handle of the row with this key. */
  gripProps: (key: string) => {
    onPointerDown: (e: ReactPointerEvent) => void;
    onClick: (e: ReactMouseEvent) => void;
  };
  /** Merge into the row at this index: what the drag is doing to it. */
  rowProps: (index: number) => { className: string; style?: CSSProperties };
  /** Key of the row being dragged, if one is. */
  active: string | null;
};

/**
 * Drag-to-reorder for a vertical list of `.row` elements.
 *
 * @param keys  the rows, in the order they are rendered.
 * @param commit called with the new order once, on release, only if it changed.
 */
export function useReorder(
  keys: string[],
  commit: (order: string[]) => void
): ReorderBits {
  const [drag, setDrag] = useState<Drag | null>(null);
  const dragRef = useRef<Drag | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  // Read inside listeners that were installed before the current render, so
  // they must not close over the props themselves.
  const keysRef = useRef(keys);
  keysRef.current = keys;
  const commitRef = useRef(commit);
  commitRef.current = commit;
  const stopRef = useRef<(() => void) | null>(null);
  // Whether the gesture that just ended actually moved the row. A press that
  // did not is still a click, and on these rows a click may mean "open me".
  const movedRef = useRef(false);

  // A drag left running past unmount would keep window listeners alive and
  // commit into a store nobody is showing any more.
  useEffect(() => () => stopRef.current?.(), []);

  const onPointerDown = useCallback((key: string, e: ReactPointerEvent) => {
    // Left button only, and never while another drag is still settling.
    if (e.button !== 0 || dragRef.current || stopRef.current) return;
    const list = listRef.current;
    if (!list) return;
    const rows = Array.from(list.querySelectorAll<HTMLElement>(":scope > .row"));
    const frozen = keysRef.current.slice();
    // If the DOM and the key list disagree, the indices below would move the
    // wrong row. Refuse rather than guess.
    if (rows.length !== frozen.length) return;
    const from = frozen.indexOf(key);
    if (from < 0 || frozen.length < 2) return;

    const geo: Geo[] = rows.map((el) => {
      const r = el.getBoundingClientRect();
      return { top: r.top, height: r.height };
    });
    const startY = e.clientY;
    movedRef.current = false;
    const height = geo[from].height;
    // How far the row may travel before it is past the ends of the list.
    const last = geo[geo.length - 1];
    const min = geo[0].top - geo[from].top;
    const max = last.top + last.height - (geo[from].top + height);

    const at = (d: Drag | null) => {
      dragRef.current = d;
      setDrag(d);
    };

    const move = (ev: PointerEvent) => {
      const raw = ev.clientY - startY;
      if (!dragRef.current && Math.abs(raw) < SLOP) return;
      // Clamped for the eye only. The target index is computed from the raw
      // travel, because the clamp stops the row exactly *on* the first and last
      // centres — with the clamped value the two end slots sit at a boundary
      // the comparison below can never cross, and the ends become unreachable.
      const dy = Math.min(max, Math.max(min, raw));
      // The row's centre against the *undisplaced* centres of the others: the
      // list opens a gap where that centre currently is.
      const centre = geo[from].top + height / 2 + raw;
      let to = from;
      for (let k = from + 1; k < geo.length; k++) {
        if (centre > geo[k].top + geo[k].height / 2) to = k;
      }
      for (let k = from - 1; k >= 0; k--) {
        if (centre < geo[k].top + geo[k].height / 2) to = k;
      }
      movedRef.current = true;
      at({ key, from, to, dy, height });
    };

    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", drop);
      window.removeEventListener("pointercancel", abort);
      window.removeEventListener("keydown", onKey, true);
      stopRef.current = null;
      at(null);
    };
    const abort = () => stop();
    const onKey = (ev: KeyboardEvent) => {
      // Escape puts it back where it came from, like every other drag people
      // have ever used.
      if (ev.key === "Escape") {
        ev.preventDefault();
        ev.stopPropagation();
        stop();
      }
    };
    const drop = () => {
      const d = dragRef.current;
      stop();
      if (!d || d.to === d.from) return;
      // Rows arrive and leave on daemon events, drag or no drag. If the list
      // changed under the gesture, the indices no longer mean anything.
      const nowKeys = keysRef.current;
      if (nowKeys.length !== frozen.length) return;
      if (nowKeys.some((k, i) => k !== frozen[i])) return;
      const order = frozen.slice();
      order.splice(d.to, 0, order.splice(d.from, 1)[0]);
      commitRef.current(order);
    };

    stopRef.current = stop;
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", drop);
    window.addEventListener("pointercancel", abort);
    window.addEventListener("keydown", onKey, true);
    // Keeps the press from starting a text selection or the webview's own
    // image drag, both of which would fight the gesture.
    e.preventDefault();
  }, []);

  const gripProps = useCallback(
    (key: string) => ({
      onPointerDown: (e: ReactPointerEvent) => onPointerDown(key, e),
      // The click the browser synthesises after a drag lands on the row, whose
      // own click handler would open a dialog nobody asked for. Swallow that
      // one; leave an honest click alone.
      onClick: (e: ReactMouseEvent) => {
        if (!movedRef.current) return;
        movedRef.current = false;
        e.preventDefault();
        e.stopPropagation();
      },
    }),
    [onPointerDown]
  );

  const rowProps = useCallback(
    (index: number): { className: string; style?: CSSProperties } => {
      if (!drag) return { className: "" };
      if (index === drag.from) {
        return {
          className: "is-drag",
          style: { transform: `translateY(${drag.dy}px)` },
        };
      }
      // Everything the row passed over moves by exactly its height — the size
      // of the gap it left behind — which is why rows of different heights
      // still land where the drop will put them.
      const up = drag.to > drag.from && index > drag.from && index <= drag.to;
      const down = drag.to < drag.from && index >= drag.to && index < drag.from;
      const shift = up ? -drag.height : down ? drag.height : 0;
      return {
        className: "is-shift",
        style: shift ? { transform: `translateY(${shift}px)` } : undefined,
      };
    },
    [drag]
  );

  return {
    listRef: useCallback((el: HTMLDivElement | null) => {
      listRef.current = el;
    }, []),
    gripProps,
    rowProps,
    active: drag?.key ?? null,
  };
}