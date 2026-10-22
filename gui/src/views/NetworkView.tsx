// Rete: la riga di Impostazioni (12b) che dice attraverso cosa passa quello
// che mandi, e dove lo si cambia. Era dentro Avanzate, due volte: una riga
// qui e una lì. Una sola — questa.

import { useEffect, useState } from "react";
import { fire, useStore } from "../store";
import { useT } from "../i18n";
import { haptic } from "../haptics";
import { Icon } from "../ui/Icons";
import { Empty } from "../ui/Primitives";
import { InputRow } from "../ui/Rows";
import { toast } from "../ui/Toasts";

const SOURCE_KEY = {
  env: "settings.sourceEnv",
  config: "settings.sourceConfig",
  builtin: "settings.sourceBuiltin",
  none: "settings.sourceNone",
} as const;

export function NetworkView() {
  const t = useT();
  const config = useStore((s) => s.config);
  const error = useStore((s) => s.configError);
  const save = useStore((s) => s.saveConfig);
  const loadConfig = useStore((s) => s.loadConfig);
  const [relay, setRelay] = useState("");
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    fire(loadConfig());
  }, [loadConfig]);
  // A refresh may land while the field is being typed in, and it must not eat
  // what is there.
  useEffect(() => {
    if (!config) return;
    setRelay((r) => (dirty ? r : config.relay_configured));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config]);

  if (!config) {
    return (
      <div className="ios-page">
        <h2 className="page-title">{t("settings.network")}</h2>
        <Empty icon={<Icon.Relay size={22} />} title={error ?? t("common.loading")} />
      </div>
    );
  }
  const locked = config.relay_source === "env";

  const commit = () => {
    if (!dirty || locked) return;
    const next = relay.trim();
    if (next === config.relay_configured) {
      setDirty(false);
      return;
    }
    fire(
      save({ relay: next ? { set: next } : "clear" }).then(() => {
        setDirty(false);
        haptic.did();
        toast.info(t("settings.relaySaved"), t("settings.relaySavedDetail"));
      })
    );
  };

  /** Togliere la chiave dal file, non scrivere il valore di serie dentro di
   *  esso: sono due cose diverse e solo la prima è un ritorno al predefinito.
   *  Scrivere il builtin lo congelerebbe — una versione dopo, quando l'app
   *  cambia relay di serie, questo telefono resterebbe sul vecchio senza che
   *  nessuno l'abbia chiesto. */
  const reset = () => {
    setRelay("");
    setDirty(false);
    fire(
      save({ relay: "clear" }).then(() => {
        haptic.did();
        toast.info(t("settings.relayResetDone"), t("settings.relayResetDetail"));
      })
    );
  };

  return (
    <div className="ios-page">
      <h2 className="page-title">{t("settings.network")}</h2>
      <div className="ios-group">
        <InputRow
          label={t("settings.relayLabel")}
          value={relay}
          mono
          disabled={locked}
          placeholder={config.relay ?? t("settings.relayPlaceholder")}
          onChange={(v) => {
            setRelay(v);
            setDirty(true);
          }}
          onCommit={commit}
        />
      </div>
      <div className="ios-foot">
        {locked
          ? t("settings.relayLocked")
          : t(
              "settings.relayHint",
              config.relay ?? t("settings.relayNone"),
              config.relay_source in SOURCE_KEY
                ? t(SOURCE_KEY[config.relay_source as keyof typeof SOURCE_KEY])
                : config.relay_source
            )}
      </div>
      <div className="ios-foot">{t("settings.relayNote")}</div>

      {/* Solo quando c'è qualcosa da togliere. Un pulsante che rimette un
          valore che è già quello in uso non fa niente, e un pulsante che non fa
          niente è una domanda a cui l'utente risponde due volte per capire che
          non era una domanda. Con ARVOLO_RELAY in mano all'ambiente non c'è
          nulla da togliere qui dentro: la riga sopra lo dice già. */}
      {!locked && config.relay_configured.trim() !== "" && (
        <>
          <div className="ios-group">
            <button className="ios-row action" onClick={reset}>
              {t("settings.relayReset")}
            </button>
          </div>
          <div className="ios-foot">{t("settings.relayResetHint")}</div>
        </>
      )}
    </div>
  );
}
