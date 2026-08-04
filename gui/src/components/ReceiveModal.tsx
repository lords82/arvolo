// Paste-anything receive — the GUI's `arvolo recv`. One field for all three
// artefacts (arvc… ticket, pairing code, arvm… offline ticket): the daemon works
// out which it is, exactly like the CLI verb, and the download lands on the board
// as a normal row. A browser link opens in the browser and needs no app at all,
// so it is not accepted here — a hint says so if one is pasted.

import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";

/** What kind of artefact the pasted text looks like — mirrors the daemon's own
 *  sorting; only used to adapt the hint text and the password field. */
function sniff(t: string): "ticket" | "offline" | "code" | "link" | "unknown" {
  const s = t.trim();
  if (!s) return "unknown";
  if (/^https?:\/\//i.test(s)) return "link";
  if (/^arvc/i.test(s)) return "ticket";
  if (/^arvm/i.test(s)) return "offline";
  // The CLI's code grammar: digits-word-word, with an optional @relay tail.
  if (/^\d{3,5}-[a-z]+-[a-z]+(@\S+)?$/i.test(s)) return "code";
  return "unknown";
}

export function ReceiveModal() {
  const isOpen = useStore((s) => s.receiveOpen);
  const close = useStore((s) => s.closeReceive);
  const receive = useStore((s) => s.receive);
  const defaultDir = useStore((s) => s.status?.download_dir ?? "");

  const [ticket, setTicket] = useState("");
  const [password, setPassword] = useState("");
  const [dest, setDest] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  useEffect(() => {
    if (isOpen) {
      setTicket("");
      setPassword("");
      setDest(null);
      setBusy(false);
      setErr("");
    }
  }, [isOpen]);

  const kind = useMemo(() => sniff(ticket), [ticket]);

  if (!isOpen) return null;

  const pickFolder = async () => {
    const sel = await open({ directory: true, multiple: false });
    if (typeof sel === "string") setDest(sel);
  };

  const go = async () => {
    setBusy(true);
    setErr("");
    try {
      await receive(ticket.trim(), dest, password.trim() ? password : null);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const hint = {
    ticket: "Ticket P2P — scarichi direttamente dal mittente (deve essere online).",
    offline: "Deposito sul relay — si scarica anche se il mittente è offline.",
    code: "Codice breve — si aggancia al mittente tramite il relay.",
    link: "Questo è un link per il browser: aprilo lì, non serve Arvolo.",
    unknown: "",
  }[kind];

  const acceptable = kind === "ticket" || kind === "offline" || kind === "code";

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
          width: 500,
          background: "#fff",
          borderRadius: 18,
          boxShadow: "0 30px 70px -12px rgba(0,0,0,.45)",
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
          <div
            style={{
              width: 36,
              height: 36,
              borderRadius: 10,
              background: "#e9f3fb",
              color: "#0369a1",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: 17,
              fontWeight: 700,
              flex: "none",
            }}
          >
            ↓
          </div>
          <div style={{ flex: 1 }}>
            <div style={{ fontSize: 15, fontWeight: 700 }}>Ricevi</div>
            <div style={{ fontSize: 11.5, color: "#a8a29a" }}>
              Incolla un ticket, un codice o un deposito che ti hanno mandato
            </div>
          </div>
          <button
            onClick={close}
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

        <div style={{ padding: "16px 20px 20px" }}>
          {err && (
            <div
              className="selectable"
              style={{
                background: "#fdecec",
                color: "#b91c1c",
                borderRadius: 10,
                padding: "9px 12px",
                fontSize: 12,
                marginBottom: 12,
              }}
            >
              {err}
            </div>
          )}

          <textarea
            value={ticket}
            onChange={(e) => setTicket(e.target.value)}
            placeholder={"arvc… / 4821-crater-mango / arvm…"}
            rows={3}
            className="mono"
            style={{
              width: "100%",
              border: "1px solid var(--line-strong)",
              borderRadius: 10,
              padding: "11px 13px",
              fontSize: 12,
              resize: "none",
              outline: "none",
              marginBottom: 6,
            }}
          />
          {hint && (
            <div
              style={{
                fontSize: 11.5,
                color: kind === "link" ? "#b45309" : "#57534c",
                marginBottom: 10,
              }}
            >
              {kind === "link" ? "⚠ " : "✓ "}
              {hint}
            </div>
          )}

          {kind === "offline" && (
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="Password (solo se il deposito ne ha una)"
              style={{
                width: "100%",
                border: "1px solid var(--line-strong)",
                borderRadius: 10,
                padding: "10px 12px",
                fontSize: 12,
                marginBottom: 10,
                outline: "none",
              }}
            />
          )}

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
              marginBottom: 14,
              cursor: "pointer",
            }}
          >
            <span style={{ fontSize: 11.5, color: "#57534c" }}>Salva in</span>
            <span style={{ fontSize: 11.5, fontWeight: 500 }}>
              {dest ?? (defaultDir || "cartella predefinita")} ⌵
            </span>
          </button>

          <button
            disabled={busy || !acceptable}
            onClick={go}
            style={{
              width: "100%",
              border: "none",
              background: busy || !acceptable ? "#e2ddd6" : "#0369a1",
              color: "#fff",
              borderRadius: 11,
              padding: 13,
              fontSize: 13,
              fontWeight: 700,
              cursor: busy || !acceptable ? "default" : "pointer",
            }}
          >
            {busy
              ? kind === "code"
                ? "Aggancio al mittente…"
                : "Avvio…"
              : "Scarica"}
          </button>
        </div>
      </div>
    </div>
  );
}
