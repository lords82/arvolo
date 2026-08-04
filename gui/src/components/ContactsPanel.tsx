// The address book, in full — the GUI counterpart of `arvolo contacts` (and of
// `arvolo me name` for one's own advertised name). Everything the CLI can do to
// a contact is here: add, rename, remove, verify/unverify, trust/untrust,
// block/unblock, approve a pending advertised name.
//
// The one deliberately *ceremonial* flow is *verifica*: marking a key verified
// means "I compared this fingerprint out-of-band", so the fingerprint is shown
// and must be confirmed — a one-click verify with the fingerprint hidden would
// make the ✓ badge a lie. Same reason the CLI's `contacts verify` prompts.

import { useEffect, useState } from "react";
import { useStore } from "../store";
import { shortId } from "../format";
import type { ContactDto } from "../types";

const AVATAR_COLORS = ["#c2410c", "#0369a1", "#7c3aed", "#be185d", "#0f766e", "#4b5563"];
function colorFor(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  return AVATAR_COLORS[h % AVATAR_COLORS.length];
}
function initials(name: string): string {
  const parts = name.trim().split(/\s+/);
  return ((parts[0]?.[0] ?? "?") + (parts[1]?.[0] ?? "")).toUpperCase();
}

/** Which secondary flow a row has open (mutually exclusive on purpose: two open
 *  confirmations on one row is how missclicks happen). */
type RowFlow =
  | { kind: "verify" }
  | { kind: "trust-force"; error: string }
  | { kind: "rename"; value: string }
  | { kind: "remove" }
  | null;

export function ContactsPanel() {
  const isOpen = useStore((s) => s.contactsOpen);
  const close = useStore((s) => s.closeContacts);
  const contacts = useStore((s) => s.contacts);
  const status = useStore((s) => s.status);

  const [adding, setAdding] = useState(false);

  useEffect(() => {
    if (isOpen) setAdding(false);
  }, [isOpen]);

  if (!isOpen) return null;

  return (
    <div
      onClick={(e) => {
        if (e.target === e.currentTarget) close();
      }}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(20,16,12,.4)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        backdropFilter: "blur(2px)",
        zIndex: 100,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 600,
          maxHeight: 640,
          background: "#fff",
          borderRadius: 18,
          boxShadow: "0 30px 70px -12px rgba(0,0,0,.45)",
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          animation: "pop .14s ease",
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 12,
            padding: "18px 20px",
            borderBottom: "1px solid var(--line)",
          }}
        >
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 15, fontWeight: 700 }}>Rubrica</div>
            <div style={{ fontSize: 11.5, color: "#a8a29a" }}>
              Le persone a cui invii, e come ti fidi di loro
            </div>
          </div>
          <button
            onClick={() => setAdding((v) => !v)}
            style={{
              border: "none",
              background: adding ? "#f4f1ee" : "#171514",
              color: adding ? "#57534c" : "#fff",
              borderRadius: 9,
              padding: "8px 14px",
              fontSize: 12,
              fontWeight: 600,
              cursor: "pointer",
            }}
          >
            {adding ? "Annulla" : "+ Aggiungi"}
          </button>
          <button
            onClick={close}
            aria-label="Chiudi"
            style={{
              width: 30,
              height: 30,
              border: "none",
              background: "#f4f1ee",
              borderRadius: 8,
              cursor: "pointer",
              fontSize: 14,
            }}
          >
            ✕
          </button>
        </div>

        <div style={{ flex: 1, overflowY: "auto", padding: "14px 18px 18px" }}>
          {adding && <AddContactForm done={() => setAdding(false)} />}

          <MeCard
            myId={status?.public_id ?? ""}
            fingerprint={status?.fingerprint ?? ""}
            displayName={status?.display_name ?? ""}
          />

          {contacts.length === 0 ? (
            <div style={{ fontSize: 12.5, color: "#a8a29a", padding: "18px 4px" }}>
              Nessun contatto. Aggiungine uno con “+ Aggiungi”: ti serve il suo ID
              pubblico (chiediglielo — lo trova in alto nella sua app o con{" "}
              <span className="mono">arvolo me</span>).
            </div>
          ) : (
            contacts.map((c) => <ContactRow key={c.id + c.name} c={c} />)
          )}
        </div>
      </div>
    </div>
  );
}

