// Presentation helpers: byte/size formatting, id shortening, and the colour/label
// metadata maps ported from the mock's `renderVals`.

import type { DepositDto, Method, UIStatus, UITransfer } from "./types";

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
    case "in corso":
      return { tone: "out", text: "In corso" };
    case "completato":
      return { tone: "ok", text: "Completato" };
    case "deposited":
      return { tone: "in", text: "Depositato" };
    case "in attesa":
      return { tone: "warn", text: "In pausa" };
    case "in arrivo":
      return { tone: "warn", text: "Da confermare" };
    case "in stallo":
      return { tone: "mut", text: "In attesa di ripresa" };
    case "fallito":
      return { tone: "bad", text: "Fallito" };
    case "in annullamento":
      return { tone: "mut", text: "Annullamento…" };
    case "annullato":
      return { tone: "mut", text: "Annullato" };
  }
}

export interface MethodMeta {
  glyph: string;
  label: string;
  tone: Tone;
}
const METHODS: Record<Method, MethodMeta> = {
  p2p: { glyph: "⇄", label: "Diretto", tone: "in" },
  cloud: { glyph: "☁", label: "Mailbox", tone: "mut" },
  link: { glyph: "◇", label: "Link", tone: "violet" },
  ticket: { glyph: "⛓", label: "Ticket", tone: "out" },
};
export function methodMeta(m: Method): MethodMeta {
  // `METHODS[m]` alone walks the prototype chain: "toString"/"valueOf" would hit
  // Object.prototype's, which is truthy, so `??` would not fall back and the row
  // would render `undefined`. Only an own key counts.
  return Object.prototype.hasOwnProperty.call(METHODS, m) ? METHODS[m] : METHODS.cloud;
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
  // Own keys only — see `methodMeta` for why an inherited hit is not a match.
  return Object.prototype.hasOwnProperty.call(EXT_TINT, ext)
    ? EXT_TINT[ext]
    : "mut";
}

/** Progress-bar modifier classes: direction, plus the state that overrides it.
 *  A finished bar is green and a failed one red regardless of which way it went —
 *  at that point the outcome is the only thing left worth encoding. */
export function barClass(t: UITransfer): string {
  const dir = t.dir === "out" ? "out" : "in";
  if (t.status === "completato") return `prog ${dir} done`;
  if (t.status === "fallito") return `prog ${dir} bad`;
  if (t.status === "in stallo" || t.status === "in attesa")
    return `prog ${dir} stall`;
  return `prog ${dir}`;
}

export function pct(t: UITransfer): number {
  if (!t.size) return 0;
  return Math.min(100, Math.round((t.transferred / t.size) * 100));
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
export function fmtEta(t: UITransfer): string {
  if (!t.rate || t.rate <= 0 || !Number.isFinite(t.rate) || !t.size) return "";
  const secs = Math.max(0, (t.size - t.transferred) / t.rate);
  if (!Number.isFinite(secs) || secs > ETA_MAX_SECS) return "";
  if (secs < 90) return `${Math.max(1, Math.round(secs))} s`;
  if (secs < 90 * 60) return `${Math.round(secs / 60)} min`;
  return `${Math.round(secs / 3600)} h`;
}

/** The right-hand meta line under the status. */
export function metaLine(t: UITransfer): string {
  switch (t.status) {
    case "in corso": {
      const parts = [`${pct(t)}%`];
      if (t.rate && t.rate > 0) {
        parts.push(fmtRate(t.rate));
        const eta = fmtEta(t);
        if (eta) parts.push(eta);
      }
      return parts.join(" · ");
    }
    case "in attesa":
      return "in pausa";
    case "in stallo":
      return t.reason ? t.reason : "riprende appena possibile";
    case "in arrivo":
      return "tocca per i dettagli";
    case "deposited":
      return "in attesa che il destinatario lo ritiri";
    case "fallito":
      return t.reason || "trasferimento fallito";
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

/** Human time until a unix-seconds deadline, e.g. "6 giorni", "3 ore". Empty once
 *  the deadline has passed — the caller says "scaduto" instead of counting down
 *  into the negatives. */
export function fmtUntil(unixSecs: number, nowMs: number = Date.now()): string {
  const secs = unixSecs - Math.floor(nowMs / 1000);
  if (secs <= 0) return "";
  if (secs < 90) return `${secs} secondi`;
  if (secs < 90 * 60) return `${Math.round(secs / 60)} minuti`;
  if (secs < 48 * 3600) return `${Math.round(secs / 3600)} ore`;
  return `${Math.round(secs / 86400)} giorni`;
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
      text: "Scaduto",
      detail: "il relay l'ha già lasciato andare",
      revocable: false,
    };
  }
  if (d.present === false) {
    return {
      tone: "mut",
      text: "Non più disponibile",
      detail:
        d.kind === "link"
          ? "scaricato fino al limite, oppure già revocato"
          : "ritirato dal destinatario, oppure già revocato",
      revocable: false,
    };
  }
  const until = fmtUntil(d.expires, nowMs);
  const scade = until ? `scade tra ${until}` : "in scadenza";
  if (d.present === null) {
    // Reachability is not the same as absence. We keep it revocable: the daemon
    // will find out for real when the user asks.
    return {
      tone: "warn",
      text: "Stato sconosciuto",
      detail: `relay non raggiungibile · ${scade}`,
      revocable: true,
    };
  }
  const parts: string[] = [];
  if (d.downloads !== null) {
    const cap =
      d.max_downloads !== null && d.max_downloads < UNLIMITED
        ? `/${d.max_downloads}`
        : "";
    parts.push(`${d.downloads}${cap} download`);
  } else {
    // An older relay reports presence but not counts. Say the cap we asked for
    // rather than a number we do not have.
    parts.push(d.max_label === "unlimited" ? "nessun limite" : `max ${d.max_label}`);
  }
  parts.push(scade);
  return { tone: "ok", text: "Attivo", detail: parts.join(" · "), revocable: true };
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
  const match = (t: UITransfer) =>
    !q ||
    t.name.toLowerCase().includes(q) ||
    (t.peer || "").toLowerCase().includes(q);
  const f = rows.filter((t) => t.dir === dir && match(t));

  // "in annullamento" belongs here: the cancel is in flight, the transfer is still
  // the daemon's, and the row must stay on screen. Leaving it out of every section
  // made it vanish the instant the user clicked Annulla — the board quietly
  // disagreeing with the engine, which is the bug this whole file guards against.
  const isActive = (s: UIStatus) =>
    s === "in corso" ||
    s === "in attesa" ||
    s === "in stallo" ||
    s === "in annullamento";
  const isTerminal = (s: UIStatus) =>
    s === "completato" ||
    s === "fallito" ||
    s === "annullato" ||
    s === "deposited";

  const pending = f.filter((t) => t.status === "in arrivo");
  const active = f.filter((t) => isActive(t.status));
  const today = f.filter((t) => isTerminal(t.status) && isToday(t.firstSeen));
  const earlier = f.filter((t) => isTerminal(t.status) && !isToday(t.firstSeen));

  const secs: Section[] = [];
  if (pending.length)
    secs.push({ key: "p", title: "Da confermare", items: pending });
  if (active.length)
    secs.push({ key: "a", title: "In corso e in attesa", items: active });
  if (today.length) secs.push({ key: "t", title: "Oggi", items: today });
  if (earlier.length)
    secs.push({ key: "e", title: "Precedenti", items: earlier });
  return secs;
}
