// What the type system cannot say about four dictionaries.
//
// `Dict` is derived from the English one, so a missing key or a wrong argument
// list already fails the build (see i18n/index.ts). Everything below is the
// remainder: properties that type-check perfectly and are still wrong on screen.
// A translator who drops the interpolation writes a valid string; a key left
// empty is a valid string too. Both render as silence, and silence is the one
// bug nobody reports.

import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { en } from "../i18n/en";
import { it as itDict } from "../i18n/it";
import { fr } from "../i18n/fr";
import { de } from "../i18n/de";
import {
  LANGS,
  getLang,
  getLangChoice,
  langNames,
  setLangChoice,
  systemLang,
  t,
  type Lang,
} from "../i18n";

const DICTS: Record<Lang, Record<string, unknown>> = {
  en: en as Record<string, unknown>,
  it: itDict as Record<string, unknown>,
  fr: fr as Record<string, unknown>,
  de: de as Record<string, unknown>,
};

const OTHERS: Lang[] = ["it", "fr", "de"];
const enKeys = Object.keys(en).sort();

/** A number no translation would contain by accident, so finding it in the
 *  output proves the argument reached the string rather than a literal did. */
const MARK_N = 424242;
const MARK_S = "«ZQX»";

/** Whether `fn` puts every argument it was given into what it returns.
 *
 *  Which marker to use cannot be read off the function: the build erases the
 *  type annotations, so `(n: number) => …` is plain `(n) => …` by the time it
 *  runs. So try both. A number survives arithmetic (`Math.round(n / 60)`) where
 *  a string turns into NaN and vanishes; a string survives slots that only ever
 *  get pasted in. An entry passes if either marker comes back out — it fails
 *  only when the slot is genuinely dropped. */
function keepsItsArguments(fn: (...a: unknown[]) => string): {
  ok: boolean;
  sample: string;
} {
  let sample = "";
  for (const mark of [MARK_N, MARK_S] as const) {
    // Distinct per position: filling both slots with the same value would let
    // `${a} of ?` pass, since the one marker that did come out satisfies both.
    const args = Array.from({ length: fn.length }, (_, i) =>
      typeof mark === "number" ? mark + i : `${mark}${i}`
    );
    let out: string;
    try {
      out = fn(...args);
    } catch {
      continue; // wrong shape for this marker; the other one may fit
    }
    sample = out;
    if (args.every((a) => out.includes(String(a)))) return { ok: true, sample: out };
  }
  return { ok: false, sample };
}

describe("the four dictionaries stay in step", () => {
  it.each(OTHERS)("%s has exactly English's keys — no more, no fewer", (lang) => {
    // The type checker catches a *missing* key. An extra one it cannot see at
    // all: a key nothing reads is dead weight that reads as translated work.
    expect(Object.keys(DICTS[lang]).sort()).toEqual(enKeys);
  });

  it.each(LANGS)("%s has a string or a function under every key", (lang) => {
    const wrong = enKeys.filter((k) => {
      const v = DICTS[lang][k];
      return typeof v !== "string" && typeof v !== "function";
    });
    expect(wrong).toEqual([]);
  });

  it.each(LANGS)("%s leaves no entry blank", (lang) => {
    const blank = enKeys.filter((k) => {
      const v = DICTS[lang][k];
      return typeof v === "string" && v.trim() === "";
    });
    // An empty string is a label that renders as nothing — a button with no
    // words on it, and no error anywhere to say why.
    expect(blank).toEqual([]);
  });

  it.each(OTHERS)("%s keeps as a function every key English interpolates", (lang) => {
    const fnKeys = enKeys.filter((k) => typeof en[k as keyof typeof en] === "function");
    const notFn = fnKeys.filter((k) => typeof DICTS[lang][k] !== "function");
    expect(notFn).toEqual([]);
  });

  it.each(OTHERS)("%s takes the same number of arguments as English", (lang) => {
    const mismatched = enKeys
      .filter((k) => typeof en[k as keyof typeof en] === "function")
      .filter(
        (k) =>
          (DICTS[lang][k] as (...a: never[]) => string).length !==
          (en[k as keyof typeof en] as unknown as (...a: never[]) => string).length
      );
    expect(mismatched).toEqual([]);
  });
});