/** Own identity: id, fingerprint (what others verify against), advertised name. */
function MeCard({
  myId,
  fingerprint,
  displayName,
}: {
  myId: string;
  fingerprint: string;
  displayName: string;
}) {
  const setMyName = useStore((s) => s.setMyName);
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(displayName);
  useEffect(() => setName(displayName), [displayName]);

  return (
    <div
      style={{
        border: "1px solid var(--line)",
        background: "#faf8f6",
        borderRadius: 12,
        padding: "12px 14px",
        marginBottom: 14,
      }}
    >
      <div style={{ fontSize: 10, fontWeight: 600, letterSpacing: ".06em", textTransform: "uppercase", color: "#a8a29a", marginBottom: 6 }}>
        Tu
      </div>
      <div className="mono selectable" style={{ fontSize: 11, color: "#57534c", wordBreak: "break-all" }}>
        {myId}
      </div>
      <div style={{ fontSize: 11.5, color: "#57534c", marginTop: 6 }}>
        Fingerprint: <span className="mono selectable">{fingerprint}</span>
        <span style={{ color: "#a8a29a" }}> — è quello che gli altri confrontano per verificarti</span>
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 8 }}>
        <span style={{ fontSize: 11.5, color: "#57534c" }}>Nome visibile:</span>
        {editing ? (
          <>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="(nessuno)"
              style={{
                flex: 1,
                border: "1px solid var(--line-strong)",
                borderRadius: 7,
                padding: "5px 8px",
                fontSize: 11.5,
                outline: "none",
              }}
            />
            <button
              onClick={() => {
                void setMyName(name).catch(() => {});
                setEditing(false);
              }}
              style={smallBtn("#171514", "#fff")}
            >
              Salva
            </button>
          </>
        ) : (
          <>
            <span style={{ fontSize: 11.5, fontWeight: 600 }}>
              {displayName || "(nessuno)"}
            </span>
            <button onClick={() => setEditing(true)} style={smallBtn("#fff", "#171514", true)}>
              Cambia
            </button>
          </>
        )}
      </div>
      <div style={{ fontSize: 10.5, color: "#a8a29a", marginTop: 4 }}>
        Il nome viaggia cifrato dentro le offerte che invii; chi lo riceve deve
        comunque approvarlo.
      </div>
    </div>
  );
}

