// A list row you can swipe to act on.
//
// On this platform a list is not only a set of things to open: it is a set of
// things to *do something to*, and the doing is a swipe. The alternative is a
// menu behind a glyph on every row, which is four taps of chrome per line and
// still hides the one action that matters.
//
// One action per row, deliberately. A row can hold two or three on a big phone
// and none of them can be read while your thumb is over them; the rule here is
// that the row offers the thing you would actually do to *that* row in *that*
// state — pause what is running, open what has arrived, take back what nobody
// collected — and everything else stays one tap in, where it has a name.
//
// It commits by itself past a long pull, which is this platform's own promise:
// keep going and you do not have to aim at the button.

import { useRef, useState, type ReactNode } from "react";

export interface SwipeAction {
  label: string;
  /** Danger is the only tone that changes anything: it is the one that has to be
   *  harder to do by accident, and it is red because that is what red is for. */
  danger?: boolean;
  run: () => void;
}

/** How far the row opens to show its action. */
const OPEN_PX = 96;
/** Past this, letting go runs the action without aiming at it. */
const COMMIT_PX = 190;

export function SwipeRow({
  action,
  children,
}: {
  action?: SwipeAction;
  children: ReactNode;
}) {
  const [dx, setDx] = useState(0);
  const [snap, setSnap] = useState(true);
  const start = useRef(0);
  const vertical = useRef(false);

  if (!action) return <>{children}</>;

  const onStart = (e: React.TouchEvent) => {
    start.current = e.touches[0]?.clientX ?? 0;
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
    if (Math.abs(moved) < 6) return;
    // Only leftwards: a right swipe on this platform belongs to the stack.
    setDx(Math.max(-COMMIT_PX - 40, Math.min(0, moved)));
  };
  const onEnd = () => {
    setSnap(true);
    if (dx <= -COMMIT_PX) {
      setDx(0);
      action.run();
      return;
    }
    setDx(dx <= -OPEN_PX / 2 ? -OPEN_PX : 0);
  };

  return (
    <div className={`swipe ${action.danger ? "danger" : ""}`}>
      <button
        className="swipe-action"
        onClick={() => {
          setDx(0);
          action.run();
        }}
        tabIndex={dx ? 0 : -1}
        aria-hidden={dx ? undefined : true}
      >
        {action.label}
      </button>
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
