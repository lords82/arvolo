// Receiving something that was handed to you rather than sent to your identity:
// a pairing code read out over the phone, an `arvc…` ticket, an `arvm…` mailbox
// ticket.
//
// One field, because the daemon sorts them out itself — it recognises a code, an
// `arvc…` and an `arvm…` and does the right thing with each. Asking the user to
// classify a string they were just given, in an app that can classify it
// perfectly well, would be making them do the computer's job. What the sheet
// *does* do is recognise the shape well enough to say what it is about to try,
// so the button is never a leap of faith.

import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import { Icon } from "../ui/Icons";
import { Button, Field, TextInput, Textarea } from "../ui/Primitives";
import { Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";

type Shape = "code" | "chunk" | "mailbox" | "unknown" | "empty";

/** What the pasted string looks like. Mirrors the daemon's own sorting in
 *  `recv_ticket`: a pairing code is `dddd-word-word[@relay]`, and the two ticket
 *  families announce themselves with a prefix. Anything else is left as
 *  "unknown" — the daemon gets the final say, this is only what we tell the user
 *  we are about to attempt. */
export function shapeOf(raw: string): Shape {
  const s = raw.trim();
  if (!s) return "empty";
  if (s.startsWith("arvc")) return "chunk";
  if (s.startsWith("arvm")) return "mailbox";
  // 1–6 digits: the nameplate is `rng.random_range(0..10_000)`, so `7-fox-oak`
  // is a perfectly ordinary code. Demanding three digits met one code in a
  // hundred with "non riconosco questa forma".
  if (/^\d{1,6}-[a-z]+-[a-z]+(@.+)?$/i.test(s)) return "code";
  return "unknown";
}

const EXPLAIN: Record<Shape, string> = {
  empty:
    "Incolla un codice di invio (tipo 4821-crater-mango) oppure un ticket arvc… / arvm…. Per scambiare i contatti con qualcuno usa invece Persone → Ho un codice.",
  code: "Codice di invio: mi collego a chi lo sta mostrando adesso e scarico quello che manda.",
  chunk: "Ticket peer-to-peer: scarico direttamente dal mittente.",
  mailbox: "Ticket di casella: recupero il file depositato sul relay.",
  unknown:
    "Non riconosco questa forma. La provo lo stesso — il daemon è più preciso di me — ma controlla di averla copiata per intero.",
};

export function ReceiveSheet() {
  const open = useStore((s) => s.receiveOpen);
  const close = useStore((s) => s.closeReceive);
  const receive = useStore((s) => s.receive);
  const downloadDir = useStore((s) => s.status?.download_dir ?? "");

  const [value, setValue] = useState("");
  const [out, setOut] = useState<string | null>(null);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setValue("");
    setOut(null);
    setPassword("");
    setBusy(false);
  }, [open]);

  const shape = shapeOf(value);
  // Only a mailbox ticket can be password-protected; offering the field for a
  // pairing code would suggest codes have passwords, which they do not.
  const mayNeedPassword = shape === "mailbox" || shape === "unknown";

  const submit = async () => {
    setBusy(true);
    try {
      await receive(value.trim(), out, password || null);
      toast.ok("Ricezione avviata", "La trovi fra i trasferimenti in arrivo.");
    } catch {
      // The store's `act` already reported it.
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      open={open}
      onClose={close}
      title="Ricevi"
      subtitle="Incolla quello che ti hanno dato."
      footer={
        <>
          <div className="spacer" />
          <Button onClick={close} disabled={busy}>
            Annulla
          </Button>
          <Button
            variant="in"
            onClick={submit}
            busy={busy}
            disabled={!value.trim() || busy}
          >
            <Icon.Receive size={14} />
            Ricevi
          </Button>
        </>
      }
    >
      <Field label="Codice o ticket" hint={EXPLAIN[shape]}>
        {({ id, describedBy }) => (
          <Textarea
            id={id}
            data-autofocus
            aria-describedby={describedBy}
            className="mono"
            style={{ minHeight: 84 }}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            value={value}
            onChange={(e) => setValue(e.currentTarget.value)}
            placeholder="4821-crater-mango"
          />
        )}
      </Field>

      {mayNeedPassword && (
        <Field
          label="Password (solo se protetto)"
          hint="Chi te l'ha mandato te l'avrà detta a parte. Senza, un deposito protetto non si apre."
        >
          {({ id, describedBy }) => (
            <TextInput
              id={id}
              aria-describedby={describedBy}
              type="password"
              autoComplete="off"
              value={password}
              onChange={(e) => setPassword(e.currentTarget.value)}
              placeholder="nessuna"
            />
          )}
        </Field>
      )}

      <Field
        label="Dove salvarlo"
        hint={out ? undefined : `Cartella predefinita: ${downloadDir || "…"}`}
      >
        {() => (
          <div className="hstack-sm">
            <TextInput
              readOnly
              value={out ?? ""}
              placeholder={downloadDir}
              aria-label="Cartella di destinazione"
            />
            <Button
              size="sm"
              onClick={async () => {
                const picked = await openDialog({ directory: true });
                if (typeof picked === "string") setOut(picked);
              }}
            >
              <Icon.Folder size={13} /> Scegli…
            </Button>
            {out && (
              <Button size="sm" variant="ghost" onClick={() => setOut(null)}>
                Predefinita
              </Button>
            )}
          </div>
        )}
      </Field>
    </Sheet>
  );
}