describe("every interpolated value actually reaches the screen", () => {
  it.each(LANGS)("%s drops no argument on the floor", (lang) => {
    const lost: string[] = [];
    let checked = 0;
    for (const key of enKeys) {
      if (typeof en[key as keyof typeof en] !== "function") continue;
      const fn = DICTS[lang][key] as (...a: unknown[]) => string;
      if (fn.length === 0) continue;
      checked++;
      // A translation that forgets its slot still compiles and still reads as a
      // sentence — "files waiting" instead of "12 files waiting".
      const { ok, sample } = keepsItsArguments(fn);
      if (!ok) lost.push(`${key} (${lang}) → ${sample}`);
    }
    expect(lost).toEqual([]);
    // Guard for the guard: a probe that quietly examined nothing would leave
    // `lost` empty and look exactly like a pass.
    expect(checked).toBeGreaterThan(20);
  });

  it("the probe notices a dropped argument", () => {
    expect(keepsItsArguments((n) => `${n} files`).ok).toBe(true);
    expect(keepsItsArguments((_n) => "files waiting for you").ok).toBe(false);
    // Two slots, one of them forgotten, is the likeliest translation slip.
    expect(keepsItsArguments((a, _b) => `${a} of ?`).ok).toBe(false);
    // Arithmetic on the value is still keeping it — this is why the number
    // marker is tried before the string one.
    expect(keepsItsArguments((n) => `${Number(n) * 1} s`).ok).toBe(true);
  });
});

describe("each language can name and format itself", () => {
  it.each(LANGS)("%s carries a tag Intl accepts", (lang) => {
    const tag = DICTS[lang]["locale.tag"] as string;
    expect(tag).toBe(lang);
    // `locale()` feeds Intl for weekday and month names; a tag Intl rejects
    // throws at the first date the user looks at, not here.
    expect(() => new Intl.DateTimeFormat(tag)).not.toThrow();
    expect(new Intl.DateTimeFormat(tag).resolvedOptions().locale).toContain(lang);
  });

  it("names itself in its own words, and no two alike", () => {
    const names = LANGS.map((l) => DICTS[l]["locale.name"] as string);
    expect(names.every((n) => n.trim().length > 0)).toBe(true);
    expect(new Set(names).size).toBe(LANGS.length);
    // Endonyms: someone hunting for their language looks for "Français".
    expect(names).toContain("Français");
    expect(names).toContain("Deutsch");
  });

  it("the picker offers every language, labelled by its endonym", () => {
    expect(langNames().map((o) => o.value)).toEqual([...LANGS]);
    for (const { value, label } of langNames()) {
      expect(label).toBe(DICTS[value]["locale.name"]);
    }
  });
});

describe("choosing a language", () => {
  const realLanguages = Object.getOwnPropertyDescriptor(
    window.navigator,
    "languages"
  );
  const setNavigator = (langs: string[]) =>
    Object.defineProperty(window.navigator, "languages", {
      value: langs,
      configurable: true,
    });

  beforeEach(() => {
    localStorage.clear();
    setLangChoice("en");
  });

  afterEach(() => {
    if (realLanguages) {
      Object.defineProperty(window.navigator, "languages", realLanguages);
    }
    setLangChoice("en");
  });

  it("changes the words t() hands back", () => {
    setLangChoice("it");
    const italian = t("app.send");
    setLangChoice("fr");
    expect(t("app.send")).not.toBe(italian);
    expect(t("app.send")).toBe(fr["app.send"]);
  });

  it("stamps <html lang>, which is what a screen reader reads it in", () => {
    setLangChoice("de");
    expect(document.documentElement.lang).toBe("de");
  });

  it("remembers the choice, not the language it resolved to", () => {
    setLangChoice("system");
    // Storing "it" here would freeze today's OS setting into the app for good;
    // "system" is an instruction, and it has to survive as one.
    expect(localStorage.getItem("arvolo.lang")).toBe("system");
    expect(getLangChoice()).toBe("system");
  });

  it("follows the OS in the user's own order of preference", () => {
    setNavigator(["fr-CA", "it-IT"]);
    expect(systemLang()).toBe("fr");
    setNavigator(["it-IT", "fr-CA"]);
    expect(systemLang()).toBe("it");
  });

  it("falls back to English only when the OS prefers none of the four", () => {
    setNavigator(["ja-JP", "ko-KR"]);
    expect(systemLang()).toBe("en");
    // Not a fallback: a regional variant is still the language.
    setNavigator(["de-AT"]);
    expect(systemLang()).toBe("de");
  });

  it("survives a webview that refuses localStorage", () => {
    const real = Storage.prototype.setItem;
    Storage.prototype.setItem = () => {
      throw new Error("denied");
    };
    try {
      // Private-mode webviews throw on access. Losing the preference for next
      // launch is a nuisance; failing to switch language is a broken app.
      expect(() => setLangChoice("it")).not.toThrow();
      expect(getLang()).toBe("it");
    } finally {
      Storage.prototype.setItem = real;
    }
  });
});
