// Presentation helpers: byte/size formatting, id shortening, and the colour/label
// metadata maps ported from the mock's `renderVals`.

import { t } from "./i18n";
import type { DepositDto, UIStatus, UITransfer } from "./types";

export function fmtBytes(bytes: number): string {
  if (!bytes && bytes !== 0) return "";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let n = bytes;
  while (n >= 1024 && i < u.length - 1) {
    n /= 1024;
    i++;
  }
  return (n < 10 && i > 0 ? n.toFixed(1) : Math.round(n)) + " " + u[i];
}

export function extOf(name: string): string {
  const p = (name || "").split(".");
  const ext = (p.length > 1 ? p.pop()! : "").toUpperCase().slice(0, 4);
  // A name like "." or "x." splits to an empty suffix — an empty chip reads as a
  // rendering fault. Fall back to the same label as a name with no extension.
  return ext || "FILE";
}

/** Shorten a base32 id to `if2xmne…c7daha`, matching the CLI/mock. */
export function shortId(id: string): string {
  if (id.length <= 16) return id;
  return `${id.slice(0, 7)}…${id.slice(-6)}`;
}

/** A semantic colour slot, not a colour.
 *
 *  These names are resolved to actual values by `.tone-*` / `.tint-*` in
 *  theme.css, which means every one of them has a light *and* a dark reading.
 *  This module used to return hex literals; that made the whole status
 *  vocabulary unthemeable, because a `#16a34a` baked into a style attribute
 *  cannot know which theme it landed in. */
export type Tone = "out" | "in" | "ok" | "warn" | "bad" | "mut" | "violet";

export interface StatusMeta {
  tone: Tone;
  text: string;
}
export function statusMeta(s: UIStatus): StatusMeta {
  switch (s) {
    case "active":
      return { tone: "out", text: t("status.active") };
    // Muted on purpose: nothing is happening, and nothing is wrong. A tone that
    // asked for attention would make every file you ever downloaded ask for it.
    case "sharing":
      return { tone: "mut", text: t("status.sharing") };
    case "completed":
      return { tone: "ok", text: t("status.completed") };
    case "deposited":
      return { tone: "in", text: t("status.deposited") };
    case "paused":
      return { tone: "warn", text: t("status.paused") };
    case "incoming":
      return { tone: "warn", text: t("status.incoming") };
    case "stalled":
      return { tone: "mut", text: t("status.stalled") };
    case "failed":
      return { tone: "bad", text: t("status.failed") };
    case "cancelling":
      return { tone: "mut", text: t("status.cancelling") };
    case "cancelled":
      return { tone: "mut", text: t("status.cancelled") };
  }
}

/** File-kind tint, as a tone. Grouped by what the file *is* rather than by
 *  extension family, so a glance at a list separates media from documents from
 *  archives without anyone having to learn a legend. */
const EXT_TINT: Record<string, Tone> = {
  ZIP: "out",
  TAR: "mut",
  GZ: "mut",
  "7Z": "out",
  RAR: "out",
  MOV: "ok",
  MP4: "ok",
  MKV: "ok",
  AVI: "ok",
  MP3: "in",
  WAV: "in",
  FLAC: "in",
  PDF: "bad",
  DOC: "in",
  DOCX: "in",
  XLS: "ok",
  XLSX: "ok",
  KEY: "violet",
  PPT: "out",
  PPTX: "out",
  JPG: "warn",
  JPEG: "warn",
  PNG: "warn",
  HEIC: "warn",
  GIF: "warn",
  SVG: "violet",
};
export function extTint(ext: string): Tone {
  // Own keys only: a bare index would walk the prototype chain, where
  // "toString"/"valueOf" are truthy hits for extensions nobody defined.
  return Object.prototype.hasOwnProperty.call(EXT_TINT, ext)
    ? EXT_TINT[ext]
    : "mut";
}

/** Progress-bar modifier classes: direction, plus the state that overrides it.
 *  A finished bar is green and a failed one red regardless of which way it went —
 *  at that point the outcome is the only thing left worth encoding. */
export function barClass(tx: UITransfer): string {
  const dir = tx.dir === "out" ? "out" : "in";
  if (tx.status === "completed") return `prog ${dir} done`;
  if (tx.status === "failed") return `prog ${dir} bad`;
  if (tx.status === "stalled" || tx.status === "paused")
    return `prog ${dir} stall`;
  // A share is complete by definition — the file is all there, waiting. Drawing it
  // as a bar invites reading a full one as "done" and an empty one as "stuck",
  // which is exactly how a seed row came to look like a failed send.
  if (tx.status === "sharing") return `prog ${dir} stall`;
  return `prog ${dir}`;
}

export function pct(tx: UITransfer): number {
  if (!tx.size) return 0;
  return Math.min(100, Math.round((tx.transferred / tx.size) * 100));
}

