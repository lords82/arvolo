// Small, unopinionated building blocks. Each one is a class name from theme.css
// plus the accessibility wiring that is easy to forget and expensive to omit:
// `aria-checked` on the switch, `aria-selected` on the segmented control,
// `aria-describedby` linking a field to its hint and its error.
//
// Nothing here holds state beyond what a control needs to be a control. Anything
// that talks to the daemon lives in `store.ts`; anything that lays out a screen
// lives in `views/`.

import {
  forwardRef,
  useId,
  type ButtonHTMLAttributes,
  type InputHTMLAttributes,
  type ReactNode,
  type TextareaHTMLAttributes,
} from "react";
import { Icon } from "./Icons";

// ---- Button ---------------------------------------------------------------

type Variant = "default" | "primary" | "in" | "ghost" | "danger";
type Size = "sm" | "md" | "lg";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  block?: boolean;
  /** Shows a spinner and disables the button. The label stays put so the button
   *  does not change width mid-click — a moving target after you've committed. */
  busy?: boolean;
}

const VARIANT: Record<Variant, string> = {
  default: "",
  primary: "btn-primary",
  in: "btn-in",
  ghost: "btn-ghost",
  danger: "btn-danger",
};
const SIZE: Record<Size, string> = { sm: "btn-sm", md: "", lg: "btn-lg" };

export function Button({
  variant = "default",
  size = "md",
  block,
  busy,
  className = "",
  children,
  disabled,
  ...rest
}: ButtonProps) {
  const cls = [
    "btn",
    VARIANT[variant],
    SIZE[size],
    block ? "btn-block" : "",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button className={cls} disabled={disabled || busy} {...rest}>
      {busy && <span className="spinner" />}
      {children}
    </button>
  );
}

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** Required: an icon-only control with no accessible name is invisible to a
   *  screen reader and unlabelled on hover. It doubles as the tooltip. */
  label: string;
  children: ReactNode;
}

export function IconButton({
  label,
  children,
  className = "",
  ...rest
}: IconButtonProps) {
  return (
    <button
      className={`icon-btn ${className}`}
      aria-label={label}
      title={label}
      {...rest}
    >
      {children}
    </button>
  );
}

// ---- Fields ---------------------------------------------------------------

interface FieldProps {
  label?: string;
  hint?: ReactNode;
  error?: string | null;
  children: (ids: { id: string; describedBy?: string }) => ReactNode;
}

/** Label + control + hint + error, wired together by id.
 *
 *  The render-prop is what makes the wiring real rather than decorative: the
 *  control receives the very id the `<label>` points at, and an
 *  `aria-describedby` naming whichever of hint/error is actually on screen. */
export function Field({ label, hint, error, children }: FieldProps) {
  const id = useId();
  const hintId = hint ? `${id}-hint` : undefined;
  const errId = error ? `${id}-err` : undefined;
  const describedBy = [errId, hintId].filter(Boolean).join(" ") || undefined;
  return (
    <div className="field">
      {label && <label htmlFor={id}>{label}</label>}
      {children({ id, describedBy })}
      {error && (
        <div className="err" id={errId} role="alert">
          {error}
        </div>
      )}
      {hint && (
        <div className="hint" id={hintId}>
          {hint}
        </div>
      )}
    </div>
  );
}

export const TextInput = forwardRef<
  HTMLInputElement,
  InputHTMLAttributes<HTMLInputElement> & { big?: boolean }
>(function TextInput({ className = "", big, ...rest }, ref) {
  return (
    <input
      ref={ref}
      className={`input ${big ? "input-lg" : ""} ${className}`}
      {...rest}
    />
  );
});

export const Textarea = forwardRef<
  HTMLTextAreaElement,
  TextareaHTMLAttributes<HTMLTextAreaElement>
>(function Textarea({ className = "", ...rest }, ref) {
  return <textarea ref={ref} className={`textarea ${className}`} {...rest} />;
});

// ---- Switch ---------------------------------------------------------------

export function Switch({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  label: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      className="switch"
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}

/** A switch with its explanation — the shape a settings screen is made of. */
export function SwitchRow({
  title,
  desc,
  checked,
  onChange,
  disabled,
}: {
  title: string;
  desc?: ReactNode;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <div className="switch-row">
      <div className="grow">
        <div style={{ fontWeight: 570 }}>{title}</div>
        {desc && <div className="hint">{desc}</div>}
      </div>
      <Switch
        checked={checked}
        onChange={onChange}
        disabled={disabled}
        label={title}
      />
    </div>
  );
}

// ---- Segmented ------------------------------------------------------------

export function Segmented<T extends string>({
  value,
  onChange,
  options,
  block,
  label,
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
  block?: boolean;
  label: string;
}) {
  return (
    // `radiogroup`, not `tablist`: these pick one of several alternatives and
    // there are no tabpanels for tabs to control. A screen reader announcing
    // "tab 2 of 4" for a TTL choice is describing a widget that isn't there.
    <div
      className={`segmented ${block ? "segmented-block" : ""}`}
      role="radiogroup"
      aria-label={label}
    >
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          aria-selected={value === o.value}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

// ---- Badge ----------------------------------------------------------------

type BadgeKind =
  | "neutral"
  | "ok"
  | "warn"
  | "bad"
  | "info"
  | "out"
  | "trust"
  | "dev";

export function Badge({
  kind = "neutral",
  children,
  title,
}: {
  kind?: BadgeKind;
  children: ReactNode;
  title?: string;
}) {
  return (
    <span
      className={`badge ${kind === "neutral" ? "" : kind}`}
      title={title}
    >
      {children}
    </span>
  );
}

/** The three marks a contact can carry, in one place so they never drift apart
 *  between the people grid, the transfer row and the incoming dialog. */
export function TrustBadges({
  verified,
  trusted,
  blocked,
}: {
  verified?: boolean;
  trusted?: boolean;
  blocked?: boolean;
}) {
  return (
    <>
      {blocked && (
        <Badge kind="bad" title="Le sue offerte vengono scartate all'arrivo">
          <Icon.Ban size={10} /> Bloccato
        </Badge>
      )}
      {verified && (
        <Badge kind="ok" title="Impronta confermata fuori banda">
          <Icon.Shield size={10} /> Verificato
        </Badge>
      )}
      {trusted && (
        <Badge kind="trust" title="I suoi file si scaricano senza chiedere">
          <Icon.Star size={10} /> Fidato
        </Badge>
      )}
    </>
  );
}

// ---- Empty state ----------------------------------------------------------

export function Empty({
  icon,
  title,
  children,
  action,
}: {
  icon: ReactNode;
  title: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <div className="glyph">{icon}</div>
      <h3>{title}</h3>
      {children && <p>{children}</p>}
      {action && <div style={{ marginTop: 8 }}>{action}</div>}
    </div>
  );
}

// ---- Keyboard hint --------------------------------------------------------

const IS_MAC =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

/** Renders `mod` as ⌘ or Ctrl for the platform actually running the app. */
export function Kbd({ children }: { children: string }) {
  const text = children === "mod" ? (IS_MAC ? "⌘" : "Ctrl") : children;
  return <span className="kbd">{text}</span>;
}

export const modKey = IS_MAC ? "⌘" : "Ctrl";
