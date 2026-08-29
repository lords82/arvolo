// A list row you can swipe to act on.
//
// On this platform a list is not only a set of things to open: it is a set of
// things to *do something to*, and the doing is a swipe. The alternative is a
// menu behind a glyph on every row, which is four taps of chrome per line and
// still hides the one action that matters.
//
// One or two actions, never more. Mail's own rows stop at two per side, and the
// reason is the thumb: past two, the buttons are narrower than the finger that
// has to land on one of them, and the row stops being a shortcut. The rule here
// is that a row offers what you would actually do to *that* row in *that* state
// — hold what is running and give it up, open what has arrived, take back what
// nobody collected — and everything else stays one tap in, where it has a name.
//
// A single action commits by itself past a long pull, which is this platform's
// own promise: keep going and you do not have to aim at the button. Two do not,
// because there would be no saying which of them you meant.

import { useRef, useState, type ReactNode } from "react";
import { haptic } from "../haptics";

export interface SwipeAction {
  label: string;
  /** Danger is the only tone that changes anything: it is the one that has to be
   *  harder to do by accident, and it is red because that is what red is for. */
  danger?: boolean;
  run: () => void;
}

/** How far the row opens per action revealed. */
const OPEN_PX = 96;
/** Past this, letting go runs the action without aiming at it. One action only.
 *  Measured on the finger, not on the row: past `open` the row moves less than
 *  the finger does (see `RESIST`), and a commit point measured on the row would
 *  drift away as the resistance grew. */
const COMMIT_PX = 190;
/** How far the finger goes before the row does.
 *
 *  Without it the row started moving on the first pixel, which is a row with no
 *  weight: it slid out from under a thumb that was on its way to a tap, and a
 *  list scrolled at a slight angle came apart. This is the same slop the
 *  platform's own rows have — enough to say "I meant sideways", little enough
 *  that meaning it costs nothing. */
const SLOP_PX = 12;
/** How much of the finger's travel the row keeps, once it is fully open.
 *
 *  Past the actions there is nothing more to reveal, so the row stops being
 *  dragged and starts being *pulled*: it follows at a third of the speed, which
 *  is what a rubber band feels like and what tells a thumb it has reached the
 *  end without a line having to be drawn there. */
const RESIST = 0.32;

export function SwipeRow({
  action,
  actions,
  children,
}: {
  /** The single-action shorthand, kept for the rows that have exactly one. */
  action?: SwipeAction;
  /** Two, at most, in the order they are revealed from the edge inwards. */
  actions?: SwipeAction[];
  children: ReactNode;
}) {
  const [dx, setDx] = useState(0);
  const [snap, setSnap] = useState(true);
  const start = useRef({ x: 0, y: 0 });
  /** Which way this gesture turned out to be about. Decided once, on the first
   *  movement big enough to have a direction, and then held: a gesture that
   *  changes its mind halfway is a row that jitters while somebody scrolls. */
  const axis = useRef<"?" | "x" | "y">("?");
  /** How far the finger has gone, before any resistance is applied. What the
   *  commit is measured against. */
  const pulled = useRef(0);

  const list = (actions ?? (action ? [action] : [])).slice(0, 2);
  if (list.length === 0) return <>{children}</>;

  const open = OPEN_PX * list.length;
  // Committing on a long pull only makes sense when there is one thing it could
  // mean.
  const commitAt = list.length === 1 ? COMMIT_PX : Infinity;

  const onStart = (e: React.TouchEvent) => {
    const t = e.touches[0];
    // Where the row already is, so a second pull continues rather than restarts.
    start.current = { x: (t?.clientX ?? 0) + dx, y: t?.clientY ?? 0 };
    axis.current = "?";
    pulled.current = dx;
    setSnap(false);
  };
  const onMove = (e: React.TouchEvent) => {
    if (axis.current === "y") return;
    const t = e.touches[0];
    if (!t) return;
    const moved = t.clientX - start.current.x;
    const lifted = t.clientY - start.current.y;

    if (axis.current === "?") {
      // Not yet either. A scroll starts as a vertical move and must keep the
      // gesture; a swipe has to have gone further sideways than up or down, and
      // further than the slop, before it takes it.
      if (Math.abs(lifted) > SLOP_PX && Math.abs(lifted) > Math.abs(moved)) {
        axis.current = "y";
        return;
      }
      if (Math.abs(moved) < SLOP_PX) return;
      axis.current = "x";
    }

    // The slop is given back, so the row starts from where it was rather than
    // jumping the twelve pixels that decided the gesture.
    const raw = Math.min(0, moved + SLOP_PX);
    pulled.current = raw;
    // Only leftwards: a right swipe on this platform belongs to the stack.
    const over = -raw - open;
    const next = over > 0 ? -(open + over * RESIST) : raw;
    setDx(Math.max(-(open + 90), next));
  };
  const onEnd = () => {
    setSnap(true);
    if (pulled.current <= -commitAt) {
      setDx(0);
      // The one gesture in the app that acts without a button being pressed:
      // it has to say so through the finger, because the eye is on the row and
      // the row is what disappears.
      haptic.pressed();
      list[0].run();
      return;
    }
    setDx(dx <= -open / 2 ? -open : 0);
  };

  return (
    <div className={`swipe ${list.length > 1 ? "two" : ""}`}>
      <div className="swipe-actions" aria-hidden={dx ? undefined : true}>
        {list.map((a) => (
          <button
            key={a.label}
            className={`swipe-action ${a.danger ? "danger" : ""}`}
            style={{ width: OPEN_PX }}
            onClick={() => {
              setDx(0);
              haptic.pressed();
              a.run();
            }}
            tabIndex={dx ? 0 : -1}
          >
            {a.label}
          </button>
        ))}
      </div>
      <div
        className="swipe-face"
        style={{
          transform: `translateX(${dx}px)`,
          // The platform's curve and its length: a snap that takes 180ms reads as a
          // web page reacting, not as a row settling.
          transition: snap ? "transform 320ms cubic-bezier(0.32,0.72,0,1)" : "none",
        }}
        onTouchStart={onStart}
        onTouchMove={onMove}
        onTouchEnd={onEnd}
        onTouchCancel={onEnd}
      >
        {children}
      </div>
    </div>
  );
}
