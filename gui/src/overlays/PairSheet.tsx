// Pairing, both kinds, in one sheet.
//
// The two are mechanically identical — a short SPAKE2 code over the relay's
// rendezvous — and consequentially nothing alike:
//
//   * **Contact pairing** trades *public* ids. Nobody learns anything the other
//     did not choose to send, and both ends come away saved *and verified*,
//     because a channel that only forms between two parties who knew the same
//     code authenticates whatever arrives through it.
//   * **Device pairing** hands over this device's *identity secret*. Afterwards
//     the two machines are the same person: one public id, one inbox. Joining
//     **replaces** the joining device's identity, and anything still sealed to
//     the old one stops being openable there.
//
// So the sheet never lets the two share a screen or a button. The device-host
// panel says what it is giving away before it shows a code, and the device-join
// panel refuses to start until the consequence has been ticked.

import { useEffect, useState } from "react";
import { useStore } from "../store";
import { Icon } from "../ui/Icons";
import { Button, Field, TextInput } from "../ui/Primitives";
import { CodeHero } from "../ui/Bits";
import { Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import type { PairKind } from "../types";

const TITLE: Record<PairKind, string> = {
  contact_host: "Scambia i contatti",
  contact_join: "Scambia i contatti",
  device_host: "Collega un altro tuo dispositivo",
  device_join: "Collega questo dispositivo",
};

const SUB: Record<PairKind, string> = {
  contact_host: "Mostragli il codice: vi salvate a vicenda, già verificati.",
  contact_join: "Inserisci il codice che ti ha letto.",
  device_host: "Condivide la tua identità con la macchina nuova.",
  device_join: "Sostituisce l'identità di questo dispositivo con la tua.",
};

export function PairSheet() {
  const pairing = useStore((s) => s.pairing);
  const start = useStore((s) => s.startPairing);
  const cancel = useStore((s) => s.cancelPairing);
  const clear = useStore((s) => s.clearPairing);
  const restartDaemon = useStore((s) => s.restartDaemon);
  const loadSync = useStore((s) => s.loadSync);

  const [code, setCode] = useState("");
  const [name, setName] = useState("");
  const [understood, setUnderstood] = useState(false);

  const kind = pairing?.kind;
  const phase = pairing?.phase;

  useEffect(() => {
    if (!pairing) {
      setCode("");
      setName("");
      setUnderstood(false);
    }
  }, [pairing]);

  if (!pairing || !kind) return null;

  const joining = kind === "contact_join" || kind === "device_join";
  const isDevice = kind === "device_host" || kind === "device_join";
  // A joining sheet parks in "starting" until the user submits the code: the
  // session is only opened at that point, so there is nothing to wait for yet.
  const notYetStarted = joining && !pairing.session && phase === "starting";

  const close = () => {
    if (phase === "done" || phase === "failed") clear();
    else void cancel();
  };

  const finish = async () => {
    if (pairing.needsRestart) {
      await restartDaemon();
      toast.info(
        "Riavvio del daemon",
        "Sta ripartendo con l'identità condivisa: qualche secondo e torna tutto."
      );
    }
    clear();
    void loadSync();
  };

  return (
    <Sheet
      open
      onClose={close}
      placement="center"
      title={TITLE[kind]}
      subtitle={SUB[kind]}
      footer={
        phase === "done" ? (
          <>
            <div className="spacer" />
            <Button variant="primary" onClick={finish} data-autofocus>
              {pairing.needsRestart ? "Riavvia e chiudi" : "Fatto"}
            </Button>
          </>
        ) : phase === "failed" ? (
          <>
            <div className="spacer" />
            <Button onClick={clear}>Chiudi</Button>
            <Button
              variant="primary"
              onClick={() => void start(kind, joining ? code : undefined, name || undefined)}
            >
              Riprova
            </Button>
          </>
        ) : notYetStarted ? (
          <>
            <div className="spacer" />
            <Button onClick={close}>Annulla</Button>
            <Button
              variant="primary"
              disabled={!code.trim() || (kind === "device_join" && !understood)}
              onClick={() => void start(kind, code.trim(), name.trim() || undefined)}
            >
              Collega
            </Button>
          </>
        ) : (
          <>
            <div className="spacer" />
            <Button onClick={close}>Annulla</Button>
          </>
        )
      }
    >
      {/* ---- outcome ------------------------------------------------- */}
      {phase === "done" && (
        <div className="stack">
          <div className="hstack">
            <span className="tone-ok">
              <Icon.Check size={22} />
            </span>
            <span className="t-head">Fatto</span>
          </div>
          <div className="t-sm t-sec">{pairing.message}</div>
          {pairing.needsRestart && (
            <div
              className="card card-pad t-sm"
              style={{ borderColor: "var(--amber)" }}
            >
              Il daemon sta ancora girando con l'identità precedente. Va riavviato
              perché il cambio abbia effetto — ci pensa il pulsante qui sotto.
            </div>
          )}
        </div>
      )}

      {phase === "failed" && (
        <div className="stack">
          <div className="hstack">
            <span className="tone-bad">
              <Icon.Alert size={20} />
            </span>
            <span className="t-head">Non ha funzionato</span>
          </div>
          <div className="t-sm t-sec" style={{ wordBreak: "break-word" }}>
            {pairing.message}
          </div>
        </div>
      )}

      {/* ---- hosting: show the code ---------------------------------- */}
      {!joining && phase !== "done" && phase !== "failed" && (
        <div className="stack">
          {isDevice && (
            <div
              className="card card-pad t-sm"
              style={{ borderColor: "var(--amber)" }}
            >
              <strong>Questo condivide la tua identità segreta.</strong> Chi
              inserisce il codice diventa te: stesso id pubblico, stessa casella,
              stessa rubrica. Usalo solo su una macchina tua. Il codice vale per
              un solo dispositivo e scade appena viene usato.
            </div>
          )}

          {pairing.code ? (
            <>
              <CodeHero
                value={pairing.code}
                caption={
                  isDevice
                    ? "Sull'altro dispositivo: Dispositivi → Collega questo dispositivo."
                    : "Leggiglielo. Lui apre Persone → Scambia contatti e lo inserisce."
                }
              />
              <div className="hstack-sm">
                <span className="spinner" />
                <span className="t-sm t-mut">
                  In attesa dell'altra parte… puoi chiudere per annullare.
                </span>
              </div>
            </>
          ) : (
            <div className="hstack">
              <span className="spinner" />
              <span className="t-sm t-mut">Preparo il codice…</span>
            </div>
          )}

          {!isDevice && (
            <div className="hint">
              Vengono scambiati solo gli id pubblici. La tua identità segreta e la
              tua rubrica non escono da qui.
            </div>
          )}
        </div>
      )}

      {/* ---- joining --------------------------------------------------- */}
      {joining && phase !== "done" && phase !== "failed" && (
        <div className="stack">
          {kind === "device_join" && (
            <div
              className="card card-pad t-sm"
              style={{ borderColor: "var(--red)" }}
            >
              <strong>Attenzione: è un'operazione irreversibile.</strong>{" "}
              L'identità attuale di questo dispositivo viene sostituita da quella
              condivisa. Tutto ciò che è ancora sigillato per la vecchia identità
              non sarà più apribile qui.
            </div>
          )}

          {notYetStarted ? (
            <>
              <Field
                label="Codice"
                hint="Quello mostrato sull'altra macchina, tipo 4821-crater-mango."
              >
                {({ id, describedBy }) => (
                  <TextInput
                    id={id}
                    data-autofocus
                    aria-describedby={describedBy}
                    className="mono"
                    big
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    value={code}
                    onChange={(e) => setCode(e.currentTarget.value)}
                    placeholder="4821-crater-mango"
                  />
                )}
              </Field>

              {kind === "contact_join" && (
                <Field
                  label="Come lo chiami (facoltativo)"
                  hint="Se lo lasci vuoto lo salvo con un nome ricavato dalla sua impronta, e poi lo rinomini quando vuoi."
                >
                  {({ id, describedBy }) => (
                    <TextInput
                      id={id}
                      aria-describedby={describedBy}
                      value={name}
                      onChange={(e) => setName(e.currentTarget.value)}
                      placeholder="es. Giulia"
                    />
                  )}
                </Field>
              )}

              {kind === "device_join" && (
                <label className="hstack" style={{ cursor: "pointer" }}>
                  <input
                    type="checkbox"
                    checked={understood}
                    onChange={(e) => setUnderstood(e.currentTarget.checked)}
                  />
                  <span className="t-sm">
                    Ho capito: questo dispositivo perde la sua identità attuale.
                  </span>
                </label>
              )}
            </>
          ) : (
            <div className="hstack">
              <span className="spinner" />
              <span className="t-sm t-mut">
                In attesa dell'altra macchina…
              </span>
            </div>
          )}
        </div>
      )}
    </Sheet>
  );
}
