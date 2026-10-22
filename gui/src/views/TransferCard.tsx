// La scheda sopra la barra (15b–15d, 16b–16d): mentre qualcosa passa, vive qui.
//
// Dopo «Invia» si torna dove si era, e il trasferimento non prende lo schermo:
// è una scheda blu notte sopra la barra, su qualunque schermata radice, con a
// chi, quanto, quanto manca — e si può intanto scegliere un altro destinatario,
// prendere dal MacBook, andare in Persone. Toccandola si apre il dettaglio come
// foglio (15c), file per file, con Pausa e Annulla. Quando è arrivato la stessa
// scheda diventa verde per qualche secondo, poi sparisce: il trasferimento resta
// in Attività.
//
// La scheda resta una sola qualunque sia il numero (16): ogni destinatario è un
// volto con il suo anello, sovrapposti come una pila; il testo dà il totale e
// chi è in casella. Nella pila i colori distinguono il verso — azzurro chiaro
// per ciò che arriva, blu per ciò che parte, verde per ciò che è arrivato.

import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { fire, useStore } from "../store";
import { useT } from "../i18n";
import { fmtBytes, fmtEta, pct } from "../format";
import { Icon } from "../ui/Icons";
import { ActionSheet } from "../ui/ActionSheet";
import { SheetGrab, useModal } from "../ui/Sheet";
import type { MenuItem } from "../ui/Menu";
import type { UIStatus, UITransfer } from "../types";
import { deviceGlyph, initial } from "./ExchangeView";

/** The states that are "passing": the card is for these. */
const MOVING: UIStatus[] = ["active", "preparing", "offered", "stalled", "paused"];
/** The states a moving row ends in without failing: the card goes green (or
 *  says «in casella») for this long, then leaves. */
const ENDED: UIStatus[] = ["completed", "deposited"];
const LINGER_MS = 4000;

const isMoving = (x: UITransfer) => MOVING.includes(x.status);
/** A send that is parked on the relay, or holding its offer for them: in casella. */
const inMailbox = (x: UITransfer) =>
  x.status === "deposited" || x.status === "offered" || x.status === "stalled";

/** What this app's own list calls the other side of a row. */
function useWho() {
  const peerLabel = useStore((s) => s.peerLabel);
  return (x: UITransfer) => x.peer || peerLabel(x.peerId ?? null, x.senderName);
}

/** The rows the card is about right now: what moves, plus what has just
 *  stopped moving — kept for a few seconds so the end is seen. */
function useCardRows(): { rows: UITransfer[]; dismiss: () => void } {
  const transfers = useStore((s) => s.transfers);
  const [ended, setEnded] = useState<Record<string, number>>({});
  const seen = useRef<Set<string>>(new Set());

  useEffect(() => {
    const now = Date.now();
    const next: Record<string, number> = { ...ended };
    let changed = false;
    for (const x of Object.values(transfers)) {
      if (isMoving(x)) seen.current.add(x.key);
      else if (ENDED.includes(x.status) && seen.current.has(x.key) && !(x.key in next)) {
        next[x.key] = now;
        changed = true;
      }
    }
    for (const k of Object.keys(next)) {
      // Gone from the list (forgotten, or the row was removed): nothing to show.
      if (!transfers[k]) {
        delete next[k];
        changed = true;
      }
    }
    if (changed) setEnded(next);
  }, [transfers]); // eslint-disable-line react-hooks/exhaustive-deps

  // Leave on time: one timer for the oldest, re-armed as the map changes.
  useEffect(() => {
    const stamps = Object.values(ended);
    if (!stamps.length) return;
    const due = Math.min(...stamps) + LINGER_MS - Date.now();
    const id = window.setTimeout(() => {
      const cut = Date.now() - LINGER_MS;
      setEnded((m) => {
        const out: Record<string, number> = {};
        for (const [k, v] of Object.entries(m)) if (v > cut) out[k] = v;
        for (const k of Object.keys(m)) if (!(k in out)) seen.current.delete(k);
        return out;
      });
    }, Math.max(0, due));
    return () => window.clearTimeout(id);
  }, [ended]);

  const rows = useMemo(
    () =>
      Object.values(transfers)
        .filter((x) => isMoving(x) || x.key in ended)
        .sort((a, b) => a.firstSeen - b.firstSeen),
    [transfers, ended]
  );
  const dismiss = () => {
    for (const k of Object.keys(ended)) seen.current.delete(k);
    setEnded({});
  };
  return { rows, dismiss };
}

