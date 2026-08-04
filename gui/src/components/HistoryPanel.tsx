// What already happened — the GUI's `arvolo history`. Read-only by construction:
// nothing here can still be acted on, which is exactly what separates it from the
// board (whose rows can all still be paused, cancelled or accepted). Fetched on
// open, like the deposits panel: there is no push event for a log.

import { useStore } from "../store";
import { extOf, fmtBytes } from "../format";
import type { HistoryDto } from "../types";

function dayLabel(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  const today = new Date();
  const yday = new Date(today);
  yday.setDate(today.getDate() - 1);
  const same = (a: Date, b: Date) =>
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate();
  if (same(d, today)) return "Oggi";
  if (same(d, yday)) return "Ieri";
  return d.toLocaleDateString("it-IT", { day: "numeric", month: "long", year: "numeric" });
}

function statusLabel(s: string): { text: string; color: string } {
  if (s === "completed") return { text: "Completato", color: "var(--green)" };
  if (s === "cancelled") return { text: "Annullato", color: "#8a827a" };
  if (s === "deposited") return { text: "Depositato", color: "var(--in)" };
  if (s.startsWith("failed")) return { text: "Fallito", color: "var(--red)" };
  return { text: s, color: "var(--ink-sec)" };
}

export function HistoryPanel() {
  const isOpen = useStore((s) => s.historyOpen);
  const close = useStore((s) => s.closeHistory);
  const history = useStore((s) => s.history);
  const loading = useStore((s) => s.historyLoading);
  const error = useStore((s) => s.historyError);
  const load = useStore((s) => s.loadHistory);
  const clear = useStore((s) => s.clearHistory);
  const peerLabel = useStore((s) => s.peerLabel);

  if (!isOpen) return null;

  // Group by day, newest first (the daemon already sorts newest-first).
  const groups: { day: string; items: HistoryDto[] }[] = [];
  for (const r of history) {
    const day = dayLabel(r.created);
    const last = groups[groups.length - 1];
    if (last && last.day === day) last.items.push(r);
    else groups.push({ day, items: [r] });
  }

  // A full view in the main pane (the sidebar's "Storico"), not an overlay.
  return (
    <div
      style={{
        flex: 1,
        minWidth: 0,
        background: "var(--card)",
        border: "1px solid var(--line)",
        borderRadius: 16,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
      }}
    >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "18px 20px",
            borderBottom: "1px solid var(--line)",
          }}
        >
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 15, fontWeight: 700 }}>Storico</div>
            <div style={{ fontSize: 11.5, color: "var(--ink-mut)" }}>
              I trasferimenti conclusi — qui non c'è più niente da fare, solo da ricordare
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
              color: loading ? "var(--ink-mut)" : "var(--ink)",
            }}
          >
            {loading ? "…" : "Aggiorna"}
          </button>
          {history.length > 0 && (
            <button
              onClick={() => void clear().catch(() => {})}
              title="Dimentica tutto lo storico (la board e i depositi non vengono toccati)"
              style={{
                border: "1px solid rgba(185,28,28,.3)",
                background: "#fff",
                color: "#b91c1c",
                borderRadius: 8,
                padding: "6px 12px",
                fontSize: 11.5,
                fontWeight: 600,
                cursor: "pointer",
              }}
            >
              Svuota
            </button>
          )}
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

        <div style={{ flex: 1, overflowY: "auto", padding: "10px 18px 18px" }}>
          {error && (
            <div
              className="selectable"
              style={{
                background: "#fdecec",
                color: "#b91c1c",
                borderRadius: 10,
                padding: "9px 12px",
                fontSize: 12,
                margin: "8px 0",
              }}
            >
              {error}
            </div>
          )}
          {history.length === 0 && !loading && !error && (
            <div style={{ fontSize: 12.5, color: "var(--ink-mut)", padding: "22px 4px", textAlign: "center" }}>
              Ancora niente: quello che completi (o annulli) finirà qui.
            </div>
          )}
          {groups.map((g) => (
            <div key={g.day}>
              <div
                className="mono"
                style={{
                  fontSize: 9.5,
                  fontWeight: 600,
                  letterSpacing: ".08em",
                  textTransform: "uppercase",
                  color: "var(--ink-mut)",
                  padding: "10px 4px 4px",
                }}
              >
                {g.day}
              </div>
              {g.items.map((r) => {
                const sm = statusLabel(r.status);
                return (
                  <div
                    key={r.id}
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 10,
                      border: "1px solid rgba(0,0,0,.07)",
                      borderRadius: 11,
                      padding: "9px 11px",
                      marginBottom: 6,
                    }}
                  >
                    <span
                      style={{
                        width: 22,
                        height: 22,
                        borderRadius: 6,
                        flex: "none",
                        display: "flex",
                        alignItems: "center",
                        justifyContent: "center",
                        fontSize: 11,
                        fontWeight: 700,
                        background: r.direction === "send" ? "#fff3e9" : "#e9f3fb",
                        color: r.direction === "send" ? "#c2410c" : "var(--in)",
                      }}
                    >
                      {r.direction === "send" ? "↗" : "↙"}
                    </span>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div
                        style={{
                          fontSize: 12,
                          fontWeight: 600,
                          overflow: "hidden",
                          textOverflow: "ellipsis",
                          whiteSpace: "nowrap",
                        }}
                      >
                        {r.name}
                        <span className="mono" style={{ fontWeight: 500, color: "var(--ink-weak)", fontSize: 10, marginLeft: 6 }}>
                          {extOf(r.name)} · {fmtBytes(r.total_size)}
                        </span>
                      </div>
                      <div style={{ fontSize: 10.5, color: "var(--ink-mut)" }}>
                        {r.direction === "send" ? "a" : "da"}{" "}
                        {peerLabel(r.peer, undefined)}
                        {" · "}
                        {new Date(r.created * 1000).toLocaleTimeString("it-IT", {
                          hour: "2-digit",
                          minute: "2-digit",
                        })}
                      </div>
                    </div>
                    <span style={{ fontSize: 10.5, fontWeight: 600, color: sm.color, flex: "none" }}>
                      {sm.text}
                    </span>
                  </div>
                );
              })}
            </div>
          ))}
        </div>
    </div>
  );
}
