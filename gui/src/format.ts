// Presentation helpers: byte/size formatting, id shortening, and the colour/label
// metadata maps ported from the mock's `renderVals`.

import type { Method, UIStatus, UITransfer } from "./types";

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
  return (p.length > 1 ? p.pop()! : "FILE").toUpperCase().slice(0, 4);
}

/** Shorten a base32 id to `if2xmne…c7daha`, matching the CLI/mock. */
export function shortId(id: string): string {
  if (id.length <= 16) return id;
  return `${id.slice(0, 7)}…${id.slice(-6)}`;
}

export interface StatusMeta {
  color: string;
  text: string;
}
export function statusMeta(s: UIStatus): StatusMeta {
  switch (s) {
    case "in corso":
      return { color: "#ea580c", text: "In corso" };
    case "completato":
      return { color: "#16a34a", text: "Completato" };
    case "deposited":
      return { color: "#0369a1", text: "Depositato" };
    case "in attesa":
      return { color: "#a8834a", text: "In attesa" };
    case "in arrivo":
      return { color: "#b45309", text: "Da confermare" };
    case "in stallo":
      return { color: "#6b7280", text: "In attesa di ripresa" };
    case "fallito":
      return { color: "#dc2626", text: "Fallito" };
    case "in annullamento":
      return { color: "#8a827a", text: "Annullamento…" };
    case "annullato":
      return { color: "#8a827a", text: "Annullato" };
  }
}

export interface MethodMeta {
  glyph: string;
  label: string;
  color: string;
  bg: string;
}
const METHODS: Record<Method, MethodMeta> = {
  p2p: { glyph: "⇄", label: "P2P", color: "#0369a1", bg: "#e9f3fb" },
  cloud: { glyph: "☁", label: "Cloud", color: "#57534c", bg: "#f0ece7" },
  link: { glyph: "◇", label: "Link", color: "#7c3aed", bg: "#f3edff" },
  ticket: { glyph: "⛓", label: "Ticket", color: "#c2410c", bg: "#fff3e9" },
};
export function methodMeta(m: Method): MethodMeta {
  return METHODS[m] ?? METHODS.cloud;
}

const EXT_TINT: Record<string, [string, string]> = {
  ZIP: ["#fff3e9", "#c2410c"],
  MOV: ["#eef7f0", "#16a34a"],
  MP4: ["#eef7f0", "#16a34a"],
  MKV: ["#e9f3fb", "#0369a1"],
  PDF: ["#fdecec", "#dc2626"],
  KEY: ["#f3edff", "#7c3aed"],
  WAV: ["#e9f3fb", "#0369a1"],
  JPG: ["#fff7ed", "#c2410c"],
  PNG: ["#fff7ed", "#c2410c"],
  TAR: ["#f4f1ee", "#8a827a"],
};
export function extTint(ext: string): [string, string] {
  return EXT_TINT[ext] ?? ["#f4f1ee", "#8a827a"];
}

/** Colour of the progress bar for a transfer (attenuated when stalled). */
export function barColor(t: UITransfer): string {
  const stall = t.status === "in stallo";
  if (t.dir === "out") return stall ? "#e8c4a8" : "#f97316";
  return stall ? "#a9c6dd" : "#0369a1";
}

export function pct(t: UITransfer): number {
  if (!t.size) return 0;
  return Math.min(100, Math.round((t.transferred / t.size) * 100));
}

/** Human throughput, e.g. "42 MB/s". */
export function fmtRate(bytesPerSec: number): string {
  return fmtBytes(bytesPerSec) + "/s";
}

/** Human remaining time from size/rate, e.g. "3 min". */
export function fmtEta(t: UITransfer): string {
  if (!t.rate || t.rate <= 0 || !t.size) return "";
  const secs = Math.max(0, (t.size - t.transferred) / t.rate);
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

  const isActive = (s: UIStatus) =>
    s === "in corso" || s === "in attesa" || s === "in stallo";
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
