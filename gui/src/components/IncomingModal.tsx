import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import { extOf, fmtBytes, methodMeta } from "../format";

function initials(label: string): string {
  const p = label.trim().split(/\s+/);
  return ((p[0]?.[0] ?? "?") + (p[1]?.[0] ?? "")).toUpperCase();
}

export function IncomingModal() {
  const offerId = useStore((s) => s.incomingOfferId);
  const transfers = useStore((s) => s.transfers);
  const closeIncoming = useStore((s) => s.closeIncoming);
  const accept = useStore((s) => s.accept);
  const reject = useStore((s) => s.reject);
  const defaultDir = useStore((s) => s.status?.download_dir ?? "");
  const markVerified = useStore((s) => s.markVerified);
  const contact = useStore((s) => {
    const o = s.incomingOfferId
      ? s.transfers[`o${s.incomingOfferId}`]
      : undefined;
    return o?.peerId ? s.contactsById[o.peerId] : undefined;
  });

  const blockContact = useStore((s) => s.blockContact);

  const [dest, setDest] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [verifying, setVerifying] = useState(false);
  const [blocking, setBlocking] = useState(false);

  const offer = offerId ? transfers[`o${offerId}`] : undefined;

  useEffect(() => {
    setDest(null);
    setBusy(false);
    setVerifying(false);
    setBlocking(false);
  }, [offerId]);

  if (!offerId || !offer) return null;

  const md = methodMeta(offer.method);
  const label = offer.senderName || offer.peer || "sconosciuto";

  const pickFolder = async () => {
    const sel = await open({ directory: true, multiple: false });
    if (typeof sel === "string") setDest(sel);
  };

  const doAccept = async () => {
    setBusy(true);
    try {
      await accept(offerId, dest);
    } catch {
      // Surfaced by the store's error banner; swallowed here so a daemon refusal
      // doesn't become an unhandled rejection.
    } finally {
      setBusy(false);
    }
  };

  // Block = silence THIS sender for good, and make the current offer go away with
  // it — leaving the offer parked after a block would be a question the user
  // already answered.
  const doBlock = async () => {
    if (!offer?.peerId) return;
    setBusy(true);
    try {
      await blockContact(offer.peerId);
      await reject(offerId);
    } catch {
      // Error banner already raised by the store.
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      onClick={closeIncoming}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(20,16,12,.4)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        backdropFilter: "blur(2px)",
        zIndex: 110,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 470,
          background: "#fff",
          borderRadius: 18,
          boxShadow: "0 30px 70px -12px rgba(0,0,0,.45)",
          overflow: "hidden",
          animation: "pop .14s ease",
        }}
      >
        {/* header */}
        <div
          style={{
            padding: "20px 22px 16px",
            display: "flex",
            alignItems: "center",
            gap: 14,
            borderBottom: "1px solid var(--line)",
          }}
        >
          <div
            className="avatar"
            style={{ width: 46, height: 46, background: "#0f766e", fontSize: 16 }}
          >
            {initials(label)}
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 12, color: "#a8a29a" }}>File in arrivo</div>
            <div style={{ fontSize: 15, fontWeight: 700 }}>{label}</div>
          </div>
          <span
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 4,
              fontSize: 9.5,
              fontWeight: 600,
              background: md.bg,
              color: md.color,
              padding: "3px 8px",
              borderRadius: 20,
            }}
          >
            <span>{md.glyph}</span>
            {md.label}
          </span>
        </div>

        <div style={{ padding: "18px 22px 22px" }}>
          {/* file card */}
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 12,
              padding: 12,
              border: "1px solid var(--line)",
              borderRadius: 12,
              marginBottom: 14,
            }}
          >
            <div
              className="mono"
              style={{
                width: 36,
                height: 36,
                borderRadius: 9,
                background: "#fff3e9",
                color: "#c2410c",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                fontSize: 9,
                fontWeight: 700,
              }}
            >
              {extOf(offer.name)}
            </div>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontSize: 13, fontWeight: 600 }}>{offer.name}</div>
              <div
                className="mono"
                style={{ fontSize: 11, fontWeight: 500, color: "#a8a29a" }}
              >
                {fmtBytes(offer.size)}
              </div>
            </div>
          </div>

          {offer.note && (
            <div
              style={{
                background: "#f7f4f1",
                borderRadius: 12,
                padding: "12px 14px",
                marginBottom: 14,
                borderLeft: "3px solid #d4b483",
              }}
            >
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 5,
                  fontSize: 9.5,
                  fontWeight: 600,
                  letterSpacing: ".06em",
                  textTransform: "uppercase",
                  color: "#a8a29a",
                  marginBottom: 5,
                }}
              >
                <span style={{ fontSize: 11 }}>✉</span>Messaggio del mittente
              </div>
              <div style={{ fontSize: 12.5, color: "#3a352f", lineHeight: 1.55 }}>
                {offer.note}
              </div>
            </div>
          )}

          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              fontSize: 11,
              color: "#8a827a",
              marginBottom: 12,
              flexWrap: "wrap",
            }}
          >
            {offer.verified ? (
              <span style={{ fontWeight: 600, color: "#0f766e" }}>
                ✓ Identità verificata
              </span>
            ) : (
              <span style={{ fontWeight: 600, color: "#b45309" }}>
                ⚠ Identità non verificata
              </span>
            )}
            {!offer.verified && contact && !verifying && (
              <button
                onClick={() => setVerifying(true)}
                style={{
                  border: "1px solid rgba(15,118,110,.35)",
                  background: "#e6f4ef",
                  color: "#0f766e",
                  borderRadius: 7,
                  padding: "3px 8px",
                  fontSize: 10.5,
                  fontWeight: 600,
                  cursor: "pointer",
                }}
              >
                Verifica identità…
              </button>
            )}
            {offer.peerId && !blocking && (
              <button
                onClick={() => setBlocking(true)}
                title="Le sue offerte non ti raggiungeranno più"
                style={{
                  border: "1px solid rgba(185,28,28,.3)",
                  background: "#fff",
                  color: "#b91c1c",
                  borderRadius: 7,
                  padding: "3px 8px",
                  fontSize: 10.5,
                  fontWeight: 600,
                  cursor: "pointer",
                  marginLeft: "auto",
                }}
              >
                Blocca mittente
              </button>
            )}
          </div>

          {/* Verify = "I compared this fingerprint out-of-band" — so it is on
              screen, and the click confirms the comparison, not a wish. */}
          {verifying && contact && (
            <div
              style={{
                background: "#faf8f6",
                border: "1px solid var(--line)",
                borderRadius: 10,
                padding: "10px 12px",
                marginBottom: 12,
              }}
            >
              <div style={{ fontSize: 11.5, color: "#3a352f", lineHeight: 1.5 }}>
                Confronta il fingerprint di <b>{contact.name}</b> su un altro
                canale (a voce, in videochiamata):
              </div>
              <div
                className="mono selectable"
                style={{
                  background: "#16181d",
                  color: "#34d399",
                  borderRadius: 8,
                  padding: "8px 10px",
                  fontSize: 12,
                  margin: "8px 0",
                  textAlign: "center",
                }}
              >
                {contact.fingerprint || "(fingerprint non disponibile)"}
              </div>
              <div style={{ display: "flex", gap: 6 }}>
                <button
                  disabled={busy || !contact.fingerprint}
                  onClick={() => {
                    void markVerified(contact.name).catch(() => {});
                    setVerifying(false);
                  }}
                  style={{
                    border: "none",
                    background: "#0f766e",
                    color: "#fff",
                    borderRadius: 7,
                    padding: "6px 11px",
                    fontSize: 11,
                    fontWeight: 600,
                    cursor: "pointer",
                  }}
                >
                  Coincide — segna verificato
                </button>
                <button
                  onClick={() => setVerifying(false)}
                  style={{
                    border: "1px solid rgba(0,0,0,.14)",
                    background: "#fff",
                    borderRadius: 7,
                    padding: "6px 11px",
                    fontSize: 11,
                    fontWeight: 600,
                    cursor: "pointer",
                  }}
                >
                  Annulla
                </button>
              </div>
            </div>
          )}

          {blocking && (
            <div
              style={{
                background: "#fdecec",
                border: "1px solid #f5c2c2",
                borderRadius: 10,
                padding: "10px 12px",
                marginBottom: 12,
              }}
            >
              <div style={{ fontSize: 11.5, color: "#b91c1c", lineHeight: 1.5 }}>
                Bloccare <b>{label}</b>? Questa offerta viene rifiutata e le
                prossime verranno scartate in silenzio (si annulla dalla Rubrica).
              </div>
              <div style={{ display: "flex", gap: 6, marginTop: 8 }}>
                <button
                  disabled={busy}
                  onClick={() => void doBlock()}
                  style={{
                    border: "none",
                    background: "#b91c1c",
                    color: "#fff",
                    borderRadius: 7,
                    padding: "6px 11px",
                    fontSize: 11,
                    fontWeight: 600,
                    cursor: "pointer",
                  }}
                >
                  Blocca e rifiuta
                </button>
                <button
                  onClick={() => setBlocking(false)}
                  style={{
                    border: "1px solid rgba(0,0,0,.14)",
                    background: "#fff",
                    borderRadius: 7,
                    padding: "6px 11px",
                    fontSize: 11,
                    fontWeight: 600,
                    cursor: "pointer",
                  }}
                >
                  Annulla
                </button>
              </div>
            </div>
          )}

          {/* dest folder */}
          <button
            onClick={pickFolder}
            style={{
              width: "100%",
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              background: "#f7f4f1",
              border: "none",
              borderRadius: 10,
              padding: "11px 13px",
              marginBottom: 16,
              cursor: "pointer",
            }}
          >
            <span style={{ fontSize: 11.5, color: "#57534c" }}>Salva in</span>
            <span style={{ fontSize: 11.5, fontWeight: 500, color: "#171514" }}>
              {dest
                ? shortenPath(dest)
                : defaultDir
                  ? `${shortenPath(defaultDir)} ⌵`
                  : "cartella predefinita ⌵"}
            </span>
          </button>

          <div style={{ display: "flex", gap: 10 }}>
            <button
              disabled={busy}
              onClick={() => void reject(offerId).catch(() => {})}
              style={{
                flex: 1,
                border: "1px solid rgba(0,0,0,.14)",
                background: "#fff",
                borderRadius: 11,
                padding: 12,
                fontSize: 13,
                fontWeight: 600,
                cursor: "pointer",
              }}
            >
              Rifiuta
            </button>
            <button
              disabled={busy}
              onClick={doAccept}
              style={{
                flex: 1,
                border: "none",
                background: "#16a34a",
                color: "#fff",
                borderRadius: 11,
                padding: 12,
                fontSize: 13,
                fontWeight: 700,
                cursor: "pointer",
              }}
            >
              {busy ? "…" : "Accetta"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function shortenPath(p: string): string {
  const parts = p.split(/[\\/]/);
  if (parts.length <= 2) return p;
  return "…/" + parts.slice(-2).join("/");
}
