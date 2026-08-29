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

export interface SwipeAction {
  label: string;
  /** Danger is the only tone that changes anything: it is the one that has to be
   *  harder to do by accident, and it is red because that is what red is for. */
  danger?: boolean;
  run: () => void;
}

/** How far the row opens per action revealed. */
const OPEN_PX = 96;
/** Past this, letting go runs the action without aiming at it. One action only. */
const COMMIT_PX = 190;

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
  const start = useRef(0);
  const vertical = useRef(false);

  const list = (actions ?? (action ? [action] : [])).slice(0, 2);
  if (list.length === 0) return <>{children}</>;

  const open = OPEN_PX * list.length;
  // Committing on a long pull only makes sense when there is one thing it could
  // mean.
  const commitAt = list.length === 1 ? COMMIT_PX : Infinity;

  const onStart = (e: React.TouchEvent) => {
    start.current = (e.touches[0]?.clientX ?? 0) - dx;
    vertical.current = false;
    setSnap(false);
  };
  const onMove = (e: React.TouchEvent) => {
    if (vertical.current) return;
    const t = e.touches[0];
    if (!t) return;
    const moved = t.clientX - start.current;
    // A scroll starts as a vertical move; taking it would make the list feel
    // stuck the moment somebody drags at a slight angle.
    if (Math.abs(moved - dx) < 6 && dx === 0) return;
    // Only leftwards: a right swipe on this platform belongs to the stack.
    setDx(Math.max(-commitAt - 40, Math.min(0, moved)));
  };
  const onEnd = () => {
    setSnap(true);
    if (dx <= -commitAt) {
      setDx(0);
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
