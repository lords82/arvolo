// Impostazioni, come la sezione 12 del design le riordina: dall'ingranaggio in
// alto, con il titolo nella pagina; chi sei, e poi due gruppi — le cose tue
// (dispositivi, dove vanno i ricevuti) e come funziona l'app (aspetto e lingua,
// rete, avanzate). Niente altro: link, depositi e cronologia sono invii ancora
// vivi, non regolazioni, e stanno in Attività.
//
// L'identità è prima ed è *intera* — le parole dell'impronta a una misura che
// si legge al telefono, su due righe, perché è l'unica cosa per cui servono.

import { useEffect, useState } from "react";
import { fire, useStore } from "../store";
import { useT } from "../i18n";
import { haptic } from "../haptics";
import { Avatar, ShortId } from "../ui/Bits";
import { Icon } from "../ui/Icons";

export function YouView() {
  const t = useT();
  const push = useStore((s) => s.push);
  const status = useStore((s) => s.status);
  const config = useStore((s) => s.config);
  const save = useStore((s) => s.saveConfig);
  const loadConfig = useStore((s) => s.loadConfig);
  const loadDevices = useStore((s) => s.loadDevices);
  const guiVersion = useStore((s) => s.guiVersion);

  useEffect(() => {
    fire(loadDevices());
    fire(loadConfig());
  }, [loadDevices, loadConfig]);

  // Il nome che mostri si scrive dove si legge: il blocco in testa (12b) è un
  // campo. Si salva quando si esce dal campo, come ogni riga che si scrive.
  const [name, setName] = useState("");
  const [dirty, setDirty] = useState(false);
  useEffect(() => {
    if (!config) return;
    setName((n) => (dirty ? n : config.display_name));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config]);
  const commitName = () => {
    if (!dirty || !config) return;
    const next = name.trim();
    setDirty(false);
    if (next === config.display_name) return;
    fire(
      save({ display_name: next ? { set: next } : "clear" }).then(() => haptic.did())
    );
  };
  const shown = status?.display_name || t("you.unnamed");
  // Otto parole, quattro per riga: così le disegna il canvas (12b), ed è la
  // forma in cui si dettano.
  const words = (status?.fingerprint || "").split(/[\s-]+/).filter(Boolean);
  const half = Math.ceil(words.length / 2);
  const line1 = words.slice(0, half).join(" ");
  const line2 = words.slice(half).join(" ");

  const row = (label: string, glyph: JSX.Element, onClick: () => void) => (
    <button className="big-row r60" onClick={onClick}>
      <span className="row-glyph">{glyph}</span>
      <span className="grow" style={{ minWidth: 0 }}>
        <span className="big-row-title truncate">{label}</span>
      </span>
      <Icon.ChevronRight size={16} className="ios-chev" />
    </button>
  );

  return (
    <div className="ios-page">
      <h2 className="page-title">{t("title.settings")}</h2>

      {/* Chi sei, intero: il nome si scrive qui, l'impronta si legge. */}
      <div className="me-block">
        <Avatar name={shown} id={status?.public_id} size={60} />
        <span className="grow" style={{ minWidth: 0 }}>
          <input
            className="me-name me-name-input"
            value={name}
            maxLength={64}
            placeholder={t("you.unnamed")}
            aria-label={t("settings.nameLabel")}
            onChange={(e) => {
              setName(e.currentTarget.value);
              setDirty(true);
            }}
            onBlur={commitName}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
            }}
          />
          <span className="me-words mono" style={{ display: "block" }}>
            {line1 || "…"}
            {line2 ? (
              <>
                <br />
                {line2}
              </>
            ) : null}
          </span>
        </span>
      </div>

      <div className="ios-group">
        {row(t("title.devices"), <Icon.Devices size={20} />, () => push("devices"))}
        {row(t("received.title"), <Icon.Receive size={20} />, () => push("received"))}
      </div>

      <div className="ios-group">
        {/* Una luna, un'antenna e una chiave — non due ingranaggi. `Icon.Settings`
            è una ruota dentata a otto raggi e `Icon.Sun` un sole a otto raggi:
            a 20px sono lo stesso disegno, e due righe su tre si distinguevano
            solo leggendole. Il canvas (12b) dà luna, nuvola e chiave; l'antenna
            resta perché è il glifo che il relay ha in tutta l'app. */}
        {row(t("settings.appearanceLanguage"), <Icon.Moon size={20} />, () => push("appearance"))}
        {row(t("settings.network"), <Icon.Relay size={20} />, () => push("network"))}
        {row(t("settings.advanced"), <Icon.Key size={20} />, () => push("advanced"))}
      </div>

      <div className="ios-foot you-foot">
        {t("you.footer", guiVersion || "…")}
        {status?.public_id ? (
          <>
            {" · "}
            <ShortId value={status.public_id} />
          </>
        ) : null}
      </div>
    </div>
  );
}
