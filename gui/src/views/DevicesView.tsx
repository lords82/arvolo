// Multi-device: several machines acting as one person.
//
// The mental model this screen has to install, because nothing else in the app
// implies it: linked devices do not *talk to each other*, they **are** each
// other. One identity, one public id, one inbox slot. Anyone who paired with you
// on one machine sees exactly one you. The address book is then kept in step
// through an encrypted cell on that shared inbox — which is why the sync state
// and the identity live on the same screen rather than in two places.
//
// There is no device *list*, and that is not an omission: the relay is
// zero-knowledge and nothing enumerates which machines hold the identity. Saying
// "3 dispositivi collegati" would be an invention. The fingerprint is what you
// compare, on each machine, to know they match.

import { fire, useStore } from "../store";
import { Icon } from "../ui/Icons";
import { Badge, Button, Empty, SwitchRow } from "../ui/Primitives";
import { CopyField, Fingerprint } from "../ui/Bits";
import { toast } from "../ui/Toasts";

function ago(unixSecs: number): string {
  if (!unixSecs) return "";
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - unixSecs);
  if (secs < 60) return "pochi secondi fa";
  if (secs < 3600) return `${Math.round(secs / 60)} minuti fa`;
  if (secs < 86400) return `${Math.round(secs / 3600)} ore fa`;
  return `${Math.round(secs / 86400)} giorni fa`;
}

export function DevicesView() {
  const sync = useStore((s) => s.sync);
  const loading = useStore((s) => s.syncLoading);
  const error = useStore((s) => s.syncError);
  const reload = useStore((s) => s.loadSync);
  const syncNow = useStore((s) => s.syncNow);
  const startPairing = useStore((s) => s.startPairing);
  const config = useStore((s) => s.config);
  const saveConfig = useStore((s) => s.saveConfig);
  const loadConfig = useStore((s) => s.loadConfig);

  if (error && !sync) {
    return (
      <div className="card card-pad t-sm" style={{ borderColor: "var(--red)" }}>
        {error}
        <div style={{ marginTop: 10 }}>
          <Button size="sm" onClick={() => fire(reload())}>
            Riprova
          </Button>
        </div>
      </div>
    );
  }

  if (!sync) {
    return (
      <div className="card">
        <Empty icon={<Icon.Devices size={22} />} title="Carico…" />
      </div>
    );
  }

  return (
    <div className="view-narrow stack">
      {/* ---- identity ------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="hstack">
          <span className="tone-in">
            <Icon.Users size={20} />
          </span>
          <div className="grow">
            <div className="t-head">La tua identità condivisa</div>
            <div className="hint">
              Ogni dispositivo collegato usa questa. Per il resto del mondo sei
              una persona sola, ovunque tu apra Arvolo.
            </div>
          </div>
        </div>

        <div className="stack-sm">
          <div className="t-label">Impronta</div>
          <div
            className="card card-pad"
            style={{ background: "var(--surface-2)" }}
          >
            <Fingerprint value={sync.fingerprint} />
          </div>
          <div className="hint">
            Deve essere identica su tutti i tuoi dispositivi. Se su una macchina
            leggi parole diverse, quella non è collegata: è un'altra identità.
          </div>
        </div>

        <div className="stack-sm">
          <div className="t-label">Id pubblico</div>
          <CopyField value={sync.public_id} wrap />
        </div>
      </div>

      {/* ---- pairing -------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">Collega un dispositivo</div>
        <div className="t-sm t-sec">
          Il collegamento va fatto da entrambe le parti: su questa macchina
          mostri un codice, sull'altra lo inserisci. È un'operazione delicata —
          quello che passa è la tua identità segreta, non un semplice invito.
        </div>
        <div className="hstack wrap">
          <Button
            variant="primary"
            onClick={() => fire(startPairing("device_host"))}
          >
            <Icon.Qr size={14} /> Mostra un codice
          </Button>
          <Button onClick={() => fire(startPairing("device_join"))}>
            <Icon.Key size={14} /> Ho un codice
          </Button>
        </div>
        <div
          className="card card-pad t-xs"
          style={{ borderColor: "var(--amber)", background: "var(--amber-soft)" }}
        >
          <strong>Su una macchina che non è tua, mai.</strong> Chi inserisce il
          codice diventa te a tutti gli effetti: stessa casella, stessa rubrica,
          stessa capacità di aprire ciò che ti viene mandato.
        </div>
      </div>

      {/* ---- sync ----------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="hstack">
          <div className="grow">
            <div className="t-head">Rubrica in sincronia</div>
            <div className="hint">
              I contatti viaggiano fra i tuoi dispositivi dentro una cella
              cifrata sulla tua casella. Il relay conserva byte che non sa
              leggere.
            </div>
          </div>
          <Badge kind={sync.enabled ? "ok" : "neutral"}>
            {sync.enabled ? "Attiva" : "Disattivata"}
          </Badge>
        </div>

        <div className="hstack wrap">
          <span className="t-sm t-sec">
            {sync.contacts} contatt{sync.contacts === 1 ? "o" : "i"} in rubrica
          </span>
          <span className="t-sm t-mut">·</span>
          <span className="t-sm t-sec">
            {sync.last_sync
              ? `ultima sincronizzazione ${ago(sync.last_sync)}`
              : "non ancora sincronizzata da quando il daemon è partito"}
          </span>
        </div>

        {sync.last_error && (
          <div
            className="card card-pad t-sm"
            style={{ borderColor: "var(--red)" }}
          >
            L'ultimo giro non è riuscito: {sync.last_error}
          </div>
        )}

        <div className="hstack">
          <Button onClick={() => fire(syncNow())} busy={loading}>
            <Icon.Refresh size={14} /> Sincronizza adesso
          </Button>
        </div>

        <div className="divider" />

        <SwitchRow
          title="Sincronizza da sola"
          desc="Il daemon fa un giro ogni pochi minuti. Se la disattivi, la rubrica si allinea solo quando premi il pulsante qui sopra."
          checked={config ? config.sync : sync.enabled}
          onChange={async (v) => {
            await saveConfig({ sync: { set: v } });
            await loadConfig();
            toast.info(
              v ? "Sincronizzazione automatica attiva" : "Sincronizzazione automatica spenta",
              "Ha effetto al prossimo avvio del daemon."
            );
          }}
        />
      </div>
    </div>
  );
}