/** How long ago a unix timestamp was, in words. Empty for 0 — which every caller
 *  uses to mean "never happened", and which must not render as "moments ago".
 *
 *  Every branch can land on exactly 1, so none of them hardcodes the plural: each
 *  dictionary decides where its own singular is. */
export function fmtAgo(unixSecs: number, nowMs: number = Date.now()): string {
  if (!unixSecs) return "";
  const secs = Math.max(0, Math.floor(nowMs / 1000) - unixSecs);
  if (secs < 60) return t("ago.moments");
  if (secs < 3600) return t("ago.minutes", Math.round(secs / 60));
  if (secs < 86400) return t("ago.hours", Math.round(secs / 3600));
  return t("ago.days", Math.round(secs / 86400));
}

/** Human throughput, e.g. "42 MB/s". */
export function fmtRate(bytesPerSec: number): string {
  return fmtBytes(bytesPerSec) + "/s";
}

/** An estimate further out than this is not information, it is noise: a transfer
 *  that would take a month is better described by its speed alone. It also keeps a
 *  vanishing rate from rendering as "5e+304 h" — arithmetic, not an ETA. */
const ETA_MAX_SECS = 30 * 24 * 3600;

/** Human remaining time from size/rate, e.g. "3 min". Empty when there is nothing
 *  honest to say. */
export function fmtEta(tx: UITransfer): string {
  if (!tx.rate || tx.rate <= 0 || !Number.isFinite(tx.rate) || !tx.size) return "";
  const secs = Math.max(0, (tx.size - tx.transferred) / tx.rate);
  if (!Number.isFinite(secs) || secs > ETA_MAX_SECS) return "";
  if (secs < 90) return t("eta.seconds", Math.max(1, Math.round(secs)));
  if (secs < 90 * 60) return t("eta.minutes", Math.round(secs / 60));
  return t("eta.hours", Math.round(secs / 3600));
}

/** The right-hand meta line under the status. */
export function metaLine(tx: UITransfer): string {
  switch (tx.status) {
    case "active": {
      const parts = [`${pct(tx)}%`];
      if (tx.rate && tx.rate > 0) {
        parts.push(fmtRate(tx.rate));
        const eta = fmtEta(tx);
        if (eta) parts.push(eta);
      }
      return parts.join(" · ");
    }
    case "sharing":
      return tx.downloadPeers > 0
        ? t("meta.sharingPeers", tx.downloadPeers)
        : t("meta.sharing");
    case "paused":
      return t("meta.paused");
    case "stalled":
      return tx.reason ? tx.reason : t("meta.stalled");
    case "incoming":
      return t("meta.incoming");
    case "deposited": {
      // "Waiting to be picked up" is true for as long as a week, and says nothing
      // about whether it ever reached them. When the relay has told us, say that
      // instead — in the same words the deposits panel uses for the same fact.
      const offer = {
        pending: "deposit.offerPending",
        arrived: "deposit.offerArrived",
        taken: "deposit.taken",
      }[tx.offerStatus ?? ""] as
        | "deposit.offerPending"
        | "deposit.offerArrived"
        | "deposit.taken"
        | undefined;
      return offer ? t(offer) : t("meta.deposited");
    }
    case "failed":
      return tx.reason || t("meta.failed");
    default:
      return "";
  }
}

/** Whether a client-arrival timestamp falls on the local calendar today. */
export function isToday(ms: number): boolean {
  const d = new Date(ms);
  const now = new Date();
  return (
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate()
  );
}

// ---- deposits (links + sealed) --------------------------------------------

/** `u32::MAX`, the sentinel a link is deposited with to mean "no limit". The relay
 *  may echo it back as its own cap, so both ends of the wire can carry it. */
const UNLIMITED = 4294967295;

/** Human time until a unix-seconds deadline, e.g. "6 days", "3 hours". Empty once
 *  the deadline has passed — the caller says "expired" instead of counting down
 *  into the negatives. */
export function fmtUntil(unixSecs: number, nowMs: number = Date.now()): string {
  const secs = unixSecs - Math.floor(nowMs / 1000);
  if (secs <= 0) return "";
  if (secs < 90) return t("until.seconds", secs);
  if (secs < 90 * 60) return t("until.minutes", Math.round(secs / 60));
  if (secs < 48 * 3600) return t("until.hours", Math.round(secs / 3600));
  return t("until.days", Math.round(secs / 86400));
}

export interface DepositMeta {
  tone: Tone;
  /** The one-word state shown next to the name. */
  text: string;
  /** The line under it: why, or how many downloads. */
  detail: string;
  /** Whether asking the relay to withdraw it could still achieve anything. False
   *  only when we *know* the blob is gone — never merely because we could not ask. */
  revocable: boolean;
}

/** How a deposit stands, told honestly.
 *
 *  Three different facts are in play and they must not be conflated: the local
 *  clock (has the TTL passed?), the relay's answer (is the blob still there, and
 *  how often was it served?), and *silence* — the relay could not be asked. The
 *  last one is the one that matters: an unreachable relay must read as "non lo so",
 *  because showing a downloaded one-shot link as "Attivo" is exactly the lie this
 *  panel exists to stop. */
