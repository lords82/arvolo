// Everything else, on a phone.
//
// The desktop's settings are a window: eight cards, each with a title, captioned
// fields and a Save beside every one of them. That shape was the last thing in
// the app still drawn for a mouse — pushed into from *You*, it arrived as a page
// of forms in the middle of a phone made of lists.
//
// So: grouped inset lists, a value on the right of its own row, saving on blur,
// and the explanation under the group rather than under the field.
//
// What is *not* here is the other half of the redesign. Four of the desktop's
// controls have nothing to act on here — a phone has no login items, no
// self-updating binary, no daemon of its own to restart (the engine is inside
// this process), and no text editor to open `config.toml` in. Showing them
// disabled would be the app claiming a capability and then refusing it; the
// honest version is a screen with only the rows that do something. The
// config-file path stays, as a line of fact, because "where does this live" is a
// fair question even where the answer cannot be tapped.

import { useEffect, useState } from "react";
import { useT } from "../i18n";
import { fire, useStore } from "../store";
import { haptic } from "../haptics";
import { Empty } from "../ui/Primitives";
import { InputRow, Place, ToggleRow, ValueRow } from "../ui/Rows";
import { Icon } from "../ui/Icons";
import { toast } from "../ui/Toasts";

const SOURCE_KEY = {
  env: "settings.sourceEnv",
  config: "settings.sourceConfig",
  builtin: "settings.sourceBuiltin",
  none: "settings.sourceNone",
} as const;

/** The last two components of a path — "Download/Arvolo" out of a dozen
 *  characters of Android sandbox nobody can act on. The full one is a row's
 *  title attribute away, and on a phone it would only ever wrap. */
function shortPath(p: string): string {
  const parts = p.split("/").filter(Boolean);
  return parts.slice(-2).join("/") || p;
}