/** The ring round a face: the quiet base, the progress over it. */
function Ring({ tx, size, cls }: { tx: UITransfer; size: number; cls?: string }) {
  const r = size / 2 - 1.5;
  const c = 2 * Math.PI * r;
  const done = tx.status === "completed";
  const p = done ? 100 : pct(tx);
  const stroke = done
    ? "var(--green-on-navy)"
    : tx.dir === "in"
      ? "var(--in-light)"
      : "var(--navy-blue)";
  return (
    <span className={`ring-av ${cls ?? ""}`} aria-hidden>
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} fill="none">
        <circle cx={size / 2} cy={size / 2} r={r} stroke="rgba(255,255,255,.12)" strokeWidth="3" />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          stroke={stroke}
          strokeWidth="3"
          strokeLinecap="round"
          strokeDasharray={c}
          strokeDashoffset={c * (1 - p / 100)}
          style={{ transition: "stroke-dashoffset 400ms var(--ease)" }}
        />
      </svg>
      <RingInner tx={tx} />
    </span>
  );
}

/** What sits inside the ring: the initial, a tick, the mailbox, or the machine. */
function RingInner({ tx }: { tx: UITransfer }) {
  const who = useWho();
  const status = useStore((s) => s.status);
  const mine = !!status && tx.peerId === status.public_id;
  if (tx.status === "completed")
    return (
      <span className="ring-av-in done">
        <Icon.Check size={17} />
      </span>
    );
  if (tx.dir === "out" && inMailbox(tx))
    return (
      <span className="ring-av-in">
        <Icon.Inbox size={18} />
      </span>
    );
  if (mine) return <span className="ring-av-in">{deviceGlyph("", 18)}</span>;
  return <span className="ring-av-in">{initial(who(tx))}</span>;
}

export function TransferDock() {
  const t = useT();
  const who = useWho();
  const { rows, dismiss } = useCardRows();
  const [open, setOpen] = useState(false);

  // The page under the dock needs the room: a card the last row hides under is
  // a card that reads as covering something, which it is.
  useEffect(() => {
    const el = document.documentElement;
    if (rows.length) el.style.setProperty("--dock", "74px");
    else el.style.removeProperty("--dock");
    return () => {
      el.style.removeProperty("--dock");
    };
  }, [rows.length]);

  useEffect(() => {
    if (!rows.length) setOpen(false);
  }, [rows.length]);

  if (!rows.length) return null;

  const moving = rows.filter(isMoving);
  const done = rows.filter((x) => x.status === "completed");
  const parked = rows.filter((x) => x.status === "deposited");
  const allDone = moving.length === 0;
  const size = rows.reduce((n, x) => n + x.size, 0);
  const moved = rows.reduce((n, x) => n + (x.status === "completed" ? x.size : x.transferred), 0);
  const overall = size ? Math.min(100, Math.round((moved / size) * 100)) : 0;

  let title: string;
  let sub: React.ReactNode;
  if (rows.length === 1) {
    const x = rows[0];
    const w = who(x);
    if (x.status === "completed") {
      title = x.dir === "out" ? t("xfer.arrivedTo", w) : t("xfer.arrivedFrom", w);
      sub = `${x.name} · ${fmtBytes(x.size)} · ${t("xfer.inSecs", Math.max(1, Math.round((Date.now() - x.firstSeen) / 1000)))}`;
    } else if (x.status === "deposited") {
      title = t("xfer.inMailboxTitle", w);
      sub = `${x.name} · ${fmtBytes(x.size)}`;
    } else {
      title = `${x.dir === "out" ? t("xfer.to", w) : t("xfer.from", w)} · ${pct(x)}%`;
      const eta = fmtEta(x);
      sub = [x.name, t("xfer.ofBytes", fmtBytes(x.transferred), fmtBytes(x.size)), eta]
        .filter(Boolean)
        .join(" · ");
    }
  } else {
    const dirs = new Set(rows.map((x) => x.dir));
    if (allDone) title = t("xfer.allArrived", rows.length);
    else if (done.length || parked.length)
      title = [
        t("xfer.nInProgress", moving.length),
        done.length ? t("xfer.nArrived", done.length) : "",
      ]
        .filter(Boolean)
        .join(" · ");
    else
      title = `${dirs.size === 1 ? (dirs.has("out") ? t("xfer.nSends", rows.length) : t("xfer.nReceives", rows.length)) : t("xfer.nTransfers", rows.length)} · ${overall}%`;
    const names = (xs: UITransfer[]) => xs.map(who).join(", ");
    const mailbox = rows.filter((x) => x.dir === "out" && inMailbox(x));
    const live = rows.filter((x) => !(x.dir === "out" && inMailbox(x)));
    if (dirs.size > 1)
      sub = (
        <>
          <span className="in">
            <Icon.ArrowDownLeft size={12} /> {names(live.filter((x) => x.dir === "in"))}
          </span>
          <span>
            <Icon.ArrowUpRight size={12} /> {names(live.filter((x) => x.dir === "out"))}
          </span>
        </>
      );
    else
      sub = [names(live), mailbox.length ? t("xfer.inMailbox", names(mailbox)) : ""]
        .filter(Boolean)
        .join(" · ");
  }

  return createPortal(
    <>
      <div className="dock">
        <button className="xfer-card" onClick={() => setOpen(true)} aria-label={title}>
          <span className="xfer-pile">
            {rows.slice(0, 3).map((x, i) => (
              <span key={x.key} style={{ position: "relative", zIndex: 10 - i, display: "flex" }}>
                <Ring tx={x} size={46} />
              </span>
            ))}
          </span>
          <span className="grow" style={{ minWidth: 0 }}>
            <span className="xfer-title tnum truncate" style={{ display: "block" }}>
              {title}
            </span>
            <span className="xfer-sub tnum">{sub}</span>
          </span>
          {allDone ? (
            <span
              className="xfer-done-btn"
              role="button"
              onClick={(e) => {
                e.stopPropagation();
                dismiss();
              }}
            >
              {t("xfer.done")}
            </span>
          ) : (
            <span className="xfer-open" aria-hidden>
              <Icon.ChevronUp size={16} />
            </span>
          )}
        </button>
      </div>
      {open ? <TransferSheet rows={rows} onClose={() => setOpen(false)} /> : null}
    </>,
    document.body
  );
}