export function depositMeta(d: DepositDto, nowMs: number = Date.now()): DepositMeta {
  if (d.expired) {
    return {
      tone: "mut",
      text: t("deposit.expired"),
      // `expired` is a deadline comparison against the local clock, not a report
      // from the relay. Saying what we actually know.
      detail: t("deposit.expiredDetail"),
      revocable: false,
    };
  }
  // Before the blob question, because it outranks it. A sealed deposit that has
  // been taken reads as gone from the relay — the fetch burns it — and "gone" is
  // also what a withdrawal looks like, so the row used to say "collected, or
  // already withdrawn" about an event we now know happened. `taken` is the only
  // state reported by the recipient's own ack, so it is the only one that can say
  // this outright.
  if (d.offer_status === "taken") {
    return {
      tone: "ok",
      text: t("deposit.taken"),
      detail: t("deposit.takenDetail"),
      // Nothing left to take back: they have the file. Withdrawing now would only
      // delete a blob that has already served its purpose.
      revocable: false,
    };
  }
  if (d.present === false) {
    return {
      tone: "mut",
      text: t("deposit.gone"),
      detail:
        d.kind === "link" ? t("deposit.goneLink") : t("deposit.goneSealed"),
      revocable: false,
    };
  }
  const until = fmtUntil(d.expires, nowMs);
  // `fmtUntil` returns "" only once the deadline has *passed*, so the fallback
  // must not read as "about to expire".
  const when = until
    ? t("deposit.expiresIn", until)
    : t("deposit.expiredJustNow");
  if (d.present === null) {
    // Reachability is not the same as absence. We keep it revocable: the daemon
    // will find out for real when the user asks.
    return {
      tone: "warn",
      text: t("deposit.unknown"),
      detail: t("deposit.unknownDetail", when),
      revocable: true,
    };
  }
  const parts: string[] = [];
  if (d.downloads !== null) {
    const cap =
      d.max_downloads !== null && d.max_downloads < UNLIMITED
        ? `/${d.max_downloads}`
        : "";
    parts.push(t("deposit.downloads", d.downloads, cap));
  } else {
    // An older relay reports presence but not counts. Say the cap we asked for
    // rather than a number we do not have.
    parts.push(
      d.max_label === "unlimited"
        ? t("deposit.noLimit")
        : t("deposit.max", d.max_label)
    );
  }
  // Still out there: say how far it got with the person it was sent to. Only the
  // two states that mean "not yet" are worth a word here — `taken` returned above,
  // and `gone`/`null` have nothing to add to a row that is still live.
  if (d.offer_status === "pending") parts.push(t("deposit.offerPending"));
  if (d.offer_status === "arrived") parts.push(t("deposit.offerArrived"));
  parts.push(when);
  return {
    tone: "ok",
    text: t("deposit.active"),
    detail: parts.join(" · "),
    revocable: true,
  };
}

export interface Section {
  key: string;
  title: string;
  items: UITransfer[];
}

/** Group one direction's rows into the board's ordered sections, honouring the
 *  live search query. Empty sections are dropped by the caller. */
export function sectionsFor(
  rows: UITransfer[],
  dir: "out" | "in",
  query: string
): Section[] {
  const q = query.trim().toLowerCase();
  const match = (tx: UITransfer) =>
    !q ||
    tx.name.toLowerCase().includes(q) ||
    (tx.peer || "").toLowerCase().includes(q);
  const f = rows.filter((tx) => tx.dir === dir && match(tx));

  // "cancelling" belongs here: the cancel is in flight, the transfer is still
  // the daemon's, and the row must stay on screen. Leaving it out of every section
  // made it vanish the instant the user clicked Cancel — the board quietly
  // disagreeing with the engine, which is the bug this whole file guards against.
  const isActive = (s: UIStatus) =>
    s === "active" ||
    s === "sharing" ||
    s === "paused" ||
    s === "stalled" ||
    s === "cancelling";
  const isTerminal = (s: UIStatus) =>
    s === "completed" ||
    s === "failed" ||
    s === "cancelled" ||
    s === "deposited";

  const pending = f.filter((tx) => tx.status === "incoming");
  const active = f.filter((tx) => isActive(tx.status));
  const today = f.filter((tx) => isTerminal(tx.status) && isToday(tx.firstSeen));
  const earlier = f.filter((tx) => isTerminal(tx.status) && !isToday(tx.firstSeen));

  const secs: Section[] = [];
  if (pending.length)
    secs.push({ key: "p", title: t("section.pending"), items: pending });
  if (active.length)
    secs.push({ key: "a", title: t("section.active"), items: active });
  if (today.length)
    secs.push({ key: "t", title: t("section.today"), items: today });
  if (earlier.length)
    secs.push({ key: "e", title: t("section.earlier"), items: earlier });
  return secs;
}
