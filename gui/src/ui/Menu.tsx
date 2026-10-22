// A dropdown menu that behaves like one: arrow keys move, Enter picks, Escape
// closes, a click anywhere else dismisses, and it flips above the trigger when
// there is no room below.
//
// The flip matters more than it sounds. The row menus open from the bottom of a
// long list, and a menu that renders off-screen is a menu whose last two items —
// which are always the destructive ones — cannot be reached.

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useBackDismiss } from "../backstack";

export interface MenuItem {
  key: string;
  label: string;
  icon?: ReactNode;
  /** A second line under the label, for an item whose effect is not obvious
   *  from a verb alone ("Trust" — and then what happens?). The label stays the
   *  verb; this is what the verb used to be dragging along behind it. */
  desc?: string;
  onSelect: () => void;
  disabled?: boolean;
  danger?: boolean;
  /** Draw a rule above this item — for separating the destructive tail. */
  separated?: boolean;
}

export function Menu({
  items,
  onClose,
  anchor,
}: {
  items: MenuItem[];
  onClose: () => void;
  /** The element the menu hangs off. Its rect drives placement. */
  anchor: HTMLElement | null;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const [active, setActive] = useState(0);

  const enabled = items.filter((i) => !i.disabled);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !anchor) return;
    const a = anchor.getBoundingClientRect();
    // Layout size, not the bounding rect: the phone opens the menu through a
    // scale animation, and a bounding rect read on the first frame measures
    // the shrunken menu — which then gets placed as if it were small and
    // overflows the right edge of the screen when it grows.
    const m = { width: el.offsetWidth, height: el.offsetHeight };
    const pad = 8;
    // Below by default; above when the bottom would clip. Right-aligned to the
    // trigger, then pulled back inside the window if that overflows.
    let top = a.bottom + 4;
    if (top + m.height > window.innerHeight - pad) {
      top = Math.max(pad, a.top - m.height - 4);
    }
    let left = a.right - m.width;
    if (left < pad) left = pad;
    if (left + m.width > window.innerWidth - pad) {
      left = window.innerWidth - m.width - pad;
    }
    setPos({ top, left });
  }, [anchor]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const el = ref.current;
      if (el && !el.contains(e.target as Node) && !anchor?.contains(e.target as Node)) {
        onClose();
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        setActive((i) => (i + 1) % Math.max(1, enabled.length));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setActive((i) => (i - 1 + enabled.length) % Math.max(1, enabled.length));
      } else if (e.key === "Enter") {
        e.preventDefault();
        const item = enabled[active];
        if (item) {
          onClose();
          item.onSelect();
        }
      }
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [onClose, anchor, enabled, active]);

  // Through a portal: `fixed` coordinates from `getBoundingClientRect` are
  // viewport coordinates, and they only stay that way if no ancestor carries a
  // filter or a transform — the blurred bar this menu now opens from carries
  // both kinds of trouble.
  return createPortal(
    <div
      ref={ref}
      className="menu"
      role="menu"
      style={{
        top: pos?.top ?? -9999,
        left: pos?.left ?? -9999,
        // Invisible until placed, or the first paint shows it in the wrong spot
        // and it visibly jumps.
        visibility: pos ? "visible" : "hidden",
      }}
    >
      {items.map((item) => {
        const idx = enabled.indexOf(item);
        return (
          <div key={item.key}>
            {item.separated && <hr />}
            <button
              role="menuitem"
              className={item.danger ? "danger" : ""}
              disabled={item.disabled}
              data-active={idx === active}
              onMouseEnter={() => idx >= 0 && setActive(idx)}
              onClick={() => {
                onClose();
                item.onSelect();
              }}
            >
              {item.icon}
              {item.desc ? (
                <span className="grow" style={{ textAlign: "left" }}>
                  {item.label}
                  <span className="t-xs t-mut" style={{ display: "block" }}>
                    {item.desc}
                  </span>
                </span>
              ) : (
                item.label
              )}
            </button>
          </div>
        );
      })}
    </div>,
    document.body
  );
}

/** Wraps a trigger and its menu, owning the open/closed state and the anchor ref.
 *  Callers get a button and a list of items and never touch positioning. */
export function MenuButton({
  items,
  children,
  label,
}: {
  items: MenuItem[];
  children: ReactNode;
  label: string;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  // A click outside closes this, and on a phone the system's back is that
  // click: without a turn at it, back would dismiss the screen behind the menu
  // and leave the menu hanging over whatever arrived.
  useBackDismiss(open, () => setOpen(false));
  return (
    <>
      <button
        ref={ref}
        className="icon-btn"
        aria-label={label}
        title={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        {children}
      </button>
      {open && (
        <Menu items={items} anchor={ref.current} onClose={() => setOpen(false)} />
      )}
    </>
  );
}
