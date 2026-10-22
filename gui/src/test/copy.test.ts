// The caps from `docs/COPY.md`, enforced.
//
// This test exists because prose grows back. The copy it guards was rewritten
// once already; without something that fails, the next long explanation appended
// to a button label lands unnoticed and the whole thing drifts back over a year.
//
// It went in with a ratchet: a `DEBT` map of everything over the cap on the day,
// failing in both directions so an entry could not be fixed and left listed. The
// map is empty now and gone with it. There is no allowlist any more, which is
// the point — a new string meets the cap or the build stops.

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

describe("copy stays inside its slot", () => {
  it.each(LANGS)("%s keeps every entry inside its cap", (lang) => {
    const over = KEYS.filter((k) => {
      const cat = categorise(k);
      return normalised(k, lang) > CAP[cat];
    }).map(
      (k) =>
        `${k} (${categorise(k)}, ${normalised(k, lang)} > ${CAP[categorise(k)]})`
    );
    expect(over).toEqual([]);
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
    // "Show my code": the code is the user's, and the button is what they say by
    // pressing it — "mostra il mio codice" in the design of record.
    "exchange.showCode",
    // "One of my devices": an answer to "who are you sending it to?", in the
    // words of the person answering.
    "send.toMine",
    "send.toHere",
  ]);

  it("uses the first person only where the user is the one speaking", () => {
    const talking = KEYS.filter(
      (k) => !USER_VOICE.has(k) && FIRST_PERSON.test(render(en[k as keyof typeof en]))
    );
    expect(talking).toEqual([]);
  });
});
