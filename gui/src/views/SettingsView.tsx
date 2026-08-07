// Settings, and the identity they hang off.
//
// The rule this screen follows everywhere: show what is **in force**, and say
// **where it comes from**. A relay supplied by `ARVOLO_RELAY` or compiled into
// the binary is not a saved setting, and presenting it in an editable field
// would invite the user to change something the environment will keep
// overriding. So each such field carries its provenance, and the field is
// disabled when editing it could not win.
//
// What is deliberately *not* here: temp dir, iroh relay, log level, identity
// path. Those are text-file settings whose wrong value is hard to recover from
// through a dialog. The screen links to `config.toml` instead of half-exposing
// them.

import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { useStore, type ThemeChoice } from "../store";
import { Icon } from "../ui/Icons";
import {
  Badge,
  Button,
  Empty,
  Field,
  Segmented,
  SwitchRow,
  TextInput,
} from "../ui/Primitives";
import { CopyField, Fingerprint } from "../ui/Bits";
import { Confirm } from "../ui/Sheet";
import { toast } from "../ui/Toasts";

const SOURCE_LABEL: Record<string, string> = {
  env: "imposto dalla variabile ARVOLO_RELAY",
  config: "salvato nelle impostazioni",
  builtin: "predefinito, incluso nell'app",
  none: "nessuno",
};