export function AdvancedView() {
  const t = useT();
  const push = useStore((s) => s.push);
  const config = useStore((s) => s.config);
  const error = useStore((s) => s.configError);
  const save = useStore((s) => s.saveConfig);
  const status = useStore((s) => s.status);
  const guiVersion = useStore((s) => s.guiVersion);
  const theme = useStore((s) => s.theme);

  const [name, setName] = useState("");
  const [relay, setRelay] = useState("");
  const [dirty, setDirty] = useState<{ name?: boolean; relay?: boolean }>({});

  // Same rule as the desktop's: a refresh may land while a field is being typed
  // in, and it must not eat what is there.
  useEffect(() => {
    if (!config) return;
    setName((n) => (dirty.name ? n : config.display_name));
    setRelay((r) => (dirty.relay ? r : config.relay_configured));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config]);

  if (!config) {
    return (
      <div className="ios-page">
        <Empty
          icon={<Icon.Settings size={22} />}
          title={error ?? t("common.loading")}
        />
      </div>
    );
  }

  const relayLocked = config.relay_source === "env";

  /** Copying is the only thing these two rows can do, and on a phone there is no
   *  banner to say so — the confirmation is the knock. */
  const copy = (value: string) => {
    if (!value) return;
    void navigator.clipboard
      .writeText(value)
      .then(() => haptic.did())
      .catch(() => haptic.refused());
  };

  const commitName = () => {
    if (!dirty.name) return;
    const next = name.trim();
    if (next === config.display_name) {
      setDirty((d) => ({ ...d, name: false }));
      return;
    }
    fire(
      save({ display_name: next ? { set: next } : "clear" }).then(() => {
        setDirty((d) => ({ ...d, name: false }));
        haptic.did();
      })
    );
  };

  const commitRelay = () => {
    if (!dirty.relay || relayLocked) return;
    const next = relay.trim();
    if (next === config.relay_configured) {
      setDirty((d) => ({ ...d, relay: false }));
      return;
    }
    fire(
      save({ relay: next ? { set: next } : "clear" }).then(() => {
        setDirty((d) => ({ ...d, relay: false }));
        haptic.did();
        toast.info(t("settings.relaySaved"), t("settings.relaySavedDetail"));
      })
    );
  };

  return (
    <div className="ios-page">
      {/* ---- who you are ---------------------------------------------- */}
      <div className="ios-group">
        <InputRow
          label={t("settings.nameLabel")}
          value={name}
          maxLength={64}
          placeholder={t("settings.namePlaceholder")}
          onChange={(v) => {
            setName(v);
            setDirty((d) => ({ ...d, name: true }));
          }}
          onCommit={commitName}
        />
      </div>
      <div className="ios-foot">{t("settings.nameHint")}</div>

      <div className="ios-head">{t("adv.identity")}</div>
      <div className="ios-group">
        {/* The address and the words that prove it. Both are longer than a row,
            so the row shows the head of one and hands over the whole of it. */}
        <ValueRow
          label={t("settings.publicIdLabel")}
          value={status?.public_id?.slice(0, 10) ?? "…"}
          mono
          onClick={() => copy(status?.shareable_id || status?.public_id || "")}
        />
        {/* Two of the eight words, not eight: the fingerprint is joined by
            hyphens (see `Identity::fingerprint`), so splitting it on spaces
            returned the whole of it, the value pushed the label out of its own
            row, and "La tua impronta" was nowhere on screen. A row shows enough
            to recognise; the tap hands over all of it. */}
        <ValueRow
          label={t("settings.fingerprintLabel")}
          value={status?.fingerprint?.split("-").slice(0, 2).join("-") ?? "…"}
          onClick={() => copy(status?.fingerprint ?? "")}
        />
      </div>
      <div className="ios-foot">{t("adv.copyHint")}</div>

      {/* ---- appearance ------------------------------------------------ */}
      <div className="ios-group">
        <Place
          label={t("settings.appearance")}
          note={t(
            theme === "light"
              ? "settings.themeLight"
              : theme === "dark"
                ? "settings.themeDark"
                : "settings.themeSystem"
          )}
          onClick={() => push("appearance")}
        />
      </div>

      {/* ---- files ------------------------------------------------------ */}
      {/* No "where files land" row. It named a folder that cannot be changed on
          either phone — `Download/Arvolo` through MediaStore, `Documents`
          through Files — and a screen of settings is a screen of things that
          are settable. What a person actually wants from it is the file, and
          that is one tap on the row that received it. */}
      <div className="ios-head">{t("settings.files")}</div>
      <div className="ios-group">
        <ToggleRow
          label={t("settings.seedTitle")}
          checked={config.seed ?? true}
          onChange={(v) =>
            fire(
              save({ seed: { set: v } }).then(() => {
                haptic.did();
                toast.info(t("settings.saved"), t("common.afterRestart"));
              })
            )
          }
        />
      </div>
      <div className="ios-foot">{t("settings.seedDesc")}</div>

      {/* ---- network ---------------------------------------------------- */}
      <div className="ios-head">{t("settings.network")}</div>
      <div className="ios-group">
        <InputRow
          label={t("settings.relayLabel")}
          value={relay}
          mono
          disabled={relayLocked}
          placeholder={config.relay ?? t("settings.relayPlaceholder")}
          onChange={(v) => {
            setRelay(v);
            setDirty((d) => ({ ...d, relay: true }));
          }}
          onCommit={commitRelay}
        />
      </div>
      <div className="ios-foot">
        {relayLocked
          ? t("settings.relayLocked")
          : t(
              "settings.relayHint",
              config.relay ?? t("settings.relayNone"),
              config.relay_source in SOURCE_KEY
                ? t(SOURCE_KEY[config.relay_source as keyof typeof SOURCE_KEY])
                : config.relay_source
            )}
      </div>

      {/* ---- what this is ----------------------------------------------- */}
      <div className="ios-head">{t("adv.about")}</div>
      <div className="ios-group">
        <ValueRow label={t("adv.version")} value={guiVersion || "…"} />
        <ValueRow label={t("adv.engine")} value={status?.version || "…"} />
        <ValueRow
          label={t("settings.configFileLabel")}
          value={shortPath(config.config_path)}
          mono
        />
      </div>
      <div className="ios-foot">{t("settings.configFileHint")}</div>
    </div>
  );
}
