// Theme and language: two rows, and a wheel for each.
//
// They were two lists with a tick down the side. The design of record (7s/7t)
// raises this platform's own picker instead, and the argument is the gesture:
// a list of five languages is five targets to aim at, a wheel is one movement
// of the thumb and a word to confirm with. It also puts the answer *on the row*,
// which a list of ticks can only do by being read.
//
// The theme changes as the wheel turns — the canvas is explicit that you see it
// behind the sheet before pressing Fatto, and Annulla puts back what was there.
// The language waits for Fatto: changing it live would rewrite "Annulla" and
// "Fatto" under the thumb that is still turning the wheel.

import { useState } from "react";
import { langNames, setLangChoice, useLangChoice, useT } from "../i18n";
import type { LangChoice } from "../i18n";
import { useStore, type ThemeChoice } from "../store";
import { ValueRow } from "../ui/Rows";
import { WheelSheet, type WheelOption } from "../ui/Wheel";

const THEMES = [
  { value: "system", label: "settings.themeSystem" },
  { value: "light", label: "settings.themeLight" },
  { value: "dark", label: "settings.themeDark" },
] as const;

export function AppearanceView() {
  const t = useT();
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const lang = useLangChoice();
  const [turning, setTurning] = useState<"theme" | "lang" | null>(null);

  const themeOptions: WheelOption<ThemeChoice>[] = THEMES.map((c) => ({
    value: c.value as ThemeChoice,
    label: t(c.label),
  }));
  // The language names are endonyms and stay put whatever the current language
  // is: somebody hunting for German is looking for "Deutsch". Only "System" is
  // translated, because it is a sentence about the machine rather than the name
  // of a language.
  const langOptions: WheelOption<LangChoice>[] = [
    { value: "system" as LangChoice, label: t("settings.languageAuto") },
    ...langNames().map((l) => ({ value: l.value as LangChoice, label: l.label })),
  ];
  const nameOf = <T extends string>(opts: WheelOption<T>[], v: T) =>
    opts.find((o) => o.value === v)?.label ?? "";

  return (
    <div className="ios-page">
      <div className="ios-group">
        <ValueRow
          label={t("settings.theme")}
          value={nameOf(themeOptions, theme)}
          onClick={() => setTurning("theme")}
        />
        <ValueRow
          label={t("settings.language")}
          value={nameOf(langOptions, lang as LangChoice)}
          onClick={() => setTurning("lang")}
        />
      </div>

      <WheelSheet
        open={turning === "theme"}
        title={t("settings.theme")}
        hint={t("settings.themeHintPhone")}
        options={themeOptions}
        value={theme}
        onPreview={(v) => setTheme(v)}
        onCancel={() => setTurning(null)}
        onDone={(v) => {
          setTheme(v);
          setTurning(null);
        }}
      />

      <WheelSheet
        open={turning === "lang"}
        title={t("settings.language")}
        hint={t("settings.languageHintPhone")}
        options={langOptions}
        value={lang as LangChoice}
        onCancel={() => setTurning(null)}
        onDone={(v) => {
          setLangChoice(v);
          setTurning(null);
        }}
      />
    </div>
  );
}