export function SettingsView() {
  const config = useStore((s) => s.config);
  const loading = useStore((s) => s.configLoading);
  const error = useStore((s) => s.configError);
  const reload = useStore((s) => s.loadConfig);
  const save = useStore((s) => s.saveConfig);
  const status = useStore((s) => s.status);
  const guiVersion = useStore((s) => s.guiVersion);
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const restartDaemon = useStore((s) => s.restartDaemon);

  const [name, setName] = useState("");
  const [relay, setRelay] = useState("");
  const [dirty, setDirty] = useState<{ name?: boolean; relay?: boolean }>({});
  const [busy, setBusy] = useState(false);
  const [confirmRestart, setConfirmRestart] = useState(false);

  // Reset the drafts whenever a fresh config lands, but never clobber a field
  // the user is mid-edit in: a background refresh must not eat what they typed.
  useEffect(() => {
    if (!config) return;
    setName((n) => (dirty.name ? n : config.display_name));
    setRelay((r) => (dirty.relay ? r : config.relay_configured));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config]);

  if (error && !config) {
    return (
      <div className="card card-pad t-sm" style={{ borderColor: "var(--red)" }}>
        {error}
        <div style={{ marginTop: 10 }}>
          <Button size="sm" onClick={() => void reload()}>
            Riprova
          </Button>
        </div>
      </div>
    );
  }
  if (!config) {
    return (
      <div className="card">
        <Empty icon={<Icon.Settings size={22} />} title="Carico…" />
      </div>
    );
  }

  const relayLocked = config.relay_source === "env";

  const saveName = async () => {
    setBusy(true);
    try {
      await save({ display_name: name.trim() ? { set: name.trim() } : "clear" });
      setDirty((d) => ({ ...d, name: false }));
      toast.ok(
        "Nome aggiornato",
        "Viaggia dentro ogni offerta che mandi, da subito."
      );
    } finally {
      setBusy(false);
    }
  };

  const saveRelay = async () => {
    setBusy(true);
    try {
      await save({ relay: relay.trim() ? { set: relay.trim() } : "clear" });
      setDirty((d) => ({ ...d, relay: false }));
      toast.info(
        "Relay salvato",
        "Il daemon lo userà al prossimo avvio: riavvialo qui sotto per applicarlo subito."
      );
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="view-narrow stack">
      {/* ---- identity ------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">Chi sei</div>

        <Field
          label="Nome che mostri"
          hint="Viaggia dentro ogni offerta sigillata che mandi. È un'etichetta che scegli tu: chi la riceve la vede fra virgolette, perché niente la garantisce. L'unica cosa che ti identifica davvero è l'impronta qui sotto."
        >
          {({ id, describedBy }) => (
            <div className="hstack-sm">
              <TextInput
                id={id}
                aria-describedby={describedBy}
                value={name}
                maxLength={64}
                onChange={(e) => {
                  setName(e.currentTarget.value);
                  setDirty((d) => ({ ...d, name: true }));
                }}
                placeholder="nessuno"
              />
              <Button
                size="sm"
                disabled={!dirty.name || busy}
                busy={busy && dirty.name}
                onClick={saveName}
              >
                Salva
              </Button>
            </div>
          )}
        </Field>

        <Field
          label="La tua impronta"
          hint="Le parole che gli altri confrontano per essere certi che sei tu. Leggile a voce quando qualcuno ti aggiunge."
        >
          {() => (
            <div
              className="card card-pad"
              style={{ background: "var(--surface-2)" }}
            >
              <Fingerprint value={status?.fingerprint ?? ""} />
            </div>
          )}
        </Field>

        <Field label="Il tuo id pubblico">
          {() => <CopyField value={status?.public_id ?? ""} wrap />}
        </Field>
      </div>

      {/* ---- aspetto -------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">Aspetto</div>
        <Field label="Tema">
          {() => (
            <Segmented
              block
              label="Tema"
              value={theme}
              onChange={(v) => setTheme(v as ThemeChoice)}
              options={[
                { value: "system", label: "Sistema" },
                { value: "light", label: "Chiaro" },
                { value: "dark", label: "Scuro" },
              ]}
            />
          )}
        </Field>
      </div>

      {/* ---- rete ----------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="hstack">
          <div className="grow t-head">Rete</div>
          <Badge kind={config.relay ? "ok" : "warn"}>
            <Icon.Relay size={10} />
            {config.relay ? "Relay attivo" : "Nessun relay"}
          </Badge>
        </div>

        <Field
          label="Relay"
          hint={
            relayLocked
              ? "In questo momento lo decide la variabile d'ambiente ARVOLO_RELAY: quello che scrivi qui non avrebbe effetto finché è impostata."
              : `In uso adesso: ${config.relay ?? "nessuno"} — ${SOURCE_LABEL[config.relay_source] ?? config.relay_source}. Un indirizzo senza schema diventa https://; per un relay in chiaro scrivi lo schema per esteso, tipo http://relay.local:6282.`
          }
        >
          {({ id, describedBy }) => (
            <div className="hstack-sm">
              <TextInput
                id={id}
                aria-describedby={describedBy}
                className="mono"
                spellCheck={false}
                disabled={relayLocked}
                value={relay}
                onChange={(e) => {
                  setRelay(e.currentTarget.value);
                  setDirty((d) => ({ ...d, relay: true }));
                }}
                placeholder={config.relay ?? "relay.esempio.it"}
              />
              <Button
                size="sm"
                disabled={!dirty.relay || relayLocked || busy}
                onClick={saveRelay}
              >
                Salva
              </Button>
            </div>
          )}
        </Field>

        <div className="hint">
          Il relay smista i codici, la casella e i link. Non vede mai i tuoi
          file in chiaro: quello che conserva è cifrato con chiavi che non ha.
        </div>
      </div>

      {/* ---- file ----------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">File</div>

        <Field
          label="Dove finiscono i file ricevuti"
          hint={
            config.download_dir_from_env
              ? "Deciso dalla variabile ARVOLO_DOWNLOAD_DIR."
              : "Vale per quello che accetti senza scegliere una cartella al volo."
          }
        >
          {({ id, describedBy }) => (
            <div className="hstack-sm">
              <TextInput
                id={id}
                aria-describedby={describedBy}
                readOnly
                value={config.download_dir}
              />
              <Button
                size="sm"
                disabled={config.download_dir_from_env || busy}
                onClick={async () => {
                  const picked = await openDialog({ directory: true });
                  if (typeof picked !== "string") return;
                  await save({ download_dir: { set: picked } });
                  toast.info(
                    "Cartella aggiornata",
                    "Il daemon la userà al prossimo avvio."
                  );
                }}
              >
                <Icon.Folder size={13} /> Cambia
              </Button>
              <Button
                size="sm"
                onClick={() =>
                  openPath(config.download_dir).catch((e: unknown) =>
                    toast.bad("Non riesco ad aprirla", String(e))
                  )
                }
              >
                Apri
              </Button>
            </div>
          )}
        </Field>

        <div className="divider" />

        <SwitchRow
          title="Continua a condividere ciò che hai scaricato"
          desc="Lasciando attivo il seeding aiuti chi sta scaricando lo stesso file. Puoi spegnerlo se preferisci non restare nello swarm."
          checked={config.seed ?? true}
          onChange={async (v) => {
            await save({ seed: { set: v } });
            toast.info(
              "Impostazione salvata",
              "Ha effetto al prossimo avvio del daemon."
            );
          }}
        />
      </div>

      {/* ---- avanzate ------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">Avanzate</div>

        <Field
          label="File di configurazione"
          hint="Tutto ciò che non compare qui — cartella temporanea, relay NAT, livello di log — si imposta a mano in questo file, che è commentato riga per riga."
        >
          {() => (
            <div className="hstack-sm">
              <TextInput readOnly value={config.config_path} className="mono" />
              <Button
                size="sm"
                onClick={() =>
                  openPath(config.config_path).catch((e: unknown) =>
                    toast.bad("Non riesco ad aprirlo", String(e))
                  )
                }
              >
                <Icon.External size={13} /> Apri
              </Button>
            </div>
          )}
        </Field>

        <Field
          label="Chiave d'identità"
          hint="Il tuo segreto. Non condividerlo: chi lo possiede diventa te. Per usare Arvolo su un'altra tua macchina c'è il collegamento dispositivi, che lo trasferisce cifrato."
        >
          {() => (
            <TextInput readOnly value={config.identity_path} className="mono" />
          )}
        </Field>

        <div className="divider" />

        <div className="hstack wrap">
          <span className="t-sm t-sec grow">
            Daemon {status?.version || "?"} · interfaccia {guiVersion || "?"}
          </span>
          <Button size="sm" onClick={() => setConfirmRestart(true)}>
            <Icon.Refresh size={13} /> Riavvia il daemon
          </Button>
        </div>
      </div>

      <Confirm
        open={confirmRestart}
        title="Riavviare il daemon?"
        body="I trasferimenti in corso si fermano: quelli ripristinabili riprendono da dove erano, gli altri vanno rifatti da capo. Serve per applicare relay e cartelle appena cambiati."
        confirmLabel="Riavvia"
        onCancel={() => setConfirmRestart(false)}
        onConfirm={async () => {
          setConfirmRestart(false);
          await restartDaemon();
          toast.info("Daemon in riavvio", "Torna su da solo in qualche secondo.");
        }}
      />

      {loading && <div className="t-xs t-mut">Aggiorno…</div>}
    </div>
  );
}
