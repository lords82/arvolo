// Everything you can do to one row, said with its consequences.
//
// The phone's answer to the window's row menu, and deliberately not the same
// shape. A menu hangs off a glyph, fits a verb per line and nothing else, and is
// aimed at with a pointer; this comes up from the bottom edge under the thumb,
// opens with *what the row is* — its name, its state in a word, how far it got —
// and gives every verb that needs one a line saying what it will do. The design
// of record ("Arvolo Mobile 2 — Scambia", 7c) is explicit about the order: the
// state in words, then the actions with their consequence written underneath,
// and the one that throws work away on its own at the bottom.
//
// Reached by holding a row down, or by tapping one that has nothing else to do
// on a tap (the rows of *Lasciati*). Not by a glyph: a row that carries a button
// for its menu carries it on every row forever.

import { useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useT } from "../i18n";
import { useBackDismiss } from "../backstack";
import { SheetGrab, useModal } from "./Sheet";
import { useSheetDrag } from "./sheetDrag";
import type { MenuItem } from "./Menu";

export function ActionSheet({
  open,
  onClose,
  title,
  badge,
  meta,
  note,
  items,
}: {
  open: boolean;
  onClose: () => void;
  /** What the actions are about — a file name, a person. */
  title: string;
  /** The state in one word, with the tone it is drawn in. */
  badge?: { text: string; tone: "ok" | "bad" | "warn" };
  /** Who it is with and how far it got. */
  meta?: string;
  /** The sentence under the state, where there is one worth reading. */
  note?: ReactNode;
  /** The same items the window's row menu is built from: a verb, an optional
   *  line about what it does, and whether it is the destructive one. */
  items: MenuItem[];
}) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  useModal(ref, onClose, open);
  useBackDismiss(open, onClose);
  useSheetDrag(ref, { enabled: open, onDismiss: onClose });
  if (!open) return null;

  const plain = items.filter((i) => !i.danger && !i.disabled);
  const danger = items.filter((i) => i.danger && !i.disabled);

  const row = (i: MenuItem) => (
    <button
      key={i.key}
      className={`act-sheet-row${i.danger ? " danger" : ""}`}
      onClick={() => {
        onClose();
        i.onSelect();
      }}
    >
      {i.icon ? <span className="act-sheet-glyph">{i.icon}</span> : null}
      <span className="grow" style={{ minWidth: 0 }}>
        <span className="act-sheet-label">{i.label}</span>
        {i.desc ? <span className="act-sheet-desc">{i.desc}</span> : null}
      </span>
    </button>
  );

  return createPortal(
    <>
      <div className="scrim" onClick={onClose} />
      <div
        ref={ref}
        className="act-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
      >
        <SheetGrab />
        <div className="act-sheet-head">
          <div className="act-sheet-title">{title}</div>
          {(badge || meta) && (
            <div className="act-sheet-state">
              {badge ? <span className={`badge ${badge.tone}`}>{badge.text}</span> : null}
              {meta ? <span className="tnum truncate">{meta}</span> : null}
            </div>
          )}
          {note ? <div className="act-sheet-note">{note}</div> : null}
        </div>

        {plain.length > 0 && <div className="ios-group">{plain.map(row)}</div>}
        {danger.length > 0 && <div className="ios-group">{danger.map(row)}</div>}

        {/* The way out, named rather than an ×: this sheet is a question and
            "leave it alone" is one of its answers. */}
        <button className="act-sheet-dismiss" onClick={onClose} data-autofocus>
          {t("transfers.leaveAlone")}
        </button>
      </div>
    </>,
    document.body
  );
}
