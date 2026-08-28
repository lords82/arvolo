// Choosing what arrives without being asked about.
//
// Three exclusive answers, each with a consequence worth a sentence — which on
// this platform is a screen you push into, with a checkmark against the one in
// force and the reason underneath the group. A strip of three words would have
// had room for the words and none for what they cost, and this is a choice where
// what it costs is the whole of the decision.

import { useT } from "../i18n";
import { fire, useStore } from "../store";
import { Icon } from "../ui/Icons";

const CHOICES = [
  {
    value: "never",
    label: "settings.autoAcceptNever",
    hint: "settings.autoAcceptHintNever",
  },
  {
    value: "trusted",
    label: "settings.autoAcceptTrusted",
    hint: "settings.autoAcceptHintTrusted",
  },
  {
    value: "everyone",
    label: "settings.autoAcceptEveryone",
    hint: "settings.autoAcceptHintEveryone",
  },
] as const;

export function AutoAcceptView() {
  const t = useT();
  const config = useStore((s) => s.config);
  const save = useStore((s) => s.saveConfig);
  const current = config?.auto_accept || "trusted";

  return (
    <div className="ios-page">
      <div className="ios-group">
        {CHOICES.map((c) => (
          <button
            key={c.value}
            className="ios-row"
            onClick={() => fire(save({ auto_accept: { set: c.value } }))}
          >
            <span className="grow truncate">{t(c.label)}</span>
            {current === c.value && (
              <Icon.Check size={16} className="ios-check" />
            )}
          </button>
        ))}
      </div>
      {/* The footnote under a group, which is where this platform puts the
          sentence that says what you just chose actually does. */}
      <div className="ios-foot">
        {t(CHOICES.find((c) => c.value === current)?.hint ?? CHOICES[1].hint)}
      </div>
    </div>
  );
}
