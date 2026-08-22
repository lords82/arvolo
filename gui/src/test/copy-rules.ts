// What `docs/COPY.md` says, in a form a test can check.
//
// The rule the caps encode: a user-facing string carries the outcome and, when
// there is a choice to make, the action. The reason why goes to a second level —
// a hint line on screen, `-v` in the CLI — where somebody who wants it can find
// it and everybody else is not made to re-read it. So the caps are not a style
// preference about brevity: they are where the first level ends.
//
// Categories come from the key name because the key names already encode the
// slot. `*Hint`, `*Desc`, `*Body` are the second level and get room; `*.submit*`
// and `*.menu*` are things you click and get almost none. Where a name does not
// say, `CATEGORY` names the key outright.

export type Cat =
  | "label"
  | "button"
  | "title"
  | "status"
  | "error"
  | "secondary"
  | "confirmBody";

/** Caps in characters, measured against English. */
export const CAP: Record<Cat, number> = {
  label: 24,
  button: 28,
  title: 40,
  status: 48,
  error: 60,
  secondary: 120,
  confirmBody: 140,
};

/** The cap is written for English; the others are not English and must not be
 *  squeezed into its word lengths. German compounds are the binding case — a
 *  test that ignored this would either truncate German or never pass. */
export const LANG_SLACK: Record<string, number> = {
  en: 1.0,
  it: 1.15,
  fr: 1.2,
  de: 1.35,
};

/** Keys whose category the name does not give away.
 *
 *  The fallback below is `title`, which is the strict end — so anything landing
 *  here is a string that reads as an explanation but is not named like one. Add
 *  to this map rather than loosening the fallback: a wrong default that is
 *  generous hides real cases, a wrong default that is strict shows up at once. */
const CATEGORY: Record<string, Cat> = {
  // Body text under a heading, or a whole explanatory block on its own.
  "send.pickerEmpty": "secondary",
  "send.noRelay": "secondary",
  "send.linkTooMany": "secondary",
  "settings.relayLocked": "secondary",
  "settings.downloadDirEnv": "secondary",
  // A toast's second line. `common.*` is otherwise all micro-labels, which is
  // why the prefix alone reads it as one.
  "common.afterRestart": "secondary",
};

/** The other end of the cap, for the handful of strings where being brief is the
 *  failure. Each of these is the last thing a user reads before doing something
 *  they cannot undo — handing over their identity, downloading from a key nobody
 *  checked. "Careful." satisfies every cap in this file and tells them nothing,
 *  so these carry a minimum as well as a maximum, and what the minimum has to
 *  contain is written next to each one.
 *
 *  Floors are measured in English only: a translation that came out shorter is a
 *  translation question, not a licence to weaken the original. */
export const FLOOR: Record<string, number> = {
  // Must say that whoever types the code *becomes* you — not "gets access".
  "pair.deviceWarnRest": 90,
  "devices.pairWarnRest": 90,
  "pair.joinWarnRest": 90,
  // Must say the fingerprint was never compared, and what that lets through.
  "people.confirmForceBody": 90,
  "incoming.hintUnknown": 70,
  // Must say the key *is* the identity, not merely that it is private.
  "settings.identityKeyHint": 70,
};

/** Entries nobody reads on screen, so no cap applies. The `*Kw` ones are extra
 *  words the command palette matches against — deliberately a pile of synonyms,
 *  and longer is better there. `locale.tag` is a BCP 47 tag for `Intl`. */
export function isExempt(key: string): boolean {
  return (
    /Kw$/.test(key) ||
    key === "locale.tag" ||
    key === "locale.name" ||
    key === "people.exportFilename"
  );
}

/** A number and a string no translation contains by accident, so what a function
 *  entry splices in can be subtracted back out of its length. Same trick as
 *  `i18n.test.ts`, which needs both because the build erases the parameter types
 *  and a number-shaped slot turns a string into NaN. */
const MARK_N = 424242;
const MARK_S = "«ZQX»";

/** The entry as it reaches the screen, with any interpolated value removed —
 *  a caller's file name is not part of what the copy costs. */
export function render(entry: unknown): string {
  if (typeof entry === "string") return entry;
  if (typeof entry !== "function") return "";
  const fn = entry as (...a: unknown[]) => string;
  for (const mark of [MARK_S, MARK_N] as const) {
    const args = Array.from({ length: fn.length }, (_, i) =>
      typeof mark === "number" ? mark + i : `${mark}${i}`
    );
    let out: string;
    try {
      out = fn(...args);
    } catch {
      continue;
    }
    for (const a of args) out = out.split(String(a)).join("");
    return out;
  }
  return "";
}

export function categorise(key: string): Cat {
  if (CATEGORY[key]) return CATEGORY[key];
  if (/confirm.*Body$/i.test(key)) return "confirmBody";
  if (/(Hint|Desc|Body|Note|Detail|Blurb|Warn[A-Z]|explain)/i.test(key))
    return "secondary";
  if (/^(blurb|sub)/i.test(key.split(".")[1] ?? "")) return "secondary";
  if (/^store\.err/.test(key)) return "error";
  if (/^(meta|status|state|eta|until|ago|section|deposit)\./.test(key))
    return "status";
  if (/(Title|title)$/.test(key) || /\.title/.test(key)) return "title";
  if (/^(common|trust|title)\./.test(key) || /Label$/.test(key)) return "label";
  if (/(submit|menu|action)/i.test(key) || /^(palette|rail)\./.test(key))
    return "button";
  return "title";
}
