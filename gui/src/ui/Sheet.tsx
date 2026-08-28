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
import { createPortal } from "react-dom";
import { useT } from "../i18n";
import { useBackDismiss } from "../backstack";
import { useIsPhone } from "../phone";
import { Icon } from "./Icons";
import { Button, IconButton } from "./Primitives";

const FOCUSABLE =
  'a[href],button:not([disabled]),textarea:not([disabled]),input:not([disabled]),select:not([disabled]),[tabindex]:not([tabindex="-1"])';

export function useModal(
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
  /** The same decision, said the way a phone says it: one control on the left of
   *  the top bar and one on the right, with the title between them. Given, a
   *  phone uses this and drops the footer; a window ignores it and keeps the
   *  footer, where a decision belongs when there is room under what it acts on. */
  bar?: { cancel?: ReactNode; confirm?: ReactNode };
  /** On a phone, arrive as a pushed screen — sliding in from the right, with a
   *  back button naming this label — rather than as a modal. For *content* the
   *  user drilled into (a contact, a share); a task to complete or abandon
   *  stays modal, which is the platform's own split. The way out goes through
   *  the history, so the system's back gesture and the button are one step.
   *  Ignored on a desktop, where the same sheet stays the modal it was. */
  pushFrom?: string;
  /** Extra control in the header, left of the close button. */
  headerAction?: ReactNode;
  /** Whether clicking the backdrop closes (default: yes).
   *
   *  Turn it off for a panel that changes shape while it is being used: a click
   *  aimed at a control that has just been replaced would land on the scrim and
   *  read as the dialog closing itself. Escape, the × and the footer's own way
   *  out are unaffected, so nothing becomes inescapable. */
  dismissOnScrim?: boolean;
}

export function Sheet({
  open,
  onClose,
  title,
  subtitle,
  placement = "side",
  children,
  footer,
  bar,
  headerAction,
  pushFrom,
  dismissOnScrim = true,
}: SheetProps) {
  const t = useT();
  const isPhone = useIsPhone();
  const ref = useRef<HTMLDivElement>(null);
  useModal(ref, onClose, open);
  // Escape closes this on a desktop; the phone's equivalent is the system's
  // back, and it has to reach here rather than the screen underneath.
  useBackDismiss(open && isPhone, onClose);
  if (!open) return null;
  const pushed = isPhone && !!pushFrom;
  // Through a portal, whatever subtree asked for the sheet. `position: fixed`
  // is only fixed to the viewport until an ancestor has a filter, a backdrop
  // blur or a transform — then that ancestor becomes the containing block, and
  // this app now has all three: a blurred bar, and pages that transform while
  // they slide. A sheet opened from inside the bar was drawn *inside* the bar.
  return createPortal(
    <>
      {/* A pushed screen has no scrim: nothing shows around it to dim. */}
      {!pushed && (
        <div className="scrim" onClick={dismissOnScrim ? onClose : undefined} />
      )}
      <div
        ref={ref}
        className={`sheet ${pushed ? "pushscreen" : placement}`}
        role="dialog"
        aria-modal="true"
        aria-label={typeof title === "string" ? title : undefined}
        tabIndex={-1}
      >
        {/* Two heads, one for each platform's idea of where a decision lives.
            A window puts the buttons at the bottom of the sheet, under what they
            act on; a phone puts them in the bar at the top, one on each side,
            and gives the middle to the title. `bar` is a sheet saying it knows
            how to be both — without it, the footer is used on both. And a
            pushed screen gets neither: it gets the stack's own bar, back on the
            left naming where it goes. */}
        {pushed ? (
          <div className="sheet-full-bar">
            <button
              className="nav-back"
              onClick={() =>
                window.history.length > 1 ? window.history.back() : onClose()
              }
            >
              <Icon.ChevronLeft size={19} />
              <span className="truncate">{pushFrom}</span>
            </button>
            <h2 className="truncate">{title}</h2>
            <span className="bar-pad">{headerAction}</span>
          </div>
        ) : bar && isPhone ? (
          <div className="sheet-head sheet-bar">
            <span className="bar-left">{bar.cancel}</span>
            <span className="t-head truncate">{title}</span>
            <span className="bar-right">{bar.confirm}</span>
          </div>
        ) : (
          <div className="sheet-head">
            <div className="grow">
              <div className="t-head">{title}</div>
              {subtitle && <div className="hint">{subtitle}</div>}
            </div>
            {headerAction}
            <IconButton label={t("common.close")} onClick={onClose}>
              <Icon.Close />
            </IconButton>
          </div>
        )}
        <div className="sheet-body">{children}</div>
        {footer && !(bar && isPhone) && (
          <div className="sheet-foot">{footer}</div>
        )}
      </div>
    </>,
    document.body
  );
}

interface ConfirmProps {
  open: boolean;
  title: string;
  /** The consequence, in plain words. A confirm that only restates the verb
   *  ("Delete this?") tells the user nothing they didn't already know. */
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
  confirmLabel,
  cancelLabel,
  danger,
  busy,
  onConfirm,
  onCancel,
}: ConfirmProps) {
  const t = useT();
  const isPhone = useIsPhone();
  const ref = useRef<HTMLDivElement>(null);
  const close = useCallback(() => {
    if (!busy) onCancel();
  }, [busy, onCancel]);
  useModal(ref, close, open);
  // Back dismisses an alert the way Cancel does — never the way Confirm does.
  useBackDismiss(open && isPhone, close);
  if (!open) return null;
  // Same portal as `Sheet`, same reason: an ancestor with a filter or a
  // transform would otherwise become this dialog's viewport.
  return createPortal(
    <>
      <div className="scrim" onClick={close} />
      {/* `alert`, so the phone does not give this the full-screen treatment
          every other sheet gets there: a confirmation is the one modal that
          platform keeps small — a card in the middle, over what it asks about,
          because the thing being confirmed is the context. */}
      <div
        ref={ref}
        className="sheet center alert"
        role="alertdialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        style={isPhone ? undefined : { width: "min(440px, calc(100vw - 48px))" }}
      >
        <div className="sheet-body">
          <div className="t-head">{title}</div>
          <div className="t-sm t-sec" style={{ marginTop: -8, lineHeight: 1.5 }}>
            {body}
          </div>
        </div>
        <div className="sheet-foot">
          <div className="spacer" />
          {/* Focus lands on the way *out*, not on the irreversible choice: a
              dialog that opens with Enter armed on "Revoca" turns a reflex into
              a decision. */}
          <Button onClick={close} disabled={busy} data-autofocus>
            {cancelLabel ?? t("common.cancel")}
          </Button>
          <Button
            variant={danger ? "danger" : "primary"}
            onClick={onConfirm}
            busy={busy}
          >
            {confirmLabel ?? t("common.confirm")}
          </Button>
        </div>
      </div>
    </>,
    document.body
  );
}
