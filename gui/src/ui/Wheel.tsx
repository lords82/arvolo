// The wheel, for the handful of choices that are one of a short list.
//
// This platform does not put three exclusive answers in a list with a tick; it
// raises a cylinder you turn with your thumb and confirms with a word. The
// design of record draws it for theme and language (7s/7t), and the reason to
// follow it rather than keep the list is the gesture: a list of five languages
// is five targets to aim at, a wheel is one movement and no aiming at all.
//
// Built on scroll snapping rather than on a 3D transform, because that is what
// the wheel actually is on this platform: a scroller with one item centred, a
// band drawn behind it, and the ends faded. Momentum, rubber band and snap come
// from the engine, which is why it feels right on a phone and like a list on a
// desktop — where it is never shown.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useT } from "../i18n";
import { useBackDismiss } from "../backstack";
import { haptic } from "../haptics";
import { SheetGrab } from "./Sheet";
import { useSheetDrag } from "./sheetDrag";
import { useModal } from "./Sheet";

/** Row height, and the only number the arithmetic below needs. Kept in step with
 *  `--wheel-item` in the stylesheet, which draws the band at the same height.
 *  44 because a row here is also a tap target, and that is the floor. */
const ITEM = 44;

export interface WheelOption<T extends string> {
  value: T;
  label: string;
}

export function WheelSheet<T extends string>({
  open,
  title,
  hint,
  options,
  value,
  onPreview,
  onCancel,
  onDone,
}: {
  open: boolean;
  title: string;
  /** The line under the wheel: what the odd option out actually means. */
  hint?: string;
  options: WheelOption<T>[];
  value: T;
  /** Called as the wheel turns, for a choice whose effect can be *seen* behind
   *  the sheet — the theme is one, and the design says so. Left out where a live
   *  change would rewrite the sheet's own buttons under the thumb. */
  onPreview?: (v: T) => void;
  onCancel: () => void;
  onDone: (v: T) => void;
}) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const at = Math.max(0, options.findIndex((o) => o.value === value));
  const [i, setI] = useState(at);
  useModal(ref, onCancel, open);
  useBackDismiss(open, onCancel);

  // Open on the current answer, without animating there: the wheel is already
  // turned to where you left it.
  useLayoutEffect(() => {
    if (!open) return;
    setI(at);
    list.current?.scrollTo?.({ top: at * ITEM, behavior: "auto" });
    // Only when it opens: re-running on `at` would fight the finger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // The scroll decides, and nothing else: a tap on a row scrolls to it rather
  // than selecting behind the wheel's back, so there is one source of truth for
  // where the cylinder is.
  const settle = () => {
    const el = list.current;
    if (!el) return;
    const n = Math.min(options.length - 1, Math.max(0, Math.round(el.scrollTop / ITEM)));
    if (n === i) return;
    setI(n);
    haptic.picked();
    onPreview?.(options[n].value);
  };

  // Whatever the theme was before the thumb started turning, so Annulla can put
  // it back — a preview that could not be undone would be a change made by
  // scrolling past something.
  const was = useRef(value);
  useEffect(() => {
    if (open) was.current = value;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Pulled down, it is Annulla: the preview goes back to what it was.
  useSheetDrag(ref, {
    enabled: open,
    onDismiss: () => {
      onPreview?.(was.current);
      onCancel();
    },
  });
  if (!open) return null;

  const goTo = (n: number) => {
    list.current?.scrollTo?.({ top: n * ITEM, behavior: "smooth" });
    // jsdom, and any engine that does not implement smooth scrolling, never
    // fires the scroll this would settle on — so the tap commits by itself.
    if (n !== i) {
      setI(n);
      onPreview?.(options[n].value);
    }
  };

  return createPortal(
    <>
      <div
        className="scrim"
        onClick={() => {
          onPreview?.(was.current);
          onCancel();
        }}
      />
      <div
        ref={ref}
        className="wheel-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
      >
        <SheetGrab />
        <div className="sheet-head sheet-bar">
          <span className="bar-left">
            <button
              className="bar-btn"
              onClick={() => {
                onPreview?.(was.current);
                onCancel();
              }}
            >
              {t("common.cancel")}
            </button>
          </span>
          <span className="t-head truncate">{title}</span>
          <span className="bar-right">
            <button className="bar-btn strong" onClick={() => onDone(options[i].value)}>
              {t("common.done")}
            </button>
          </span>
        </div>

        <div className="wheel">
          <div className="wheel-band" aria-hidden />
          <div
            ref={list}
            className="wheel-list"
            role="listbox"
            aria-label={title}
            tabIndex={0}
            onScroll={settle}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                goTo(Math.min(options.length - 1, i + 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                goTo(Math.max(0, i - 1));
              }
            }}
          >
            <span className="wheel-pad" aria-hidden />
            {options.map((o, n) => (
              <button
                key={o.value}
                role="option"
                aria-selected={n === i}
                className={`wheel-item${n === i ? " on" : ""}`}
                onClick={() => goTo(n)}
              >
                {o.label}
              </button>
            ))}
            <span className="wheel-pad" aria-hidden />
          </div>
        </div>

        {hint ? <div className="ios-foot wheel-hint">{hint}</div> : null}
      </div>
    </>,
    document.body
  );
}
