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

export interface MenuItem {
  key: string;
  label: string;
  icon?: ReactNode;
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
    const m = el.getBoundingClientRect();
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

  return (
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
              {item.label}
            </button>
          </div>
        );
      })}
    </div>
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
