// The log of what has finished. Read-only by construction: anything still
// actionable is a live row on the board, not a line here.
//
// Grouped by real calendar day using the engine's own timestamp rather than when
// this window happened to see it — otherwise every restart would file last
// month's transfers under "Oggi".

import { useMemo, useState } from "react";
import { fire, useStore } from "../store";
import { fmtBytes } from "../format";
import { Icon } from "../ui/Icons";
import { Button, Empty, Segmented, TextInput } from "../ui/Primitives";
import { ExtChip } from "../ui/Bits";
import { Confirm } from "../ui/Sheet";
import type { HistoryDto } from "../types";

type Filter = "all" | "send" | "recv";

const DAY_FMT = new Intl.DateTimeFormat("it-IT", {
  weekday: "long",
  day: "numeric",
  month: "long",
});
const TIME_FMT = new Intl.DateTimeFormat("it-IT", {
  hour: "2-digit",
  minute: "2-digit",
});

function dayKey(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

function dayLabel(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (dayKey(unixSecs) === dayKey(Math.floor(today.getTime() / 1000)))
    return "Oggi";
  if (dayKey(unixSecs) === dayKey(Math.floor(yesterday.getTime() / 1000)))
    return "Ieri";
  return DAY_FMT.format(d);
}

/** Split a daemon status into a word and its tone. `failed: …` carries its
 *  reason after the colon and that reason is the useful half. */
function outcome(status: string): { text: string; tone: string; detail?: string } {
  if (status === "completed") return { text: "Completato", tone: "ok" };
  if (status === "cancelled") return { text: "Annullato", tone: "mut" };
  if (status === "deposited") return { text: "Depositato", tone: "in" };
  const [head, ...rest] = status.split(":");
  if (head === "failed")
    return { text: "Fallito", tone: "bad", detail: rest.join(":").trim() };
  // Anything the engine invents that this build has not learned yet: name the
  // fact, and keep the raw string as the detail rather than putting English in
  // the status slot.
  return { text: "Esito sconosciuto", tone: "mut", detail: status };
}

function Row({ h }: { h: HistoryDto }) {
  const peerLabel = useStore((s) => s.peerLabel);
  const o = outcome(h.status);
  const out = h.direction === "send";
  return (
    <div className={`row dir-${out ? "out" : "in"} is-done`}>
      <ExtChip name={h.name} />
      <div className="row-main">
        <div className="row-name truncate" title={h.name}>
          {h.name}
        </div>
        <div className="row-meta">
          <span className={`tone-${o.tone}`} style={{ fontWeight: 600 }}>
            {o.text}
          </span>
          <span className="sep" />
          <span className="truncate">
            {out ? "a" : "da"} {peerLabel(h.peer)}
          </span>
          <span className="sep" />
          <span className="tnum">{fmtBytes(h.total_size)}</span>
          {o.detail && (
            <>
              <span className="sep" />
              <span className="truncate" title={o.detail}>
                {o.detail}
              </span>
            </>
          )}
        </div>
      </div>
      <span className="t-xs t-mut tnum">
        {TIME_FMT.format(new Date(h.created * 1000))}
      </span>
    </div>
  );
}

export function HistoryView() {
  const history = useStore((s) => s.history);
  const loading = useStore((s) => s.historyLoading);
  const error = useStore((s) => s.historyError);
  const reload = useStore((s) => s.loadHistory);
  const clear = useStore((s) => s.clearHistory);
  const peerLabel = useStore((s) => s.peerLabel);

  const [filter, setFilter] = useState<Filter>("all");
  const [q, setQ] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);

  const groups = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const rows = history.filter((h) => {
      if (filter !== "all" && h.direction !== filter) return false;
      if (!needle) return true;
      return (
        h.name.toLowerCase().includes(needle) ||
        peerLabel(h.peer).toLowerCase().includes(needle)
      );
    });
    const map = new Map<string, { label: string; when: number; items: HistoryDto[] }>();
    for (const h of rows) {
      const k = dayKey(h.created);
      const g = map.get(k);
      if (g) g.items.push(h);
      else map.set(k, { label: dayLabel(h.created), when: h.created, items: [h] });
    }
    return Array.from(map.values()).sort((a, b) => b.when - a.when);
  }, [history, filter, q, peerLabel]);

  return (
    <div className="stack">
      <div className="hstack wrap">
        <Segmented
          label="Filtro cronologia"
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: "Tutto" },
            { value: "send", label: "Inviati" },
            { value: "recv", label: "Ricevuti" },
          ]}
        />
        <div className="grow" style={{ maxWidth: 280 }}>
          <TextInput
            value={q}
            onChange={(e) => setQ(e.currentTarget.value)}
            placeholder="Cerca…"
            aria-label="Cerca nella cronologia"
          />
        </div>
        <div className="spacer grow" />
        <Button size="sm" onClick={() => fire(reload())} busy={loading}>
          <Icon.Refresh size={13} /> Aggiorna
        </Button>
        <Button
          size="sm"
          variant="danger"
          disabled={!history.length}
          onClick={() => setConfirmClear(true)}
        >
          <Icon.Trash size={13} /> Svuota
        </Button>
      </div>

      {error && (
        <div className="card card-pad t-sm" style={{ borderColor: "var(--red)" }}>
          {error}
        </div>
      )}

      {groups.length === 0 ? (
        <div className="card">
          <Empty
            icon={<Icon.History size={22} />}
            title={history.length ? "Nessun risultato" : "Ancora niente"}
          >
            {history.length
              ? "Prova a cambiare filtro o ricerca."
              : "Qui finisce ogni trasferimento concluso: cosa, con chi e com'è andata."}
          </Empty>
        </div>
      ) : (
        groups.map((g) => (
          <div key={g.label + g.when} className="section">
            <div className="section-head">
              <span className="t-label">{g.label}</span>
              <span className="t-xs t-mut tnum">{g.items.length}</span>
            </div>
            <div className="card rows">
              {g.items.map((h) => (
                <Row key={h.id} h={h} />
              ))}
            </div>
          </div>
        ))
      )}

      <Confirm
        open={confirmClear}
        title="Svuotare la cronologia?"
        body="Il registro viene dimenticato per intero e non si può recuperare. I file già ricevuti restano dove sono; questo cancella solo l'elenco."
        confirmLabel="Svuota"
        danger
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => {
          setConfirmClear(false);
          fire(clear());
        }}
      />
    </div>
  );
}
