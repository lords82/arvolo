// The caps from `docs/COPY.md`, enforced.
//
// This test exists because prose grows back. The copy it guards was rewritten
// once already; without something that fails, the next long explanation appended
// to a button label lands unnoticed and the whole thing drifts back over a year.
//
// `DEBT` is the ratchet. It lists what was over the cap when the gate went in,
// so the suite is green from the first commit and the rewrite can proceed area by
// area. It fails in *both* directions: a key over its recorded debt, and a key
// that has come under its cap but is still listed. The second half is what stops
// the list from becoming permanent — you cannot fix a string and leave its
// exemption behind for someone to hide behind later.
//
// When `DEBT` is empty, delete it and this paragraph with it.

import { describe, expect, it } from "vitest";
import { en } from "../i18n/en";
import { it as itDict } from "../i18n/it";
import { fr } from "../i18n/fr";
import { de } from "../i18n/de";
import { CAP, FLOOR, LANG_SLACK, categorise, isExempt, render } from "./copy-rules";

type Lang = "en" | "it" | "fr" | "de";
const LANGS: Lang[] = ["en", "it", "fr", "de"];

const DICTS: Record<Lang, Record<string, unknown>> = {
  en: en as Record<string, unknown>,
  it: itDict as Record<string, unknown>,
  fr: fr as Record<string, unknown>,
  de: de as Record<string, unknown>,
};

const KEYS = Object.keys(en).filter((k) => !isExempt(k));

/** Length in English-equivalent characters: a German string is allowed to be
 *  longer, so it is divided back down before being compared to one cap. */
function normalised(key: string, lang: Lang): number {
  return Math.ceil(render(DICTS[lang][key]).length / LANG_SLACK[lang]);
}

/** The worst any of the four languages does on this key. */
function worst(key: string): number {
  return Math.max(...LANGS.map((l) => normalised(key, l)));
}

/** What was over the cap when this gate was written. Shrinks to nothing. */
const DEBT: Record<string, number> = {
  "transfers.firstRunBody": 176,
  "deposits.intro": 154,
  "deposits.confirmRevokeLink": 139,
  "deposits.confirmRevokeSealed": 139,
  "deposits.ticketDetail": 134,
  "deposits.emptyBody": 124,
  "app.disconnected": 93,
  "meta.preparing": 82,
  "app.versionMismatch": 81,
  "share.fromDownload": 77,
  "transfers.saveSenderSub": 61,
  "transfers.reorder": 53,
  "store.loadTransfers": 47,
  "store.loadHistory": 45,
  "store.loadConfig": 45,
  "store.loadDeposits": 43,
  "bits.qrTooDense": 42,
  "store.loadSync": 41,
  "palette.sync": 37,
  "palette.pauseAll": 32,
  "palette.placeholder": 32,
  "palette.pairContact": 30,
  "palette.clearFinished": 29,
};

/** The app-voice sentences still to be rewritten. Ordered as `Object.keys(en)`
 *  gives them, which is the order they appear in the dictionary — so a whole
 *  area empties at once as it is worked through. Delete entries, never add. */
const VOICE_DEBT: string[] = [
  "app.disconnected",
  "transfers.openFileFailed",
  "transfers.openFolderFailed",
  "deposits.openFailed",
  "store.loadTransfers",
  "store.loadHistory",
  "store.loadDeposits",
  "store.loadConfig",
  "store.loadSync",
];

describe("copy stays inside its slot", () => {
  it.each(LANGS)("%s keeps every entry inside its cap", (lang) => {
    const over = KEYS.filter((k) => {
      const cat = categorise(k);
      const allowed = Math.max(CAP[cat], DEBT[k] ?? 0);
      return normalised(k, lang) > allowed;
    }).map((k) => `${k} (${categorise(k)}, ${normalised(k, lang)} > ${Math.max(CAP[categorise(k)], DEBT[k] ?? 0)})`);
    expect(over).toEqual([]);
  });

  it("lists nothing in DEBT that has already been fixed", () => {
    // Without this the allowlist outlives the debt, and a key that was shortened
    // once can quietly grow back up to its old exemption.
    const stale = Object.keys(DEBT).filter((k) => {
      if (!(k in en)) return true;
      return worst(k) <= CAP[categorise(k)];
    });
    expect(stale).toEqual([]);
  });

  it("keeps a warning long enough to still be a warning", () => {
    // The one place a cap can do harm: shortening these is how "whoever types
    // this code becomes you" turns into "Careful.". The floor is the promise
    // that the rewrite cannot make a risk sound smaller than it is.
    const gutted = Object.keys(FLOOR).filter((k) => normalised(k, "en") < FLOOR[k]);
    expect(gutted).toEqual([]);
  });
});

describe("the app speaks in no voice of its own", () => {
  /** "I can't reach the daemon", "I'll try it anyway — the daemon is fussier
   *  than I am": a program narrating its own plumbing in the first person. The
   *  sentence is better without it every time, and shorter too, because what
   *  follows the "I" is usually the explanation rather than the outcome.
   *
   *  Bare "me" is deliberately not matched: `arvolo me` is a command, and the
   *  hints that name it are not the app talking about itself. */
  const FIRST_PERSON = /\b(I|I['’](m|ll|ve|d)|my)\b/;

  /** Where the first person is right, because the app is not the one speaking.
   *
   *  A button and a checkbox are worded as the user's own words — they are what
   *  the user is saying by clicking, so "I'll verify first" is the user, not the
   *  app. (Several of these are still due to become plain verbs, but that is the
   *  button rule doing it, not this one.) */
  const USER_VOICE = new Set([
    "person.compared", // the claim the user signs by ticking the box
    "people.confirmForceCancel", // "I'll verify first" — a choice, in their words
    "pair.understood", // "I understand: this device loses its identity."
    "pair.captionDevice", // quotes the button above, which is the user's words
    "people.menuUntrust", // "ask me every time" — the user setting their own terms
  ]);

  it("uses the first person only where the user is the one speaking", () => {
    const talking = KEYS.filter(
      (k) => !USER_VOICE.has(k) && FIRST_PERSON.test(render(en[k as keyof typeof en]))
    );
    // Listed rather than counted: each one is a sentence to rewrite, and the
    // list is the work queue. Empty it, do not grow it.
    expect(talking).toEqual(VOICE_DEBT);
  });
});
