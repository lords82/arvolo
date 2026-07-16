// Everything left on a relay that can still be taken back: public download links
// and sealed deposits awaiting their recipient.
//
// This is the only place the app shows them, on purpose. The list is not
// event-driven — no engine event exists for a deposit, and a relay never reports a
// download back — so it is only ever as fresh as its last fetch. Keeping it behind
// a panel that fetches on open is what stops a stale list from sitting on screen
// pretending to be live.

import { useState } from "react";
import { useStore } from "../store";
import { depositMeta, extOf, fmtBytes } from "../format";
import type { DepositDto } from "../types";

export function DepositsPanel() {
  const open = useStore((s) => s.depositsOpen);
  const close = useStore((s) => s.closeDeposits);
  const deposits = useStore((s) => s.deposits);
  const loading = useStore((s) => s.depositsLoading);
  const error = useStore((s) => s.depositsError);
  const load = useStore((s) => s.loadDeposits);

  if (!open) return null;

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
        style={{
          width: 560,
          maxHeight: 600,
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
            <div style={{ fontSize: 15, fontWeight: 700 }}>Link e depositi</div>
            <div style={{ fontSize: 11.5, color: "#a8a29a" }}>
              Quello che hai lasciato su un relay e puoi ancora ritirare
            </div>
          </div>
          <button
            onClick={() => void load()}
            disabled={loading}
            style={{
              border: "1px solid var(--line-strong)",
              background: "#fff",
              borderRadius: 8,
              padding: "6px 12px",
              fontSize: 11.5,
              fontWeight: 600,
              cursor: loading ? "default" : "pointer",
              color: loading ? "#a8a29a" : "#171514",
            }}
          >
            {loading ? "Controllo…" : "Aggiorna"}
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

        {error && (
          <div
            role="alert"
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "8px 20px",
              background: "#fdecec",
              borderBottom: "1px solid #f5c2c2",
              fontSize: 11.5,
              color: "#b91c1c",
            }}
          >
            <span>⚠</span>
            <span className="selectable" style={{ flex: 1, minWidth: 0 }}>
              {error}
            </span>
            <button
              onClick={() => void load()}
              style={{
                border: "1px solid rgba(185,28,28,.3)",
                background: "#fff",
                color: "#b91c1c",
                borderRadius: 7,
                padding: "3px 10px",
                fontSize: 11,
                fontWeight: 600,
                cursor: "pointer",
              }}
            >
              Riprova
            </button>
          </div>
        )}

        <div style={{ overflowY: "auto", padding: "8px 12px 14px" }}>
          {deposits.length === 0 ? (
            <Empty loading={loading} />
          ) : (
            deposits.map((d) => <Row key={d.id} d={d} />)
          )}
        </div>
      </div>
    </div>
  );
}

function Empty({ loading }: { loading: boolean }) {
  return (
    <div style={{ padding: "34px 20px", textAlign: "center" }}>
      <div style={{ fontSize: 13, fontWeight: 600, color: "#57534c" }}>
        {loading ? "Controllo…" : "Nessun link o deposito attivo"}
      </div>
      {!loading && (
        <div
          style={{
            fontSize: 11.5,
            color: "#a8a29a",
            marginTop: 6,
            lineHeight: 1.5,
          }}
        >
          Quando crei un link di download o invii a qualcuno che è offline,
          compare qui finché non scade o lo revochi.
        </div>
      )}
    </div>
  );
}

function Row({ d }: { d: DepositDto }) {
  const revoke = useStore((s) => s.revokeDeposit);
  const revoking = useStore((s) => s.revoking);
  // Sealed deposits name a contact, so say who: the board already calls people by
  // name, and a raw key here would be the one place that doesn't. Falls back to a
  // short id for someone who isn't in the book.
  const peerLabel = useStore((s) => s.peerLabel);
  const [confirming, setConfirming] = useState(false);
  const [copied, setCopied] = useState(false);

  const meta = depositMeta(d);
  const busy = revoking.includes(d.id);
  const isLink = d.kind === "link";

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "11px 10px",
        borderBottom: "1px solid var(--line)",
      }}
    >
      <div
        className="mono"
        style={{
          width: 34,
          height: 34,
          borderRadius: 9,
          background: isLink ? "#f3edff" : "#f0ece7",
          color: isLink ? "#7c3aed" : "#57534c",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          fontSize: 8.5,
          fontWeight: 700,
          flex: "none",
        }}
      >
        {extOf(d.name)}
      </div>

      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontSize: 12.5,
            fontWeight: 600,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {d.name}
          <span style={{ fontWeight: 500, color: "#a8a29a", marginLeft: 6 }}>
            {fmtBytes(d.size)}
          </span>
        </div>
        <div style={{ fontSize: 11, marginTop: 2 }}>
          <span style={{ color: meta.color, fontWeight: 600 }}>{meta.text}</span>
          <span style={{ color: "#a8a29a" }}> · {meta.detail}</span>
        </div>
        <div
          className="mono"
          style={{ fontSize: 10, color: "#a8a29a", marginTop: 2 }}
        >
          {isLink ? d.link : `sigillato per ${peerLabel(d.recipient, "")}`}
        </div>
      </div>

      <div style={{ display: "flex", gap: 6, flex: "none" }}>
        {isLink && d.link && (
          <button
            onClick={() => {
              navigator.clipboard.writeText(d.link);
              setCopied(true);
              setTimeout(() => setCopied(false), 1200);
            }}
            style={{
              border: "1px solid var(--line-strong)",
              background: "#fff",
              borderRadius: 8,
              padding: "6px 11px",
              fontSize: 11.5,
              fontWeight: 600,
              cursor: "pointer",
            }}
          >
            {copied ? "Copiato ✓" : "Copia"}
          </button>
        )}
        {confirming ? (
          // Revoking is irreversible and, for a link already shared, it breaks the
          // download for everyone holding it. Ask — in place, saying what happens.
          <>
            <button
              onClick={() => {
                setConfirming(false);
                void revoke(d.id).catch(() => {});
              }}
              disabled={busy}
              style={{
                border: "none",
                background: "#dc2626",
                color: "#fff",
                borderRadius: 8,
                padding: "6px 11px",
                fontSize: 11.5,
                fontWeight: 600,
                cursor: busy ? "default" : "pointer",
              }}
            >
              {meta.revocable ? "Sì, revoca" : "Sì, elimina"}
            </button>
            <button
              onClick={() => setConfirming(false)}
              style={{
                border: "1px solid var(--line-strong)",
                background: "#fff",
                borderRadius: 8,
                padding: "6px 11px",
                fontSize: 11.5,
                fontWeight: 600,
                cursor: "pointer",
              }}
            >
              No
            </button>
          </>
        ) : (
          <button
            onClick={() => setConfirming(true)}
            disabled={busy}
            title={
              meta.revocable
                ? "Ritira il file dal relay: il link smette di funzionare per tutti"
                : "Non c'è più nulla sul relay: elimina solo la voce"
            }
            style={{
              border: "1px solid var(--line-strong)",
              background: "#fff",
              color: busy ? "#a8a29a" : "#b91c1c",
              borderRadius: 8,
              padding: "6px 11px",
              fontSize: 11.5,
              fontWeight: 600,
              cursor: busy ? "default" : "pointer",
            }}
          >
            {busy ? "Revoca…" : meta.revocable ? "Revoca" : "Elimina"}
          </button>
        )}
      </div>
    </div>
  );
}
