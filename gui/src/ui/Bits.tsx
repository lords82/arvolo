// The pieces that carry Arvolo's actual subject matter: identities, codes,
// tickets and progress. They are here rather than in `Primitives` because each
// one encodes a decision about *this* app rather than about widgets in general.

import { useEffect, useRef, useState, type ReactNode } from "react";
import QRCode from "qrcode";
import { useT } from "../i18n";
import { Icon } from "./Icons";
import { IconButton } from "./Primitives";
import { barClass, extOf, extTint, pct, shortId } from "../format";
import type { UITransfer } from "../types";

// ---- Avatar ---------------------------------------------------------------

/** Eight hues spaced around the wheel, all at a lightness that takes white text
 *  in either theme. Picked by hand rather than generated: an even hue sweep puts
 *  two yellows next to each other and they read as the same person. */
const HUES = [
  "#c2540b",
  "#a3341f",
  "#8a3d8f",
  "#4a4fb0",
  "#0a6ba1",
  "#0d6d66",
  "#3f7a2a",
  "#8a6a10",
];

/** Deterministic colour from an identity. The same person is the same colour on
 *  every machine and across restarts, which is the only thing that makes an
 *  avatar worth having: it becomes a second, pre-verbal handle on who this is. */
export function avatarColor(seed: string): string {
  let h = 0;
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) >>> 0;
  return HUES[h % HUES.length];
}

function initials(name: string): string {
  const parts = name.trim().split(/[\s._-]+/).filter(Boolean);
  if (!parts.length) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[1][0]).toUpperCase();
}

export function Avatar({
  name,
  id,
  size = 32,
  ring,
}: {
  name: string;
  /** The public id, when known — it makes the colour stable even if the petname
   *  changes, which is the point. Falls back to the name. */
  id?: string;
  size?: number;
  ring?: "out" | "in";
}) {
  return (
    <span
      className={`avatar ${ring ? `ring-${ring}` : ""}`}
      style={{
        width: size,
        height: size,
        background: avatarColor(id || name),
        fontSize: Math.round(size * 0.38),
      }}
      aria-hidden="true"
    >
      {initials(name)}
    </span>
  );
}

// ---- Copy -----------------------------------------------------------------

/** Copy to the clipboard, reporting whether it worked.
 *
 *  `navigator.clipboard` is unavailable in a few webview configurations and
 *  rejects rather than throwing, so the caller has to be told — a copy button
 *  that silently does nothing is worse than no copy button, because the user
 *  walks away believing they have the code. */
async function writeClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

function useCopy() {
  const [state, setState] = useState<"idle" | "ok" | "fail">("idle");
  const timer = useRef<number>();
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const copy = async (text: string) => {
    setState((await writeClipboard(text)) ? "ok" : "fail");
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setState("idle"), 1800);
  };
  return { state, copy };
}

export function CopyButton({
  value,
  label,
}: {
  value: string;
  /** Overrides the idle label; the copied/failed states always speak for
   *  themselves, because that is the information the button exists to give. */
  label?: string;
}) {
  const t = useT();
  const { state, copy } = useCopy();
  return (
    <IconButton
      label={
        state === "ok"
          ? t("common.copied")
          : state === "fail"
            ? t("common.copyFailed")
            : (label ?? t("common.copy"))
      }
      onClick={() => copy(value)}
    >
      {state === "ok" ? (
        <Icon.Check className="tone-ok" />
      ) : state === "fail" ? (
        <Icon.Alert className="tone-bad" />
      ) : (
        <Icon.Copy />
      )}
    </IconButton>
  );
}

/** A one-line value with a copy affordance — ids, links, tickets. */
export function CopyField({
  value,
  wrap,
  mono = true,
}: {
  value: string;
  wrap?: boolean;
  mono?: boolean;
}) {
  return (
    <div className={`copyfield ${wrap ? "wrap" : ""}`}>
      <code className={mono ? "mono" : ""}>{value}</code>
      <CopyButton value={value} />
    </div>
  );
}

/** The hero treatment for something a person will read out loud: a pairing code,
 *  or a ticket. Large, spaced, selectable, with the QR right there — because the
 *  alternative to reading twelve characters over a phone is pointing a camera. */
export function CodeHero({
  value,
  small,
  qr = true,
  caption,
}: {
  value: string;
  /** For long values (tickets), which cannot be read out and only need scanning. */
  small?: boolean;
  qr?: boolean;
  caption?: ReactNode;
}) {
  const t = useT();
  const { state, copy } = useCopy();
  return (
    <div className="code-hero">
      {qr && <QrCode value={value} size={148} />}
      <div className={`value ${small ? "sm" : ""}`}>{value}</div>
      {caption && <div className="hint" style={{ textAlign: "center" }}>{caption}</div>}
      <button className="btn btn-sm" onClick={() => copy(value)}>
        {state === "ok" ? <Icon.Check size={13} /> : <Icon.Copy size={13} />}
        {state === "ok"
          ? t("common.copied")
          : state === "fail"
            ? t("common.copyFailed")
            : t("common.copy")}
      </button>
    </div>
  );
}

// ---- QR -------------------------------------------------------------------

/** Rendered to a canvas by the bundled `qrcode` library — no network, which the
 *  Artifact-style CSP on the Tauri webview requires anyway.
 *
 *  Drawn in flat black on white regardless of theme: a scanner needs contrast
 *  and a fixed polarity, and an inverted QR is a QR many phone cameras refuse. */
export function QrCode({ value, size = 148 }: { value: string; size?: number }) {
  const t = useT();
  const ref = useRef<HTMLCanvasElement>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let alive = true;
    QRCode.toCanvas(el, value, {
      width: size,
      margin: 1,
      color: { dark: "#000000", light: "#ffffff" },
    })
      .then(() => alive && setFailed(false))
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, [value, size]);
  if (failed) {
    // An arvm… ticket at ~300 chars is exactly the case most likely to overflow
    // the QR capacity — vanishing silently made the panel look broken. Say why,
    // and point at the copy button that always works.
    return <div className="t-sm t-mut">{t("bits.qrTooDense")}</div>;
  }
  return (
    <div className="qr">
      <canvas ref={ref} width={size} height={size} />
    </div>
  );
}

// ---- Identity -------------------------------------------------------------

/** A word fingerprint. Never truncated: this is the string two people compare
 *  out loud to decide whether a key is the right one, and a fingerprint with an
 *  ellipsis in it cannot do that job. */
export function Fingerprint({ value }: { value: string }) {
  if (!value) return null;
  return <span className="fingerprint">{value}</span>;
}

/** A public id, shortened for display but copyable in full. */
export function ShortId({ value }: { value: string }) {
  return (
    <span className="mono t-xs t-mut" title={value}>
      {shortId(value)}
    </span>
  );
}

// ---- Transfer bits --------------------------------------------------------

export function ExtChip({ name }: { name: string }) {
  const ext = extOf(name);
  return <span className={`ext tint-${extTint(ext)}`}>{ext}</span>;
}

export function Progress({ t: tx }: { t: UITransfer }) {
  const t = useT();
  const value = pct(tx);
  // A send with no size yet (packing, or an offer not opened) has nothing honest
  // to report as a percentage; show motion instead of a bar stuck at zero.
  const indeterminate = tx.status === "active" && !tx.size;
  return (
    <div
      className={`${barClass(tx)} ${indeterminate ? "indet" : ""}`}
      role="progressbar"
      aria-valuenow={indeterminate ? undefined : value}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-label={t("transfers.progressOf", tx.name)}
    >
      <span style={{ width: `${indeterminate ? 32 : value}%` }} />
    </div>
  );
}
