// Modal surfaces: the side sheet you compose in, and the centred dialog you
// decide in.
//
// Both are real modals, which in a desktop app means three things people notice
// only when they are missing: Escape closes, focus moves in on open and comes
// back to where it was on close, and Tab cannot walk out of the panel into the
// board behind it. The trap is hand-rolled rather than pulled in because it is
// twenty lines and a dependency here would be a dependency in the CSP-restricted
// bundle forever.

import {
  useCallback,
  useEffect,
  useRef,
  type ReactNode,
} from "react";
import { Icon } from "./Icons";
import { Button, IconButton } from "./Primitives";

const FOCUSABLE =
  'a[href],button:not([disabled]),textarea:not([disabled]),input:not([disabled]),select:not([disabled]),[tabindex]:not([tabindex="-1"])';

function useModal(
  ref: React.RefObject<HTMLElement>,
  onClose: () => void,
  open: boolean
) {
  // Where focus was before we took it, so it can be handed back. Without this,
  // closing a sheet drops focus to <body> and the next Tab restarts from the top
  // of the window — which, for a keyboard user, loses their place entirely.
  const restoreTo = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;
    restoreTo.current = document.activeElement as HTMLElement | null;
    const el = ref.current;
    if (el) {
      const first = el.querySelector<HTMLElement>(
        "[data-autofocus]"
      );
      (first ?? el.querySelector<HTMLElement>(FOCUSABLE) ?? el).focus();
    }
    return () => restoreTo.current?.focus?.();
  }, [open, ref]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== "Tab") return;
      const el = ref.current;
      if (!el) return;
      const items = Array.from(el.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
        (n) => n.offsetParent !== null
      );
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      // Wrap at both ends. `document.activeElement` rather than e.target: focus
      // may be on a node inside a shadow-ish wrapper that isn't the event target.
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [open, onClose, ref]);
}

interface SheetProps {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  /** Sub-line under the title — say what this panel is for, not what it is. */
  subtitle?: ReactNode;
  /** `side` to compose alongside the board, `center` to decide. */
  placement?: "side" | "center";
  children: ReactNode;
  footer?: ReactNode;
  /** Extra control in the header, left of the close button. */
  headerAction?: ReactNode;
}

export function Sheet({
  open,
  onClose,
  title,
  subtitle,
  placement = "side",
  children,
  footer,
  headerAction,
}: SheetProps) {
  const ref = useRef<HTMLDivElement>(null);
  useModal(ref, onClose, open);
  if (!open) return null;
  return (
    <>
      <div className="scrim" onClick={onClose} />
      <div
        ref={ref}
        className={`sheet ${placement}`}
        role="dialog"
        aria-modal="true"
        aria-label={typeof title === "string" ? title : undefined}
        tabIndex={-1}
      >
        <div className="sheet-head">
          <div className="grow">
            <div className="t-head">{title}</div>
            {subtitle && <div className="hint">{subtitle}</div>}
          </div>
          {headerAction}
          <IconButton label="Chiudi" onClick={onClose}>
            <Icon.Close />
          </IconButton>
        </div>
        <div className="sheet-body">{children}</div>
        {footer && <div className="sheet-foot">{footer}</div>}
      </div>
    </>
  );
}

interface ConfirmProps {
  open: boolean;
  title: string;
  /** The consequence, in plain words. A confirm that only restates the verb
   *  ("Vuoi eliminare?") tells the user nothing they didn't already know. */
  body: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function Confirm({
  open,
  title,
  body,
  confirmLabel = "Conferma",
  cancelLabel = "Annulla",
  danger,
  busy,
  onConfirm,
  onCancel,
}: ConfirmProps) {
  const ref = useRef<HTMLDivElement>(null);
  const close = useCallback(() => {
    if (!busy) onCancel();
  }, [busy, onCancel]);
  useModal(ref, close, open);
  if (!open) return null;
  return (
    <>
      <div className="scrim" onClick={close} />
      <div
        ref={ref}
        className="sheet center"
        role="alertdialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        style={{ width: "min(440px, calc(100vw - 48px))" }}
      >
        <div className="sheet-body">
          <div className="t-head">{title}</div>
          <div className="t-sm t-sec" style={{ marginTop: -8, lineHeight: 1.5 }}>
            {body}
          </div>
        </div>
        <div className="sheet-foot">
          <div className="spacer" />
          <Button onClick={close} disabled={busy}>
            {cancelLabel}
          </Button>
          <Button
            variant={danger ? "danger" : "primary"}
            onClick={onConfirm}
            busy={busy}
            data-autofocus
          >
            {confirmLabel}
          </Button>
        </div>
      </div>
    </>
  );
}
