// The three rows a grouped list is made of.
//
// This platform's settings are not cards with labelled fields: they are lists of
// rows, and a row does one of three things — it pushes somewhere, it holds a
// value you can type over, or it carries a switch. Everything the desktop grows
// around a control (a caption above, a Save beside, a border) is absent, because
// on a phone the row *is* the control and the sentence explaining it goes under
// the whole group.
//
// Kept here rather than in each screen because there are now four screens made
// of these, and a list whose rows are 47px on one screen and 44 on another reads
// as two different apps.

import type { ReactNode } from "react";
import { Icon } from "./Icons";
import { Switch } from "./Primitives";

/** A row that pushes, with what is behind it said on the right — the detail text
 *  and the chevron this platform has always used to mean "there is more of this,
 *  one screen in". */
export function Place({
  label,
  note,
  onClick,
}: {
  label: string;
  note?: string;
  onClick: () => void;
}) {
  return (
    <button className="ios-row" onClick={onClick}>
      <span className="grow truncate">{label}</span>
      {note ? <span className="ios-detail truncate">{note}</span> : null}
      <Icon.ChevronRight size={15} className="ios-chev" />
    </button>
  );
}

/** A row that only says something. `onClick` makes it act — copying, usually —
 *  and without one it is a plain line of fact, not a button pretending. */
export function ValueRow({
  label,
  value,
  mono,
  onClick,
}: {
  label: string;
  value: ReactNode;
  mono?: boolean;
  onClick?: () => void;
}) {
  const inner = (
    <>
      <span className="grow truncate">{label}</span>
      <span className={`ios-detail truncate ${mono ? "mono" : ""}`}>
        {value}
      </span>
    </>
  );
  return onClick ? (
    <button className="ios-row" onClick={onClick}>
      {inner}
    </button>
  ) : (
    <div className="ios-row" style={{ cursor: "default" }}>
      {inner}
    </div>
  );
}

/** A row you type in. The value lives on the right where the detail text would
 *  be, so a list of them still reads as a list — and it commits on blur, because
 *  this platform has no Save button next to a field: leaving the field is what
 *  saving is. */
export function InputRow({
  label,
  value,
  placeholder,
  disabled,
  mono,
  maxLength,
  onChange,
  onCommit,
}: {
  label: string;
  value: string;
  placeholder?: string;
  disabled?: boolean;
  mono?: boolean;
  maxLength?: number;
  onChange: (v: string) => void;
  onCommit: () => void;
}) {
  return (
    <label className="ios-row">
      <span className="ios-row-label">{label}</span>
      <input
        className={`ios-value ${mono ? "mono" : ""}`}
        value={value}
        disabled={disabled}
        maxLength={maxLength}
        placeholder={placeholder}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        onChange={(e) => onChange(e.currentTarget.value)}
        onBlur={onCommit}
        // Return closes the keyboard, and closing the keyboard is what commits.
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
      />
    </label>
  );
}

/** A row with a switch at its end. The explanation goes under the group, not
 *  under the title: two lines inside a row is what this platform draws for a
 *  *choice*, and a switch is not one. */
export function ToggleRow({
  label,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="ios-row" style={{ cursor: "default" }}>
      <span className="grow wrap-label">{label}</span>
      <Switch
        checked={checked}
        disabled={disabled}
        onChange={onChange}
        label={label}
      />
    </div>
  );
}