function AddContactForm({ done }: { done: () => void }) {
  const contacts = useStore((s) => s.contacts);
  const addContact = useStore((s) => s.addContact);
  const [name, setName] = useState("");
  const [id, setId] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  // Re-keying an existing name silently clears its verified/trusted marks in the
  // book — surface that BEFORE the daemon does it, since only the UI can ask.
  const existing = contacts.find((c) => c.name === name.trim());
  const rekeying = !!existing && !!id.trim() && existing.id !== id.trim().toLowerCase();

  const save = async () => {
    setBusy(true);
    setErr("");
    try {
      await addContact(name.trim(), id.trim());
      done();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      style={{
        border: "1px solid var(--line)",
        borderRadius: 12,
        padding: "12px 14px",
        marginBottom: 14,
      }}
    >
      <div style={{ fontSize: 12.5, fontWeight: 600, marginBottom: 10 }}>Nuovo contatto</div>
      {err && (
        <div className="selectable" style={{ background: "#fdecec", color: "#b91c1c", borderRadius: 8, padding: "7px 10px", fontSize: 11.5, marginBottom: 8 }}>
          {err}
        </div>
      )}
      <input
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Nome (come vuoi chiamarlo tu)"
        style={{
          width: "100%",
          border: "1px solid var(--line-strong)",
          borderRadius: 8,
          padding: "9px 11px",
          fontSize: 12,
          marginBottom: 8,
          outline: "none",
        }}
      />
      <input
        value={id}
        onChange={(e) => setId(e.target.value)}
        placeholder="ID pubblico (if2xmne…)"
        className="mono"
        style={{
          width: "100%",
          border: "1px solid var(--line-strong)",
          borderRadius: 8,
          padding: "9px 11px",
          fontSize: 11.5,
          marginBottom: 8,
          outline: "none",
        }}
      />
      {rekeying && (
        <div style={{ fontSize: 11, color: "#b45309", marginBottom: 8 }}>
          ⚠ “{existing!.name}” esiste già con un'altra chiave. Salvando la
          sostituisci e i suoi segni <b>verificato</b> e <b>fidato</b> vengono
          azzerati (va ri-verificata).
        </div>
      )}
      <button
        disabled={busy || !name.trim() || !id.trim()}
        onClick={() => void save()}
        style={{
          border: "none",
          background: name.trim() && id.trim() ? "#171514" : "#e2ddd6",
          color: "#fff",
          borderRadius: 8,
          padding: "9px 16px",
          fontSize: 12,
          fontWeight: 600,
          cursor: name.trim() && id.trim() ? "pointer" : "default",
        }}
      >
        {busy ? "Salvo…" : rekeying ? "Sostituisci la chiave" : "Salva"}
      </button>
    </div>
  );
}

function ContactRow({ c }: { c: ContactDto }) {
  const store = useStore();
  const [flow, setFlow] = useState<RowFlow>(null);
  const [busy, setBusy] = useState(false);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    try {
      await fn();
      setFlow(null);
    } catch {
      // The store's act() already surfaced the error in the top banner.
    } finally {
      setBusy(false);
    }
  };

  const trust = () =>
    run(async () => {
      try {
        await store.markTrusted(c.name, false);
      } catch (e) {
        // The daemon refuses to trust an unverified key. Not the end of the
        // conversation: show why, and offer the same --force the CLI has.
        setFlow({ kind: "trust-force", error: String(e) });
        throw e;
      }
    });

  return (
    <div
      style={{
        border: "1px solid var(--line)",
        borderRadius: 12,
        padding: "11px 13px",
        marginBottom: 8,
        opacity: c.blocked ? 0.7 : 1,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 11 }}>
        <div
          className="avatar"
          style={{ width: 38, height: 38, background: colorFor(c.name), fontSize: 13, flex: "none" }}
        >
          {initials(c.name)}
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
            <span style={{ fontSize: 13, fontWeight: 600 }}>{c.name}</span>
            {c.display_name && c.display_name !== c.name && (
              <span style={{ fontSize: 11, color: "#a8a29a" }}>“{c.display_name}”</span>
            )}
            {c.verified && <Badge bg="#e6f4ef" color="#0f766e">✓ verificato</Badge>}
            {c.trusted && <Badge bg="#eaf3ec" color="#2f7d4f">⬇ auto-download</Badge>}
            {c.blocked && <Badge bg="#fdecec" color="#b91c1c">⊘ bloccato</Badge>}
          </div>
          <div className="mono" style={{ fontSize: 10, color: "#a8a29a", marginTop: 2 }}>
            {shortId(c.id)} · {c.fingerprint}
          </div>
        </div>
      </div>

      {c.pending_name && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 8,
            background: "#fdf3e3",
            borderRadius: 8,
            padding: "7px 10px",
            marginTop: 8,
            fontSize: 11.5,
            color: "#8a5a1e",
          }}
        >
          <span style={{ flex: 1 }}>
            Si presenta come <b>“{c.pending_name}”</b> — approvi il nome?
          </span>
          <button
            disabled={busy}
            onClick={() => void run(() => store.acceptName(c.name))}
            style={smallBtn("#171514", "#fff")}
          >
            Approva
          </button>
        </div>
      )}

      {/* row actions */}
      <div style={{ display: "flex", gap: 6, marginTop: 9, flexWrap: "wrap" }}>
        {!c.verified ? (
          <button disabled={busy} onClick={() => setFlow({ kind: "verify" })} style={smallBtn("#e6f4ef", "#0f766e")}>
            Verifica…
          </button>
        ) : (
          <button
            disabled={busy}
            onClick={() => void run(() => store.markUnverified(c.name))}
            style={smallBtn("#fff", "#57534c", true)}
          >
            Togli verifica
          </button>
        )}
        {!c.trusted ? (
          <button disabled={busy} onClick={() => void trust()} style={smallBtn("#fff", "#2f7d4f", true)}>
            Fidati (auto-download)
          </button>
        ) : (
          <button
            disabled={busy}
            onClick={() => void run(() => store.markUntrusted(c.name))}
            style={smallBtn("#fff", "#57534c", true)}
          >
            Togli fiducia
          </button>
        )}
        {!c.blocked ? (
          <button disabled={busy} onClick={() => void run(() => store.blockContact(c.name))} style={smallBtn("#fff", "#b91c1c", true)}>
            Blocca
          </button>
        ) : (
          <button disabled={busy} onClick={() => void run(() => store.unblockContact(c.name))} style={smallBtn("#fff", "#57534c", true)}>
            Sblocca
          </button>
        )}
        <button
          disabled={busy}
          onClick={() => setFlow({ kind: "rename", value: c.name })}
          style={smallBtn("#fff", "#57534c", true)}
        >
          Rinomina
        </button>
        <button disabled={busy} onClick={() => setFlow({ kind: "remove" })} style={smallBtn("#fff", "#b91c1c", true)}>
          Rimuovi
        </button>
      </div>

      {/* secondary flows */}
      {flow?.kind === "verify" && (
        <FlowBox>
          <div style={{ fontSize: 11.5, color: "#3a352f", lineHeight: 1.5 }}>
            Confronta il fingerprint <b>su un altro canale</b> (a voce, una
            videochiamata, un messaggio firmato) con quello che {c.name} vede con{" "}
            <span className="mono">arvolo me</span>:
          </div>
          <div
            className="mono selectable"
            style={{
              background: "#16181d",
              color: "#34d399",
              borderRadius: 8,
              padding: "9px 11px",
              fontSize: 12,
              margin: "8px 0",
              textAlign: "center",
            }}
          >
            {c.fingerprint || "(fingerprint non disponibile)"}
          </div>
          <div style={{ display: "flex", gap: 6 }}>
            <button
              disabled={busy || !c.fingerprint}
              onClick={() => void run(() => store.markVerified(c.name))}
              style={smallBtn("#0f766e", "#fff")}
            >
              Coincide — segna verificato
            </button>
            <button disabled={busy} onClick={() => setFlow(null)} style={smallBtn("#fff", "#57534c", true)}>
              Annulla
            </button>
          </div>
        </FlowBox>
      )}

      {flow?.kind === "trust-force" && (
        <FlowBox>
          <div style={{ fontSize: 11.5, color: "#b45309", lineHeight: 1.5 }}>
            ⚠ {c.name} non è verificato: l'auto-download da una chiave mai
            confermata è un rischio MITM. Meglio verificarlo prima; puoi comunque
            forzare.
          </div>
          <div style={{ display: "flex", gap: 6, marginTop: 8 }}>
            <button
              disabled={busy}
              onClick={() => void run(() => store.markTrusted(c.name, true))}
              style={smallBtn("#b45309", "#fff")}
            >
              Forza la fiducia
            </button>
            <button disabled={busy} onClick={() => setFlow(null)} style={smallBtn("#fff", "#57534c", true)}>
              Annulla
            </button>
          </div>
        </FlowBox>
      )}

      {flow?.kind === "rename" && (
        <FlowBox>
          <div style={{ display: "flex", gap: 6 }}>
            <input
              value={flow.value}
              onChange={(e) => setFlow({ kind: "rename", value: e.target.value })}
              style={{
                flex: 1,
                border: "1px solid var(--line-strong)",
                borderRadius: 7,
                padding: "6px 9px",
                fontSize: 11.5,
                outline: "none",
              }}
            />
            <button
              disabled={busy || !flow.value.trim() || flow.value.trim() === c.name}
              onClick={() => void run(() => store.renameContact(c.name, flow.value.trim()))}
              style={smallBtn("#171514", "#fff")}
            >
              Rinomina
            </button>
            <button disabled={busy} onClick={() => setFlow(null)} style={smallBtn("#fff", "#57534c", true)}>
              Annulla
            </button>
          </div>
          <div style={{ fontSize: 10.5, color: "#a8a29a", marginTop: 6 }}>
            Rinominare mantiene chiave e segni verificato/fidato.
          </div>
        </FlowBox>
      )}

      {flow?.kind === "remove" && (
        <FlowBox>
          <div style={{ fontSize: 11.5, color: "#3a352f" }}>
            Rimuovere <b>{c.name}</b> dalla rubrica? I suoi segni
            verificato/fidato vanno persi.
          </div>
          <div style={{ display: "flex", gap: 6, marginTop: 8 }}>
            <button
              disabled={busy}
              onClick={() => void run(() => store.removeContact(c.name))}
              style={smallBtn("#b91c1c", "#fff")}
            >
              Rimuovi
            </button>
            <button disabled={busy} onClick={() => setFlow(null)} style={smallBtn("#fff", "#57534c", true)}>
              Annulla
            </button>
          </div>
        </FlowBox>
      )}
    </div>
  );
}

function FlowBox({ children }: { children: React.ReactNode }) {
  return (
    <div
      style={{
        background: "#faf8f6",
        border: "1px solid var(--line)",
        borderRadius: 10,
        padding: "10px 12px",
        marginTop: 9,
      }}
    >
      {children}
    </div>
  );
}

function Badge({ bg, color, children }: { bg: string; color: string; children: React.ReactNode }) {
  return (
    <span
      style={{
        fontSize: 9,
        fontWeight: 600,
        background: bg,
        color,
        padding: "2px 6px",
        borderRadius: 20,
      }}
    >
      {children}
    </span>
  );
}

function smallBtn(bg: string, color: string, outline = false): React.CSSProperties {
  return {
    border: outline ? "1px solid rgba(0,0,0,.14)" : "none",
    background: bg,
    color,
    borderRadius: 7,
    padding: "5px 10px",
    fontSize: 10.5,
    fontWeight: 600,
    cursor: "pointer",
  };
}
