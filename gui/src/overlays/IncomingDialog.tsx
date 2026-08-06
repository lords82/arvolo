// The one screen in the app where a security decision is actually made.
//
// Someone you may or may not know is offering you a file. Everything shown here
// exists to make one question answerable: *is this who I think it is?* So the
// dialog leads with identity, not with the filename — and it is careful about
// three distinctions the interface must never blur:
//
//   * A **display name** is a claim the sender types about themselves. It is not
//     authenticated by anything. It is shown in quotes, and never in the place a
//     saved contact's name would go.
//   * A **saved contact name** is your own label for a key you chose to keep.
//   * A **verified** mark means you compared the fingerprint out of band. Only
//     that one is evidence.
//
// An unknown sender therefore gets their fingerprint shown in full, not tucked
// behind a disclosure: it is the only handle on them that means anything, and
// deciding without it is deciding blind.

import { useMemo, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import { fmtBytes } from "../format";
import { Icon } from "../ui/Icons";
import { Badge, Button, Field, TextInput, TrustBadges } from "../ui/Primitives";
import { Avatar, ExtChip, Fingerprint } from "../ui/Bits";
import { Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";

export function IncomingDialog() {
  const offerId = useStore((s) => s.incomingOfferId);
  const close = useStore((s) => s.closeIncoming);
  const transfers = useStore((s) => s.transfers);
  const contactsById = useStore((s) => s.contactsById);
  const accept = useStore((s) => s.accept);
  const reject = useStore((s) => s.reject);
  const addContact = useStore((s) => s.addContact);
  const blockContact = useStore((s) => s.blockContact);
  const downloadDir = useStore((s) => s.status?.download_dir ?? "");

  const [out, setOut] = useState<string | null>(null);
  const [busy, setBusy] = useState<"accept" | "reject" | null>(null);
  const [saveAs, setSaveAs] = useState("");
  const [saving, setSaving] = useState(false);

  const t = offerId ? transfers[`o${offerId}`] : undefined;
  const contact = useMemo(
    () => (t?.peerId ? contactsById[t.peerId] : undefined),
    [t?.peerId, contactsById]
  );

  if (!t || !offerId) return null;

  const known = !!contact;
  const claimed = t.senderName?.trim();
  const label = contact?.name ?? (claimed ? `“${claimed}”` : "Mittente sconosciuto");

  const doAccept = async () => {
    setBusy("accept");
    try {
      await accept(offerId, out);
      toast.ok("Download avviato", t.name);
    } catch {
      // reported by the store
    } finally {
      setBusy(null);
    }
  };

  const doReject = async () => {
    setBusy("reject");
    try {
      await reject(offerId);
    } catch {
      // reported by the store
    } finally {
      setBusy(null);
    }
  };

  return (
    <Sheet
      open
      onClose={close}
      placement="center"
      title="Ti stanno mandando un file"
      subtitle="Accetta solo se sai da chi arriva."
      footer={
        <>
          <Button
            variant="danger"
            onClick={doReject}
            busy={busy === "reject"}
            disabled={busy !== null}
          >
            Rifiuta
          </Button>
          <div className="spacer" />
          <Button onClick={close} disabled={busy !== null}>
            Decido dopo
          </Button>
          <Button
            variant="in"
            onClick={doAccept}
            busy={busy === "accept"}
            disabled={busy !== null}
            data-autofocus
          >
            <Icon.Receive size={14} />
            Accetta e scarica
          </Button>
        </>
      }
    >
      {/* --- who ------------------------------------------------------- */}
      <div
        className="card card-pad"
        style={{
          borderColor: known
            ? contact!.verified
              ? "var(--green)"
              : "var(--line-strong)"
            : "var(--amber)",
        }}
      >
        <div className="hstack">
          <Avatar name={label} id={t.peerId} size={40} ring="in" />
          <div className="grow">
            <div className="hstack-sm wrap">
              <span className="t-head truncate">{label}</span>
              {known ? (
                <TrustBadges
                  verified={contact!.verified}
                  trusted={contact!.trusted}
                  blocked={contact!.blocked}
                />
              ) : (
                <Badge kind="warn">
                  <Icon.Alert size={10} /> Non in rubrica
                </Badge>
              )}
            </div>
            {known && claimed && claimed !== contact!.name && (
              <div className="t-xs t-mut" style={{ marginTop: 2 }}>
                si presenta come “{claimed}” — è un nome che sceglie da sé,
                niente lo garantisce
              </div>
            )}
          </div>
        </div>

        <div className="divider" style={{ margin: "12px 0" }} />

        <div className="stack-sm">
          <div className="t-label">Impronta della chiave</div>
          <Fingerprint value={t.peerId ? fingerprintOf(contact, t.peerId) : ""} />
          <div className="hint">
            {known && contact!.verified
              ? "Hai già confermato questa impronta fuori banda. È lo stesso interlocutore di sempre."
              : "Confrontala a voce con chi ti sta mandando il file. È l'unico modo per essere certi che sia davvero lui — un nome non lo dimostra."}
          </div>
        </div>
      </div>

      {/* --- what ------------------------------------------------------ */}
      <div className="card rows">
        <div className="row" style={{ gridTemplateColumns: "38px 1fr auto" }}>
          <ExtChip name={t.name} />
          <div className="row-main">
            <div className="row-name truncate" title={t.name}>
              {t.name}
            </div>
            <div className="row-meta">
              <span className="tnum">{fmtBytes(t.size)}</span>
            </div>
          </div>
        </div>
        {t.note && (
          <div style={{ padding: "11px 14px" }}>
            <div className="t-label" style={{ marginBottom: 4 }}>
              Messaggio allegato
            </div>
            <div className="t-sm selectable" style={{ whiteSpace: "pre-wrap" }}>
              {t.note}
            </div>
          </div>
        )}
      </div>

      {/* --- where ----------------------------------------------------- */}
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
          </div>
        )}
      </Field>

      {/* --- act on the sender ----------------------------------------- */}
      {!known && (
        <div className="card card-pad stack-sm">
          <div className="t-label">Se lo conosci</div>
          <div className="hstack-sm">
            <TextInput
              value={saveAs}
              onChange={(e) => setSaveAs(e.currentTarget.value)}
              placeholder="Salvalo in rubrica come…"
              aria-label="Nome da dare al contatto"
            />
            <Button
              size="sm"
              disabled={!saveAs.trim() || saving}
              busy={saving}
              onClick={async () => {
                setSaving(true);
                try {
                  await addContact(saveAs.trim(), t.peerId!);
                  toast.ok(
                    `Salvato come ${saveAs.trim()}`,
                    "Resta non verificato: conferma l'impronta a voce e poi segnalo da Persone."
                  );
                  setSaveAs("");
                } catch {
                  // reported by the store
                } finally {
                  setSaving(false);
                }
              }}
            >
              Salva
            </Button>
          </div>
          <div className="hint">
            Salvarlo non lo verifica. Diventa verificato solo quando confronti
            l'impronta di persona o a voce.
          </div>
          <div>
            <Button
              size="sm"
              variant="danger"
              onClick={async () => {
                await blockContact(t.peerId!);
                await reject(offerId);
                toast.ok(
                  "Bloccato",
                  "Le sue offerte verranno scartate all'arrivo, senza avvisarti."
                );
              }}
            >
              <Icon.Ban size={13} /> Blocca e rifiuta
            </Button>
          </div>
        </div>
      )}
    </Sheet>
  );
}

/** The contact's stored fingerprint when we have one, else nothing — the id
 *  itself is not a fingerprint and showing it in that slot would invite the two
 *  to be compared as if they were the same thing. */
function fingerprintOf(
  contact: { fingerprint: string } | undefined,
  id: string
): string {
  return contact?.fingerprint || id;
}
