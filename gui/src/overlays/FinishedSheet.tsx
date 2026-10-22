// Finito: cosa è arrivato, dove è andato, e l'unica domanda che vale la pena
// fare — una volta sola.
//
// È la schermata che il design chiama *Finito* (10d, e 6a/6b per la domanda).
// Compare quando qualcosa **arriva**: un invio che finisce non è un incontro, è
// una riga che smette di muoversi.
//
// La regola che la governa viene prima di ogni pixel: **il trasferimento non
// aspetta nessuna risposta**. Quello che è arrivato è già nella cartella di
// Arvolo quando questa schermata si apre; la Galleria è una copia in più. Per
// questo la domanda si fa qui e non prima, e per questo la si fa **una volta
// sola**: la risposta diventa la preferenza, e dalla seconda volta questa
// schermata si limita a dire dove sono andate le cose, con il modo di disfarlo.

import { useState } from "react";
import { api } from "../ipc";
import { fire, useStore } from "../store";
import { fmtBytes, isMedia } from "../format";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import { Button, Switch } from "../ui/Primitives";
import { ExtChip } from "../ui/Bits";
import { Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";

export function FinishedSheet() {
  const t = useT();
  const finished = useStore((s) => s.finished);
  const close = useStore((s) => s.closeFinished);
  const transfers = useStore((s) => s.transfers);
  const photos = useStore((s) => s.photos);
  const setPhotos = useStore((s) => s.setPhotos);
  const putInGallery = useStore((s) => s.putInGallery);
  const takeOutOfGallery = useStore((s) => s.takeOutOfGallery);
  const inGallery = useStore((s) => s.inGallery);
  const peerLabel = useStore((s) => s.peerLabel);
  const contactsById = useStore((s) => s.contactsById);
  const openPerson = useStore((s) => s.openPerson);

  /** «Sempre» è acceso di default: chi lo spegne sceglie solo per stavolta. */
  const [always, setAlways] = useState(true);

  const rows = finished.map((k) => transfers[k]).filter(Boolean);
  if (!rows.length) return null;

  const media = rows.filter((tx) => isMedia(tx.name));
  const who =
    rows[0].peer || peerLabel(rows[0].peerId ?? null, rows[0].senderName);
  const named = who && who !== "unknown";
  const known = rows[0].peerId ? !!contactsById[rows[0].peerId] : false;
  const saved = media.some((tx) => inGallery[tx.key]);
  /** La domanda si fa solo quando c'è qualcosa da chiedere: foto o video, e
   *  nessuna risposta ancora data. */
  const asking = photos === "ask" && media.length > 0;

  const answer = async (choice: "gallery" | "arvolo") => {
    if (always) setPhotos(choice);
    if (choice === "gallery") await putInGallery(media.map((tx) => tx.key));
  };

  return (
    <Sheet open onClose={close} title={t("finished.title")} pushFrom={t("common.done")}>
      <div className="stack">
        <div className="fin-head">
          <span className="fin-tick">
            <Icon.Check size={18} />
          </span>
          <span className="fin-count tnum">
            {t(
              "finished.count",
              rows.length,
              fmtBytes(rows.reduce((n, tx) => n + tx.size, 0)),
              named ? who : t("finished.someone")
            )}
          </span>
        </div>

        {/* Dove è andato cosa, in una riga. Solo quando c'è più di un posto in
            cui sia andato qualcosa: se è tutto nella cartella di Arvolo, dirlo
            sarebbe ripetere la cosa che l'app fa sempre. */}
        {!asking && media.length > 0 && (
          <div className="fin-where">
            <span className="grow">
              {saved ? t("received.wentToGallery") : t("received.allInArvolo")}
            </span>
            {saved && (
              <button
                className="fin-undo"
                onClick={() =>
                  fire(
                    Promise.all(
                      media.map((tx) => takeOutOfGallery(tx.key))
                    ).then(() => {})
                  )
                }
              >
                {t("finished.takeOut")}
              </button>
            )}
          </div>
        )}

        {/* La prima volta, la domanda. Riguarda solo le foto, e lo dice: il PDF
            che è arrivato con loro non c'entra e resta dov'è. */}
        {asking && (
          <div className="fin-ask">
            <div>
              <span className="fin-ask-title">
                {t("finished.askTitle", media.length)}
              </span>
              {media.length < rows.length && (
                <span className="fin-ask-sub">{t("finished.askRest")}</span>
              )}
            </div>
            <div className="fin-ask-choices">
              <button className="fin-choice ink" onClick={() => fire(answer("gallery"))}>
                <Icon.Image size={20} />
                <span className="fin-choice-title">{t("received.inGallery")}</span>
                <span className="fin-choice-sub">{t("finished.withYourPhotos")}</span>
              </button>
              <button className="fin-choice" onClick={() => fire(answer("arvolo"))}>
                <Icon.Folder size={20} />
                <span className="fin-choice-title">{t("received.onlyArvolo")}</span>
                <span className="fin-choice-sub">{t("finished.inFiles")}</span>
              </button>
            </div>
            <label className="fin-always">
              <span className="grow">{t("finished.always")}</span>
              <Switch
                checked={always}
                onChange={setAlways}
                label={t("finished.always")}
              />
            </label>
          </div>
        )}

        <div className="ios-group">
          {rows.map((tx) => (
            <div key={tx.key} className="fin-file">
              <ExtChip name={tx.name} />
              <span className="grow" style={{ minWidth: 0 }}>
                <span className="fin-file-name">{tx.name}</span>
                <span className="fin-file-meta tnum">
                  {fmtBytes(tx.size)}
                  {inGallery[tx.key] ? ` · ${t("received.inGalleryShort")}` : ""}
                </span>
              </span>
              {tx.path && (
                <button
                  className="fin-open"
                  onClick={() =>
                    api
                      .openPath(tx.path!)
                      .catch((e: unknown) =>
                        toast.bad(t("transfers.openFileFailed"), String(e))
                      )
                  }
                >
                  {t("common.open")}
                </button>
              )}
            </div>
          ))}
        </div>

        {/* Chi te l'ha mandato, se non ce l'hai. Qui e non altrove: è il momento
            in cui sai chi è, e l'unico in cui la domanda non è fuori posto. */}
        {named && !known && (
          <div className="fin-save">
            <div>
              <div className="fin-save-title">{t("finished.saveWho", who)}</div>
              <div className="fin-save-sub">{t("finished.saveWhy")}</div>
            </div>
            <div className="hstack-sm">
              <Button size="sm" onClick={() => openPerson(who)}>
                {t("common.save")}
              </Button>
              <Button size="sm" variant="ghost" onClick={close}>
                {t("finished.notNow")}
              </Button>
            </div>
          </div>
        )}

        <div className="phone-actions">
          <Button variant="primary" size="lg" onClick={close}>
            {t("common.done")}
          </Button>
        </div>
      </div>
    </Sheet>
  );
}
