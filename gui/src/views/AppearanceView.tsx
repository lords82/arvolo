// Theme and language, as two lists.
//
// Both are exclusive choices out of a handful, which on a phone is a checkmark
// down a list and not a strip of segments: five languages in a segmented control
// is five labels at four characters each, and the moment a sixth is added the
// whole strip has to be redesigned. A list just gets longer.
//
// They share one screen rather than one each, because neither is worth a tap of
// its own from the settings list — "Appearance" is one thing to go and change.

import { langNames, setLangChoice, useLangChoice, useT } from "../i18n";
import type { LangChoice } from "../i18n";
import { useStore, type ThemeChoice } from "../store";
import { Icon } from "../ui/Icons";

const THEMES = [
  { value: "system", label: "settings.themeSystem" },
  { value: "light", label: "settings.themeLight" },
  { value: "dark", label: "settings.themeDark" },
] as const;

function Choice({
  label,
  chosen,
  onClick,
}: {
  label: string;
  chosen: boolean;
  onClick: () => void;
}) {
  return (
    <button className="ios-row" onClick={onClick}>
      <span className="grow truncate">{label}</span>
      {chosen && <Icon.Check size={16} className="ios-check" />}
    </button>
  );
}

export function AppearanceView() {
  const t = useT();
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const lang = useLangChoice();

  return (
    <div className="ios-page">
      <div className="ios-head">{t("settings.theme")}</div>
      <div className="ios-group">
        {THEMES.map((c) => (
          <Choice
            key={c.value}
            label={t(c.label)}
            chosen={theme === c.value}
            onClick={() => setTheme(c.value as ThemeChoice)}
          />
        ))}
      </div>

      {/* The language names are endonyms and stay put whatever the current
          language is: somebody hunting for German is looking for "Deutsch".
          Only "System" is translated, because it is a sentence about the machine
          rather than the name of a language. */}
      <div className="ios-head">{t("settings.language")}</div>
      <div className="ios-group">
        <Choice
          label={t("settings.languageAuto")}
          chosen={lang === "system"}
          onClick={() => setLangChoice("system")}
        />
        {langNames().map((l) => (
          <Choice
            key={l.value}
            label={l.label}
            chosen={lang === l.value}
            onClick={() => setLangChoice(l.value as LangChoice)}
          />
        ))}
      </div>
      <div className="ios-foot">{t("settings.languageHint")}</div>
    </div>
  );
}
