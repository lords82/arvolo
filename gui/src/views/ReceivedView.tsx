// Impostazioni › Ricevuti: dove va quello che arriva.
//
// Due blocchi e nessuna sorpresa. Il primo è l'unica scelta che c'è — foto e
// video nella Galleria, solo in Arvolo, oppure chiedimelo ogni volta — e il
// secondo è un fatto, non una scelta: la cartella sul telefono è fissa, e dirlo
// come se fosse configurabile sarebbe una bugia con un chevron accanto.
//
// La scelta vale per quello che arriva da *chiunque*, compresi i tuoi
// dispositivi, e non tocca mai gli altri file: un PDF nella Galleria non lo
// vuole nessuno, e il design lo dice prima di ogni schermata (sezione 6).
//
// Una regola che non si vede qui ma che questa schermata presuppone: il
// trasferimento non aspetta mai questa risposta. Quello che arriva è già nella
// cartella di Arvolo prima che la domanda esista; la Galleria è una copia in
// più, decisa dopo.

import { useEffect, useState } from "react";
import { api } from "../ipc";
import { useT } from "../i18n";
import { phoneOs } from "../phone";
import { useStore, type PhotosChoice } from "../store";
import { Icon } from "../ui/Icons";

/** Il nome della cartella, per piattaforma — la stessa frase che la schermata di
 *  ricezione dice da sempre, perché è lo stesso posto. */
const WHERE_KEY = {
  android: "receive.whereAndroid",
  ios: "receive.whereIos",
  other: "receive.whereHere",
} as const;

const CHOICES: { value: PhotosChoice; label: string; sub?: string }[] = [
  { value: "gallery", label: "received.inGallery", sub: "received.inGallerySub" },
  { value: "arvolo", label: "received.onlyArvolo" },
  { value: "ask", label: "received.askEvery", sub: "received.askEverySub" },
];

export function ReceivedView() {
  const t = useT();
  const photos = useStore((s) => s.photos);
  const setPhotos = useStore((s) => s.setPhotos);
  const [hasGallery, setHasGallery] = useState(true);

  // Una scelta che questa piattaforma non può onorare non va mostrata: sulla
  // finestra non c'è nessuna Galleria a cui aggiungere.
  useEffect(() => {
    api
      .photosAvailable()
      .then(setHasGallery)
      .catch(() => setHasGallery(false));
  }, []);

  return (
    <div className="ios-page">
      {hasGallery && (
        <div>
          <div className="ios-head">{t("received.photosLabel")}</div>
          <div className="ios-group">
            {CHOICES.map((c) => (
              <button
                key={c.value}
                className="ios-row"
                onClick={() => setPhotos(c.value)}
              >
                <span className="grow" style={{ minWidth: 0 }}>
                  <span className="ios-row-label">{t(c.label as never)}</span>
                  {c.sub ? (
                    <span className="ios-sub">{t(c.sub as never)}</span>
                  ) : null}
                </span>
                {photos === c.value && (
                  <Icon.Check size={18} className="ios-check" />
                )}
              </button>
            ))}
          </div>
          <div className="ios-foot">{t("received.photosFoot")}</div>
        </div>
      )}

      <div>
        <div className="ios-head">{t("received.whereLabel")}</div>
        <div className="ios-group">
          {/* Non un bottone: non c'è niente da scegliere. Su un telefono la
              cartella è decisa dal sistema, e il massimo che questa riga può
              fare è dire quale. */}
          <div className="ios-row" style={{ cursor: "default" }}>
            <Icon.Folder size={17} className="t-sec" />
            <span className="grow truncate">{t(WHERE_KEY[phoneOs()])}</span>
          </div>
        </div>
        <div className="ios-foot">{t("received.whereFoot")}</div>
      </div>
    </div>
  );
}
