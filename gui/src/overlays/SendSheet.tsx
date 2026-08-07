// Sending, in one panel with four answers to "who is this for?".
//
// The CLI has four verbs here — `send --to`, `code`, `link`, `ticket` — and they
// are genuinely different things, not options on one thing: one needs a saved
// contact, one needs the other person present *now*, one needs nothing but a
// browser, one needs no relay at all. Presenting them as a mode switch rather
// than as four buttons on the rail makes the choice visible without making it
// four separate places to learn.
//
// The mailbox options (scadenza, download, password) appear only under "a un
// contatto", and only once "lascia in casella" is on. That is not tidiness: TTL
// and password apply to a *deposit* and to nothing else, and showing them on a
// live P2P send would promise a protection that is not being applied.

import { useEffect, useMemo, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import { Icon } from "../ui/Icons";
import {
  Badge,
  Button,
  Field,
  Segmented,
  Switch,
  TextInput,
  Textarea,
  TrustBadges,
} from "../ui/Primitives";
import { Avatar, CodeHero } from "../ui/Bits";
import { Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import type { ContactDto } from "../types";

type Mode = "contact" | "code" | "link" | "ticket";

const MODES: { value: Mode; label: string }[] = [
  { value: "contact", label: "A un contatto" },
  { value: "code", label: "Codice" },
  { value: "link", label: "Link" },
  { value: "ticket", label: "Ticket" },
];

/** The blurb under the mode switch. Each says what the recipient needs, because
 *  that is the only thing that actually decides which mode is right. */
const BLURB: Record<Mode, string> = {
  contact:
    "Va dritto a chi hai in rubrica. Se è collegato passa diretto da dispositivo a dispositivo; se non lo è resta nella sua casella sul relay finché non lo ritira.",
  code:
    "Un codice corto da leggere a voce o inquadrare. Chi lo riceve lo incolla in Arvolo — non serve che sia già in rubrica, ma dovete essere entrambi collegati adesso.",
  link:
    "Un indirizzo che si apre in qualsiasi browser: chi lo riceve non ha bisogno di Arvolo né di un account. Il file viene decifrato nel browser, la chiave viaggia nel frammento dell'URL e al relay non arriva mai.",
  ticket:
    "Un ticket arvc… peer-to-peer: non passa né dalla casella né dal relay Arvolo. Per bucare il NAT può servire un relay di collegamento, che vede solo traffico cifrato.",
};

const TTL_CHOICES = [
  { secs: 3600, label: "1 ora" },
  { secs: 24 * 3600, label: "1 giorno" },
  { secs: 7 * 24 * 3600, label: "7 giorni" },
  { secs: 30 * 24 * 3600, label: "30 giorni" },
];

interface Result {
  kind: "code" | "link" | "ticket" | "deposit" | "sent";
  value: string;
  detail?: string;
}

function basename(p: string): string {
  return p.split(/[/\\]/).pop() || p;
}

function ContactPicker({
  contacts,
  value,
  onChange,
}: {
  contacts: ContactDto[];
  value: string;
  onChange: (name: string) => void;
}) {
  const [q, setQ] = useState("");
  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const usable = contacts.filter((c) => !c.blocked);
    if (!needle) return usable;
    return usable.filter(
      (c) =>
        c.name.toLowerCase().includes(needle) ||
        c.display_name.toLowerCase().includes(needle) ||
        c.id.toLowerCase().startsWith(needle)
    );
  }, [contacts, q]);

  if (!contacts.length) {
    return (
      <div className="card card-pad t-sm t-sec">
        Non hai ancora nessuno in rubrica. Aggiungi qualcuno da{" "}
        <strong>Persone</strong> — il modo più rapido è lo scambio con codice,
        che vi salva a vicenda già verificati.
      </div>
    );
  }

  return (
    <div className="stack-sm">
      <TextInput
        placeholder="Cerca un contatto…"
        value={q}
        onChange={(e) => setQ(e.currentTarget.value)}
        aria-label="Cerca un contatto"
      />
      <div
        className="card rows"
        style={{ maxHeight: 232, overflowY: "auto" }}
        role="radiogroup"
        aria-label="Destinatario"
      >
        {shown.length === 0 && (
          <div className="t-sm t-mut" style={{ padding: 14 }}>
            Nessun contatto corrisponde a «{q}».
          </div>
        )}
        {shown.map((c) => (
          <button
            key={c.name}
            role="radio"
            aria-checked={value === c.name}
            onClick={() => onChange(c.name)}
            className="row"
            style={{
              gridTemplateColumns: "32px 1fr auto",
              border: 0,
              width: "100%",
              textAlign: "left",
              cursor: "pointer",
              background:
                value === c.name ? "var(--out-soft)" : "var(--surface)",
            }}
          >
            <Avatar name={c.display_name || c.name} id={c.id} size={32} />
            <span className="row-main">
              <span className="row-name truncate">{c.name}</span>
              <span className="row-meta mono" style={{ fontSize: 10.5 }}>
                {c.fingerprint}
              </span>
            </span>
            <span className="hstack-sm">
              <TrustBadges verified={c.verified} trusted={c.trusted} />
              {value === c.name && <Icon.Check size={14} className="tone-out" />}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

export function SendSheet() {
  const paths = useStore((s) => s.sheetPaths);
  const presetTo = useStore((s) => s.sheetTo);
  const close = useStore((s) => s.closeSheet);
  const contacts = useStore((s) => s.contacts);
  const send = useStore((s) => s.send);
  const depositAction = useStore((s) => s.deposit);
  const ticket = useStore((s) => s.ticket);
  const code = useStore((s) => s.code);
  const link = useStore((s) => s.link);
  const relay = useStore((s) => s.status?.relay ?? null);

  const [mode, setMode] = useState<Mode>("contact");
  const [files, setFiles] = useState<string[]>([]);
  const [to, setTo] = useState("");
  const [note, setNote] = useState("");
  const [asDeposit, setAsDeposit] = useState(false);
  const [ttl, setTtl] = useState(7 * 24 * 3600);
  const [maxDl, setMaxDl] = useState("1");
  const [password, setPassword] = useState("");
  const [keepCode, setKeepCode] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<Result | null>(null);

  const open = paths !== null;

  // Reopening must not inherit the last send's answers — least of all its
  // password, which would silently protect a different file for a different
  // person. Everything resets except the files the sheet was opened with.
  useEffect(() => {
    if (!open) return;
    setFiles(paths ?? []);
    setTo(presetTo ?? "");
    setResult(null);
    setBusy(false);
    setNote("");
    setPassword("");
    setAsDeposit(false);
    setKeepCode(false);
    setMaxDl("1");
    setTtl(7 * 24 * 3600);
  }, [open, paths, presetTo]);

  const pick = async (directory: boolean) => {
    const picked = await openDialog({ multiple: true, directory });
    if (!picked) return;
    const list = Array.isArray(picked) ? picked : [picked];
    setFiles((f) => Array.from(new Set([...f, ...list])));
  };

  const canSubmit =
    files.length > 0 && (mode !== "contact" || to !== "") && !busy;
  // A link is deposited from a single payload; the daemon's CreateLink takes one
  // path. Packing several into an archive first is a send-side job the link path
  // does not do, so the sheet says so rather than silently sending only the first.
  const linkTooMany = mode === "link" && files.length > 1;

  const submit = async () => {
    setBusy(true);
    try {
      if (mode === "contact") {
        if (asDeposit) {
          const max = maxDl.trim() === "" ? null : Number(maxDl);
          const r = await depositAction(
            to,
            files,
            note,
            ttl,
            Number.isFinite(max as number) && max !== null ? max : null,
            password || null
          );
          setResult({
            kind: "deposit",
            value: r.ticket,
            detail: `Depositato per ${to}. Il ticket qui sotto è la tua copia: serve solo se vuoi consegnarlo tu, per esempio se ${to} non riceve l'offerta.`,
          });
        } else {
          await send(to, files, note);
          toast.ok(
            `In consegna a ${to}`,
            "Se è online passa diretto, altrimenti resta nella sua casella."
          );
          close();
        }
      } else if (mode === "code") {
        const r = await code(files, keepCode);
        setResult({
          kind: "code",
          value: r.code,
          detail: keepCode
            ? "Il codice resta valido per più destinatari finché non annulli l'invio."
            : "Il codice vale per un solo destinatario e poi si ritira da solo.",
        });
      } else if (mode === "link") {
        const max = maxDl.trim() === "" ? null : Number(maxDl);
        const url = await link(
          files[0],
          ttl,
          Number.isFinite(max as number) && max !== null ? max : null
        );
        setResult({
          kind: "link",
          value: url,
          detail:
            "Chiunque abbia questo indirizzo può scaricare il file finché non scade, non esaurisce i download consentiti o non lo revochi da «Link e depositi».",
        });
      } else {
        const r = await ticket(files);
        setResult({
          kind: "ticket",
          value: r.ticket,
          detail:
            "Ticket peer-to-peer: resta valido finché il daemon è in esecuzione e l'invio non viene annullato.",
        });
      }
    } catch {
      // `act` in the store already recorded the reason and raised the toast.
    } finally {
      setBusy(false);
    }
  };

  const totalNote =
    files.length === 0
      ? null
      : `${files.length} element${files.length === 1 ? "o" : "i"}${
          files.length > 1 ? " · verranno impacchettati in un archivio" : ""
        }`;

  return (
    <Sheet
      open={open}
      onClose={close}
      title={result ? "Pronto" : "Invia"}
      subtitle={
        result
          ? "Consegna quello che vedi qui sotto."
          : "Cifrato end-to-end, sempre."
      }
      footer={
        result ? (
          <>
            <div className="spacer" />
            <Button variant="primary" onClick={close}>
              Fatto
            </Button>
          </>
        ) : (
          <>
            <span className="t-xs t-mut truncate">{totalNote}</span>
            <div className="spacer" />
            <Button onClick={close} disabled={busy}>
              Annulla
            </Button>
            <Button
              variant="primary"
              onClick={submit}
              busy={busy}
              disabled={!canSubmit || linkTooMany}
            >
              <Icon.Send size={14} />
              {mode === "contact"
                ? asDeposit
                  ? "Deposita"
                  : "Invia"
                : mode === "code"
                  ? "Genera il codice"
                  : mode === "link"
                    ? "Crea il link"
                    : "Crea il ticket"}
            </Button>
          </>
        )
      }
    >
      {result ? (
        <div className="stack">
          {result.kind === "sent" ? null : (
            <CodeHero
              value={result.value}
              small={result.kind !== "code"}
              caption={result.detail}
            />
          )}
          {result.kind === "link" && (
            <div className="t-sm t-sec">
              Il link contiene la chiave dopo il <code className="mono">#</code>:
              i browser non inviano quella parte al server, quindi il relay
              conserva solo byte che non sa leggere.
            </div>
          )}
        </div>
      ) : (
        <div className="stack">
          {/* --- files ------------------------------------------------- */}
          <Field
            label="Cosa mandi"
            hint="Puoi anche trascinare file e cartelle nella finestra."
          >
            {() => (
              <div className="stack-sm">
                {files.length > 0 && (
                  <div className="card rows" style={{ maxHeight: 160, overflowY: "auto" }}>
                    {files.map((p) => (
                      <div
                        key={p}
                        className="hstack"
                        style={{ padding: "8px 11px" }}
                      >
                        <Icon.Folder size={14} className="t-mut" />
                        <span className="grow truncate t-sm" title={p}>
                          {basename(p)}
                        </span>
                        <button
                          className="icon-btn"
                          aria-label={`Togli ${basename(p)}`}
                          onClick={() =>
                            setFiles((f) => f.filter((x) => x !== p))
                          }
                        >
                          <Icon.Close size={13} />
                        </button>
                      </div>
                    ))}
                  </div>
                )}
                <div className="hstack-sm">
                  <Button size="sm" onClick={() => pick(false)}>
                    <Icon.Plus size={13} /> File…
                  </Button>
                  <Button size="sm" onClick={() => pick(true)}>
                    <Icon.Folder size={13} /> Cartella…
                  </Button>
                </div>
              </div>
            )}
          </Field>

          {/* --- mode -------------------------------------------------- */}
          <Field label="A chi va">
            {() => (
              <div className="stack-sm">
                <Segmented
                  block
                  label="Modo di invio"
                  value={mode}
                  onChange={setMode}
                  options={MODES}
                />
                <div className="hint">{BLURB[mode]}</div>
              </div>
            )}
          </Field>

          {mode === "contact" && (
            <ContactPicker contacts={contacts} value={to} onChange={setTo} />
          )}

          {(mode === "contact" || mode === "code") && (
            <Field
              label="Due righe per chi riceve (facoltativo)"
              hint="Viaggia dentro l'offerta sigillata: il relay non la vede."
            >
              {({ id, describedBy }) => (
                <Textarea
                  id={id}
                  aria-describedby={describedBy}
                  value={note}
                  maxLength={280}
                  onChange={(e) => setNote(e.currentTarget.value)}
                  placeholder="Ecco i file di cui parlavamo."
                />
              )}
            </Field>
          )}

          {mode === "code" && (
            <div className="switch-row">
              <div className="grow">
                <div style={{ fontWeight: 570 }}>Vale per più persone</div>
                <div className="hint">
                  Di norma il codice vale per un solo destinatario e poi si
                  ritira. Attivalo per lasciarlo aperto finché non annulli l'invio.
                </div>
              </div>
              <Switch
                checked={keepCode}
                onChange={setKeepCode}
                label="Codice valido per più persone"
              />
            </div>
          )}

          {mode === "contact" && (
            <>
              <div className="switch-row">
                <div className="grow">
                  <div style={{ fontWeight: 570 }}>
                    Lascia in casella, non aspettare
                  </div>
                  <div className="hint">
                    Deposita subito sul relay anche se è collegato: tu chiudi e te ne
                    dimentichi. Sblocca scadenza, numero di ritiri e password.
                  </div>
                </div>
                <Switch
                  checked={asDeposit}
                  onChange={setAsDeposit}
                  label="Lascia in casella"
                />
              </div>

              {asDeposit && (
                <div
                  className="stack"
                  style={{
                    padding: 14,
                    borderRadius: "var(--r-md)",
                    background: "var(--surface-2)",
                    border: "1px solid var(--line)",
                  }}
                >
                  <Field label="Scade dopo">
                    {() => (
                      <Segmented
                        block
                        label="Scadenza del deposito"
                        value={String(ttl)}
                        onChange={(v) => setTtl(Number(v))}
                        options={TTL_CHOICES.map((c) => ({
                          value: String(c.secs),
                          label: c.label,
                        }))}
                      />
                    )}
                  </Field>
                  <Field
                    label="Ritiri consentiti"
                    hint="Di norma uno solo: appena lo scarica, il relay lo cancella."
                  >
                    {({ id, describedBy }) => (
                      <TextInput
                        id={id}
                        aria-describedby={describedBy}
                        className="tnum"
                        inputMode="numeric"
                        value={maxDl}
                        onChange={(e) => setMaxDl(e.currentTarget.value)}
                      />
                    )}
                  </Field>
                  <Field
                    label="Password (facoltativa)"
                    hint="Cifra il deposito anche per il destinatario: senza questa password non si apre. Il relay non la conosce e non può recuperarla — se la perdi, il file è perso."
                  >
                    {({ id, describedBy }) => (
                      <TextInput
                        id={id}
                        aria-describedby={describedBy}
                        type="password"
                        autoComplete="new-password"
                        value={password}
                        onChange={(e) => setPassword(e.currentTarget.value)}
                        placeholder="nessuna"
                      />
                    )}
                  </Field>
                </div>
              )}
            </>
          )}

          {mode === "link" && (
            <div className="stack">
              {linkTooMany && (
                <div className="card card-pad t-sm" style={{ borderColor: "var(--amber)" }}>
                  Un link pubblica un solo elemento. Scegline uno, oppure metti
                  tutto in una cartella e seleziona quella.
                </div>
              )}
              <Field label="Scade dopo">
                {() => (
                  <Segmented
                    block
                    label="Scadenza del link"
                    value={String(ttl)}
                    onChange={(v) => setTtl(Number(v))}
                    options={TTL_CHOICES.map((c) => ({
                      value: String(c.secs),
                      label: c.label,
                    }))}
                  />
                )}
              </Field>
              <Field
                label="Download consentiti"
                hint="Lascia vuoto per non mettere limiti."
              >
                {({ id, describedBy }) => (
                  <TextInput
                    id={id}
                    aria-describedby={describedBy}
                    className="tnum"
                    inputMode="numeric"
                    value={maxDl}
                    onChange={(e) => setMaxDl(e.currentTarget.value)}
                    placeholder="illimitati"
                  />
                )}
              </Field>
            </div>
          )}

          {(mode === "code" || mode === "link") && !relay && (
            <div className="card card-pad t-sm" style={{ borderColor: "var(--red)" }}>
              Serve un relay per questa modalità e non ne risulta configurato
              nessuno. Impostane uno da <strong>Impostazioni</strong>.
            </div>
          )}

          {mode === "ticket" && (
            <div className="hstack-sm">
              <Badge kind="info">
                <Icon.Lock size={10} /> Nessun relay Arvolo
              </Badge>
            </div>
          )}
        </div>
      )}
    </Sheet>
  );
}