/** Il foglio (15c, 16c): si apre dalla scheda in qualsiasi schermata. */
function TransferSheet({ rows, onClose }: { rows: UITransfer[]; onClose: () => void }) {
  const t = useT();
  const who = useWho();
  const ref = useRef<HTMLDivElement>(null);
  useModal(ref, onClose, true);
  const pause = useStore((s) => s.pause);
  const resume = useStore((s) => s.resume);
  const cancel = useStore((s) => s.cancel);
  const [held, setHeld] = useState<UITransfer | null>(null);

  const single = rows.length === 1 ? rows[0] : null;
  const size = rows.reduce((n, x) => n + x.size, 0);
  const moved = rows.reduce((n, x) => n + (x.status === "completed" ? x.size : x.transferred), 0);
  const pausable = rows.filter((x) => x.status === "active" && x.canPause);
  const paused = rows.filter((x) => x.status === "paused");
  const out = rows.filter((x) => x.dir === "out");
  const inn = rows.filter((x) => x.dir === "in");

  const rowSub = (x: UITransfer) => {
    if (x.status === "completed") return `${x.name} · ${t("xfer.arrived")}`;
    if (x.dir === "out" && inMailbox(x))
      return `${x.name} · ${x.status === "deposited" ? t("xfer.inMailboxWaiting") : t("xfer.waitingMailbox")}`;
    return `${x.name} · ${t("xfer.ofBytes", fmtBytes(x.transferred), fmtBytes(x.size))}${x.dir === "out" ? " · " + t("xfer.direct") : ""}`;
  };
  const rowRight = (x: UITransfer) =>
    x.status === "completed" ? (
      <span className="xr-done">{t("xfer.done")}</span>
    ) : (
      <span className="xr-pct tnum">{pct(x)}%</span>
    );
  const items = (x: UITransfer): MenuItem[] => {
    const out: MenuItem[] = [];
    if (x.status === "active" && x.canPause)
      out.push({ key: "pause", label: t("transfers.pause"), icon: <Icon.Pause size={15} />, onSelect: () => fire(pause(x.id)) });
    if (x.status === "paused")
      out.push({ key: "resume", label: t("transfers.resume"), icon: <Icon.Play size={15} />, onSelect: () => fire(resume(x.id)) });
    if (isMoving(x))
      out.push({ key: "cancel", label: t("transfers.cancel"), icon: <Icon.Close size={15} />, danger: true, onSelect: () => fire(cancel(x.id)) });
    return out;
  };

  const etaAll = (() => {
    const rate = rows.reduce((n, x) => n + (x.rate ?? 0), 0);
    if (!rate) return "";
    return fmtEta({ ...rows[0], size, transferred: moved, rate } as UITransfer);
  })();

  return createPortal(
    <>
      <div className="xfer-scrim" onClick={onClose} />
      <div ref={ref} className="xfer-sheet" role="dialog" aria-modal="true" aria-label={t("xfer.sheetTitle")}>
        <SheetGrab />
        {single ? (
          <>
            <div className="xs-top">
              <Ring tx={single} size={60} cls="lg" />
              <div className="grow" style={{ minWidth: 0 }}>
                <div className="xs-pct tnum">{single.status === "completed" ? "100%" : `${pct(single)}%`}</div>
                <div className="xs-who tnum truncate">
                  {single.dir === "out" ? t("xfer.to", who(single)) : t("xfer.from", who(single))}
                  {fmtEta(single) ? ` · ${t("xfer.remaining", fmtEta(single))}` : ""}
                </div>
              </div>
              <span className="xs-pill">
                {single.dir === "out" && inMailbox(single) ? (
                  <>
                    <Icon.Inbox size={13} /> {t("xfer.mailbox")}
                  </>
                ) : (
                  <>
                    <Icon.Wifi size={13} /> {t("xfer.direct")}
                  </>
                )}
              </span>
            </div>
            <div>
              <div className="xf-row">
                <div className="xf-line">
                  <span className="xf-thumb" aria-hidden />
                  <div className="grow" style={{ minWidth: 0 }}>
                    <div className="xf-name truncate">{single.name}</div>
                    <div className="xf-size tnum">
                      {single.status === "completed"
                        ? fmtBytes(single.size)
                        : t("xfer.ofBytes", fmtBytes(single.transferred), fmtBytes(single.size))}
                    </div>
                  </div>
                  {single.status === "completed" ? <Icon.Check size={18} /> : null}
                </div>
                <div className="xf-bar">
                  <span
                    className={single.status === "completed" ? "done" : undefined}
                    style={{ width: `${single.status === "completed" ? 100 : pct(single)}%` }}
                  />
                </div>
              </div>
            </div>
            {isMoving(single) ? (
              <div className="xs-actions">
                {single.status === "paused" ? (
                  <button className="xs-act" onClick={() => fire(resume(single.id))}>
                    <Icon.Play size={17} /> {t("transfers.resume")}
                  </button>
                ) : (
                  <button className="xs-act" disabled={!single.canPause || single.status !== "active"} onClick={() => fire(pause(single.id))}>
                    <Icon.Pause size={17} /> {t("transfers.pause")}
                  </button>
                )}
                <button className="xs-act danger" onClick={() => fire(cancel(single.id))}>
                  {t("transfers.cancel")}
                </button>
              </div>
            ) : null}
            <div className="xs-note">{t("xfer.bgNote")}</div>
          </>
        ) : (
          <>
            <div className="xs-top multi">
              <div>
                <div className="xs-total tnum">{t("xfer.nTransfers", rows.length)}</div>
                <div className="xs-total-sub tnum">
                  {t("xfer.ofBytes", fmtBytes(moved), fmtBytes(size))}
                  {etaAll ? ` · ${t("xfer.about", etaAll)}` : ""}
                </div>
              </div>
              {pausable.length ? (
                <button className="xs-pause-all" onClick={() => pausable.forEach((x) => fire(pause(x.id)))}>
                  <Icon.Pause size={15} /> {t("transfers.pause")}
                </button>
              ) : paused.length ? (
                <button className="xs-pause-all" onClick={() => paused.forEach((x) => fire(resume(x.id)))}>
                  <Icon.Play size={15} /> {t("transfers.resume")}
                </button>
              ) : null}
            </div>
            {out.length ? <div className="xs-lane">{t("xfer.outgoing")}</div> : null}
            {out.map((x) => (
              <button key={x.key} className="xr-row" onClick={() => setHeld(x)}>
                <Ring tx={x} size={44} cls="md" />
                <span className="grow" style={{ minWidth: 0 }}>
                  <span className="xr-name">
                    <Icon.ArrowUpRight size={13} />
                    <span className="truncate">{who(x)}</span>
                  </span>
                  <span className="xr-sub tnum truncate" style={{ display: "block" }}>
                    {rowSub(x)}
                  </span>
                </span>
                {rowRight(x)}
              </button>
            ))}
            {inn.length ? <div className="xs-lane">{t("xfer.incoming")}</div> : null}
            {inn.map((x) => (
              <button key={x.key} className="xr-row" onClick={() => setHeld(x)}>
                <Ring tx={x} size={44} cls="md" />
                <span className="grow" style={{ minWidth: 0 }}>
                  <span className="xr-name">
                    <Icon.ArrowDownLeft size={13} />
                    <span className="truncate">{t("xfer.fromWho", who(x))}</span>
                  </span>
                  <span className="xr-sub tnum truncate" style={{ display: "block" }}>
                    {rowSub(x)}
                  </span>
                </span>
                {rowRight(x)}
              </button>
            ))}
            <div className="xs-note">{t("xfer.rowNote")}</div>
          </>
        )}
      </div>
      {held ? (
        <ActionSheet
          open
          onClose={() => setHeld(null)}
          title={held.name}
          meta={held.dir === "out" ? t("xfer.to", who(held)) : t("xfer.from", who(held))}
          items={items(held)}
        />
      ) : null}
    </>,
    document.body
  );
}
