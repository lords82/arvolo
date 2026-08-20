// Translation, without a library.
//
// Two reasons this is 80 lines rather than a dependency. The bundle runs behind
// the Tauri webview's CSP with nothing fetched at runtime, so a loader that
// splits dictionaries per language buys nothing — all four ship anyway. And the
// interesting property here is not lookup, it is that the four dictionaries
// cannot drift: `Dict` is derived from the English one, so a missing key or an
// interpolation with the wrong arguments is a type error at build time.
//
// The language lives here rather than in `store.ts` because the store itself
// needs `t()` for the failures it reports, and a store that imported the module
// that imported the store would be a cycle. Components subscribe through
// `useT()`; everything else calls `t()` directly and reads whatever is current.

import { useSyncExternalStore } from "react";
import { en, type Dict } from "./en";
import { it } from "./it";
import { fr } from "./fr";
import { de } from "./de";

export const LANGS = ["en", "it", "fr", "de"] as const;
export type Lang = (typeof LANGS)[number];

/** What the user picked. `"system"` is not a language — it is the instruction to
 *  keep following the OS, which can differ between launches. Storing the choice
 *  rather than the resolved language is what makes that instruction survive. */
export type LangChoice = "system" | Lang;

const DICTS: Record<Lang, Dict> = { en, it, fr, de };

const LANG_KEY = "arvolo.lang";

function isLang(v: unknown): v is Lang {
  return typeof v === "string" && (LANGS as readonly string[]).includes(v);
}

/** The OS language, when Arvolo speaks it. `navigator.languages` is in the
 *  user's own order of preference, so a machine set to French-then-Italian gets
 *  French; only a machine that prefers none of the four falls back to English. */
export function systemLang(): Lang {
  const tags =
    typeof navigator === "undefined"
      ? []
      : (navigator.languages ?? [navigator.language]).filter(Boolean);
  for (const tag of tags) {
    const base = tag.toLowerCase().split("-")[0];
    if (isLang(base)) return base;
  }
  return "en";
}

function readChoice(): LangChoice {
  try {
    const v = localStorage.getItem(LANG_KEY);
    if (v === "system" || isLang(v)) return v;
  } catch {
    // Private-mode webviews can throw on localStorage. A language is not worth
    // failing to boot over — see `readTheme` in the store for the same call.
  }
  return "system";
}

let choice: LangChoice = readChoice();
let lang: Lang = choice === "system" ? systemLang() : choice;

const listeners = new Set<() => void>();

/** Look a string up in a given dictionary.
 *
 *  The rest parameter is typed from the entry itself, so `t("app.send", "x")`
 *  and a forgotten argument are both compile errors. */
export type TFn = <K extends keyof Dict>(
  key: K,
  ...args: Dict[K] extends (...a: infer A) => string ? A : []
) => string;

function bind(d: Dict): TFn {
  return (key, ...args) => {
    const entry = d[key];
    return typeof entry === "function"
      ? (entry as (...a: unknown[]) => string)(...args)
      : (entry as string);
  };
}

/** One `t` per language, made once.
 *
 *  A single `t` that read a mutable `dict` would be simpler, and wrong in one
 *  specific way: its identity would never change, so it could not be a `useMemo`
 *  dependency, and a memo holding translated text would keep the old language
 *  until something else invalidated it. Binding per language gives an identity
 *  that is stable across renders and changes exactly when the words do. */
const BOUND: Record<Lang, TFn> = {
  en: bind(en),
  it: bind(it),
  fr: bind(fr),
  de: bind(de),
};

/** The current language's lookup, for code that is not a component. Components
 *  should use `useT()`, or they will not repaint when the language changes. */
export const t: TFn = (key, ...args) => BOUND[lang](key, ...args);

export function getLang(): Lang {
  return lang;
}

export function getLangChoice(): LangChoice {
  return choice;
}

/** BCP 47 tag for `Intl` — weekday and month names, and the clock convention. */
export function locale(): string {
  return t("locale.tag");
}

export function setLangChoice(next: LangChoice): void {
  choice = next;
  lang = next === "system" ? systemLang() : next;
  // The native side (tray, notifications) follows along. Deliberately a dynamic
  // import: this module is loaded by tests and by the pure-web preview, where no
  // Tauri backend exists to invoke.
  import("../ipc")
    .then((m) => m.api.setUiLanguage(lang))
    .catch(() => {});
  // `<html lang>` is not decoration: it is what a screen reader picks its voice
  // from, and what the webview hyphenates and spell-checks against.
  if (typeof document !== "undefined") {
    document.documentElement.lang = lang;
  }
  try {
    localStorage.setItem(LANG_KEY, next);
  } catch {
    // See `readChoice`: the language still applies, it just will not be
    // remembered next launch.
  }
  for (const fn of listeners) fn();
}

/** Apply the stored choice on boot — mainly to stamp `<html lang>`, since the
 *  dictionary is already resolved at module load. */
export function initLang(): void {
  setLangChoice(choice);
}

function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

/** `t`, for components: the caller repaints when the language changes, and the
 *  function it gets back is safe to list as a `useMemo`/`useEffect` dependency —
 *  stable within a language, different across one. */
export function useT(): TFn {
  return BOUND[useSyncExternalStore(subscribe, getLang, getLang)];
}

/** The active language, for the few places that need the tag itself rather than
 *  a string: `Intl` formatters, mostly. */
export function useLang(): Lang {
  return useSyncExternalStore(subscribe, getLang, getLang);
}

/** The current choice, for the settings picker. */
export function useLangChoice(): LangChoice {
  return useSyncExternalStore(subscribe, getLangChoice, getLangChoice);
}

/** Every language's own name, for the picker. Endonyms: a French speaker
 *  looking for their language is looking for "Français", not "French". */
export function langNames(): { value: Lang; label: string }[] {
  return LANGS.map((l) => ({ value: l, label: DICTS[l]["locale.name"] }));
}

export type { Dict };
