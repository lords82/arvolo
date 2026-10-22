// Attività: one list of what is moving, and the same place for what has moved.
//
// This screen has been a board of two columns, then a mailbox of correspondents,
// then a board of two directions. The design of record ("Arvolo Mobile 2 —
// Scambia", 7a/7d/7q) makes it one list in time order, and the argument is the
// phone's rather than the window's: on a screen this narrow a direction is not a
// column, it is two words at the start of a line — *da Marco*, *a Sara* — and
// splitting a handful of rows into two headed sections meant scrolling past an
// empty half to find the one transfer you came to look at.
//
// So, top to bottom: anything waiting on a decision, then everything in flight,
// then what finished today. Earlier than today is not hidden — it is the other
// half of the segmented control, which is the same screen at another time rather
// than a place to navigate to.

import { useEffect, useMemo, useState } from "react";
import { fire, useStore } from "../store";
import {
  fmtAgo,
  fmtBytes,
  isToday,
  metaLine,
  pct,
  statusMeta,
} from "../format";
import { useT } from "../i18n";
import type { UITransfer } from "../types";
import { Icon } from "../ui/Icons";
import { Button, Empty, Segmented, TextInput } from "../ui/Primitives";
import { Confirm } from "../ui/Sheet";
import { useCopy } from "../ui/Bits";
import { openUrl } from "@tauri-apps/plugin-opener";
import { depositMeta } from "../format";
import type { DepositDto } from "../types";
import { Avatar, Progress } from "../ui/Bits";
import { SwipeRow, type SwipeAction } from "../ui/SwipeRow";
import { ActionSheet } from "../ui/ActionSheet";
import type { MenuItem } from "../ui/Menu";
import { api } from "../ipc";
import { toast } from "../ui/Toasts";
import { HistoryView } from "./HistoryView";

/** A row is showing motion if there is motion to show. Everything else on it —
 *  the percentage, the bar — hangs off this one question. */
function isMoving(tx: UITransfer): boolean {
  return tx.status === "active" || tx.status === "stalled";
}

/** Everything still in flight, held, or waiting for the other side to take it. */
function isLive(tx: UITransfer): boolean {
  return (
    tx.status === "active" ||
    tx.status === "preparing" ||
    tx.status === "sharing" ||
    tx.status === "paused" ||
    tx.status === "stalled" ||
    tx.status === "offered" ||
    tx.status === "cancelling"
  );
}

function isDone(tx: UITransfer): boolean {
  return (
    tx.status === "completed" ||
    tx.status === "failed" ||
    tx.status === "cancelled" ||
    tx.status === "deposited"
  );
}

/** The one line under a row's name, and the direction is the first thing on it.
 *
 *  Tinted, because that is the whole of what replaced the two columns: *da Marco*
 *  in the arriving colour and *a Sara* in the sending one say which way the bytes
 *  are going before the sentence after them is read. */
function Meta({ tx, who }: { tx: UITransfer; who: string }) {
  const t = useT();
  const parts: string[] = [];
  // The app's own sentence for every state, the same one the window writes:
  // "62% · 4,2 MB/s · 2 min" while it moves, "riprende appena possibile" while it
  // cannot. It is also why there is no status badge on this row — a badge beside
  // a line that already says it is the same fact twice, and the design of record
  // spends that space on a way *in* instead (see `side` below).
  const said = metaLine(tx);
  if (said) parts.push(said);
  if (isMoving(tx) && tx.swarmPeers > 0)
    parts.push(t("transfers.peers", tx.swarmPeers));
  if (!said) {
    // `metaLine` says nothing about a finished row — on a desktop the peer and
    // the time are columns of their own. Here there are no columns, and a wall
    // of green ticks with no sender, no size and no time on any of them is what
    // that silence looked like.
    if (tx.size > 0) parts.push(fmtBytes(tx.size));
    if (tx.stamped) parts.push(fmtAgo(Math.floor(tx.firstSeen / 1000)));
  }
  return (
    <span className="act-meta truncate">
      {who && who !== "unknown" ? (
        <>
          <span className={`act-dir tone-${tx.dir}`}>
            {tx.dir === "in" ? t("activity.from", who) : t("activity.to", who)}
          </span>
          {parts.length ? " · " : null}
        </>
      ) : null}
      {parts.join(" · ")}
    </span>
  );
}

/** The decision, cold and at the top, with its own verb.
 *
 *  Not accept-or-reject here. Who it is from and the eight words that say so are
 *  what decides an offer, and neither fits on a card — so the card asks to be
 *  looked at, and the looking is the screen behind "Esamina" (the design's 7b). */
function Offer({ tx }: { tx: UITransfer }) {
  const t = useT();
  const openIncoming = useStore((s) => s.openIncoming);
  const peerLabel = useStore((s) => s.peerLabel);
  const contactsById = useStore((s) => s.contactsById);

  const myId = useStore((st) => st.status?.public_id ?? "");
  const mine = !!myId && tx.peerId === myId;
  // `peerLabel` answers "Your devices" for our own key, which is a column
  // heading: inside "X wants to send you a file" it reads as a plural subject
  // for a sentence about one machine. The sentence gets its own subject.
  const who = mine
    ? t("activity.oneOfYours")
    : tx.peer || peerLabel(tx.peerId ?? null, tx.senderName);
  const known = mine || (tx.peerId ? !!contactsById[tx.peerId] : false);
  const named = who && who !== "unknown";

  return (
    <div className="act-offer">
      <div className="act-offer-who">
      <Avatar name={named ? who : "?"} id={tx.peerId} size={48} ring="in" />
      <div className="grow" style={{ minWidth: 0 }}>
        <div className="act-offer-title">
          {named ? t("app.offerWaitingFrom", who) : t("app.offerWaiting")}
        </div>
        <div className="act-offer-sub tnum truncate">
          {[
            fmtBytes(tx.size),
            // Your own machine shares your key, so neither word applies to it:
            // there is nothing to verify and nothing to look up in a book.
            mine
              ? t("exchange.deviceOfYours")
              : known
                ? tx.verified
                  ? t("exchange.verified")
                  : t("exchange.unverified")
                : t("activity.notInBook"),
          ].join(" · ")}
        </div>
      </div>
      </div>
      <Button variant="in" onClick={() => openIncoming(tx.offerId!)}>
        {t("activity.examine")}
      </Button>
    </div>
  );
}

function Row({ tx }: { tx: UITransfer }) {
  const t = useT();
  const pause = useStore((s) => s.pause);
  const resume = useStore((s) => s.resume);
  const cancel = useStore((s) => s.cancel);
  const depositNow = useStore((s) => s.depositNow);
  const removeRow = useStore((s) => s.removeRow);
  const openShare = useStore((s) => s.openShare);
  const push = useStore((s) => s.push);
  const peerLabel = useStore((s) => s.peerLabel);
  /** Held down: everything this row can do, with its consequences (the design's
   *  7c). The swipe keeps the two that are worth a gesture. */
  const [held, setHeld] = useState(false);

  const moving = isMoving(tx);
  const done = tx.status === "completed";
  const who = tx.peer || peerLabel(tx.peerId ?? null, tx.senderName);

  // The two things a finger does to a row, and neither of them is a menu: hold
  // what is moving, forget what is finished.
  const actions: SwipeAction[] = moving
    ? [
        { label: t("transfers.pause"), run: () => fire(pause(tx.id)) },
        { label: t("transfers.cancel"), danger: true, run: () => fire(cancel(tx.id)) },
      ]
    : tx.status === "paused"
      ? [
          { label: t("transfers.resume"), run: () => fire(resume(tx.id)) },
          { label: t("transfers.cancel"), danger: true, run: () => fire(cancel(tx.id)) },
        ]
      : [
          {
            label: t("transfers.removeRow"),
            danger: true,
            run: () => fire(removeRow(tx.key)),
          },
        ];

  // Not the window's list, and the differences are the platform's rather than a
  // choice: there is no "show in the folder" (nothing reveals a file in Files),
  // and no confirmation step behind the cancel — this sheet already writes what
  // cancelling costs, and getting here took a deliberate hold.
  const actionItems: MenuItem[] = [];
  if (tx.status === "sharing")
    actionItems.push({
      key: "share",
      label: t("transfers.shareDetails"),
      icon: <Icon.Link size={15} />,
      onSelect: () => openShare(tx.id),
    });
  if (tx.status === "active")
    actionItems.push({
      key: "pause",
      label: t("transfers.pause"),
      icon: <Icon.Pause size={15} />,
      onSelect: () => fire(pause(tx.id)),
    });
  if (tx.status === "paused" || tx.status === "stalled")
    actionItems.push({
      key: "resume",
      label: t("transfers.resume"),
      icon: <Icon.Play size={15} />,
      onSelect: () => fire(resume(tx.id)),
    });
  if (tx.status === "offered")
    actionItems.push({
      key: "depositnow",
      label: t("transfers.depositNow"),
      desc: t("transfers.depositNowDesc"),
      icon: <Icon.Mailbox size={15} />,
      onSelect: () => fire(depositNow(tx.id)),
    });
  if (tx.code)
    actionItems.push({
      key: "code",
      label: t("codes.show"),
      icon: <Icon.Qr size={15} />,
      onSelect: () => push("codes"),
    });
  if (tx.path)
    actionItems.push({
      key: "openfile",
      label: t("transfers.openFile"),
      icon: <Icon.External size={15} />,
      onSelect: () =>
        api
          .openPath(tx.path!)
          .catch((e: unknown) => toast.bad(t("transfers.openFileFailed"), String(e))),
    });
  if (isLive(tx))
    actionItems.push({
      key: "cancel",
      label:
        tx.status === "deposited"
          ? t("transfers.revokeDeposit")
          : t("transfers.cancel"),
      desc: t("transfers.cancelDesc"),
      icon: <Icon.Stop size={15} />,
      danger: true,
      onSelect: () => fire(cancel(tx.id)),
    });
  if (isDone(tx))
    actionItems.push({
      key: "remove",
      label: t("transfers.removeRow"),
      desc: t("transfers.removeRowDesc"),
      icon: <Icon.Trash size={15} />,
      danger: true,
      onSelect: () => fire(removeRow(tx.key)),
    });

  /** The one thing on the right of a row, where the row has one. */
  const side = tx.code ? (
    /* A send answering to a code is the one row whose *point* is on another
       screen: the square somebody has to photograph. */
    <button className="act-side" onClick={() => push("codes")} aria-label={t("codes.show")}>
      <Icon.Qr size={16} />
    </button>
  ) : tx.status === "sharing" ? (
    <button
      className="act-side"
      onClick={() => openShare(tx.id)}
      aria-label={t("transfers.shareDetails")}
    >
      <Icon.Link size={16} />
    </button>
  ) : tx.status === "paused" ? (
    <button
      className="act-side"
      onClick={() => fire(resume(tx.id))}
      aria-label={t("transfers.resume")}
    >
      <Icon.Play size={16} />
    </button>
  ) : done && tx.path ? (
    /* A finished receive ends in the thing you actually wanted from it. */
    <button
      className="act-side"
      onClick={() =>
        api
          .openPath(tx.path!)
          .catch((e: unknown) => toast.bad(t("transfers.openFileFailed"), String(e)))
      }
      aria-label={t("transfers.openFile")}
    >
      <Icon.External size={16} />
    </button>
  ) : null;

  const state = statusMeta(tx.status);

  return (
    <SwipeRow actions={actions} onLongPress={() => setHeld(true)}>
      <ActionSheet
        open={held}
        onClose={() => setHeld(false)}
        title={tx.name}
        badge={{ text: state.text, tone: state.tone === "ok" ? "ok" : state.tone === "bad" ? "bad" : "warn" }}
        meta={[
          who && who !== "unknown"
            ? tx.dir === "in"
              ? t("activity.from", who)
              : t("activity.to", who)
            : "",
          tx.size > 0
            ? t("activity.ofSize", fmtBytes(tx.transferred), fmtBytes(tx.size))
            : "",
        ]
          .filter(Boolean)
          .join(" · ")}
        note={metaLine(tx)}
        items={actionItems}
      />
      <div className="act-row">
        <div className="act-top">
          <span className={`act-glyph tone-${tx.dir}`}>
            {done ? (
              <Icon.Check size={17} />
            ) : tx.dir === "in" ? (
              <Icon.Receive size={17} />
            ) : (
              <Icon.Send size={17} />
            )}
          </span>
          <span className="act-name truncate">{tx.name}</span>
          {/* La percentuale sta qui, in colonna, non nella riga sotto: è un
              numero, e un numero in mezzo a una frase si legge due volte. */}
          {moving ? <span className="act-pct tnum">{pct(tx)}%</span> : null}
          {side}
        </div>
        {/* E la frase sotto compare solo quando ha qualcosa da dire che la
            percentuale e la barra non dicono già: perché è ferma, perché
            aspetta, com'è andata. Una riga che dice "62%" sopra una barra al
            62% è la stessa cosa detta tre volte. */}
        {moving && !tx.reason && !tx.waitingCode ? null : (
          <Meta tx={tx} who={who} />
        )}
        {moving || tx.status === "paused" ? <Progress t={tx} /> : null}
      </div>
    </SwipeRow>
  );
}

/** A heading with the one thing that can be done to the whole of what is under
 *  it. Both of them are plural verbs — hold everything, forget everything — and
 *  on this platform that belongs beside the label rather than in a toolbar. */
function Head({
  label,
  action,
  onAction,
}: {
  label: string;
  action?: string;
  onAction?: () => void;
}) {
  return (
    <div className="act-head-row">
      <span className="t-label">{label}</span>
      {action && onAction ? (
        <button className="act-head-act" onClick={onAction}>
          {action}
        </button>
      ) : null}
    </div>
  );
}

export function ActivityView() {
  const t = useT();
  const transfers = useStore((s) => s.transfers);
  const push = useStore((s) => s.push);
  const go = useStore((s) => s.go);
  const search = useStore((s) => s.search);
  const setSearch = useStore((s) => s.setSearch);
  const togglePauseAll = useStore((s) => s.togglePauseAll);
  const pauseAll = useStore((s) => s.pauseAll);
  const clearFinished = useStore((s) => s.clearFinished);

  /** Now, or the record of it. The same screen at two times — see the file head. */
  const [half, setHalf] = useState<"now" | "left" | "log">("now");
  const [finding, setFinding] = useState(false);
  /** Il già fatto, srotolato. Chiuso finché non lo si chiede. */
  const [openDone, setOpenDone] = useState(false);

  // The log is a fetch, not an event stream: nothing pushes it, so it is only as
  // fresh as the last time somebody asked. Arriving at a *route* is where the
  // store asks (see `arrive`), and this is not a route any more — it is half of
  // one — so the ask moved here, to the moment the half is chosen.
  const loadHistory = useStore((s) => s.loadHistory);
  useEffect(() => {
    if (half === "log") void loadHistory().catch(() => {});
  }, [half, loadHistory]);

  const { offers, live, today } = useMemo(() => {
    const q = search.trim().toLowerCase();
    const match = (tx: UITransfer) =>
      !q ||
      tx.name.toLowerCase().includes(q) ||
      (tx.peer || "").toLowerCase().includes(q);
    const rows = Object.values(transfers).filter(match);
    const byNewest = (a: UITransfer, b: UITransfer) => b.rank - a.rank;
    const todayOnly = (tx: UITransfer) => isDone(tx) && isToday(tx.firstSeen);
    return {
      offers: rows.filter((r) => r.status === "incoming").sort(byNewest),
      live: rows.filter(isLive).sort(byNewest),
      today: rows.filter(todayOnly).sort(byNewest),
    };
  }, [transfers, search]);

  const nothing = !offers.length && !live.length && !today.length;

  return (
    <div className="ios-page">
      {/* The screen's own head. The large title is drawn here rather than by the
          frame because the two glyphs belong beside it: the tray is the way to
          what is still sitting on a relay, and the magnifier is the only way to
          search on a phone — there is no field in the bar to put it in. */}
      <div className="act-head">
        <h2 className="large-title">{t("title.activity")}</h2>
        <div className="act-head-tools">
          <button
            className="act-tool"
            aria-label={t("app.searchLabel")}
            aria-pressed={finding}
            onClick={() => {
              if (finding) setSearch("");
              setFinding((v) => !v);
            }}
          >
            <Icon.Search size={19} />
          </button>
          {/* L'ingranaggio: dove Impostazioni abita dalla sezione 12. */}
          <button
            className="act-tool"
            aria-label={t("title.settings")}
            onClick={() => push("settings")}
          >
            <Icon.Settings size={19} />
          </button>
        </div>
      </div>

      <Segmented
        block
        label={t("activity.halfLabel")}
        value={half}
        onChange={setHalf}
        options={[
          { value: "now" as const, label: t("activity.now") },
          // Tre tempi dello stesso invio (12a): adesso, lasciato, finito.
          { value: "left" as const, label: t("activity.left") },
          { value: "log" as const, label: t("title.history") },
        ]}
      />

      {finding && (
        <TextInput
          autoFocus
          value={search}
          onChange={(e) => setSearch(e.currentTarget.value)}
          placeholder={t("app.searchPlaceholder")}
          aria-label={t("app.searchLabel")}
        />
      )}

      {half === "left" ? (
        <LeftHalf query={search} />
      ) : half === "log" ? (
        <HistoryView query={search} />
      ) : nothing ? (
        /* What will appear here, and the two gestures that make it appear —
           stacked, full width and in the middle of the screen, which is where
           the design of record puts them (7q). No card around it: a card is a
           group of things, and there is nothing to group. */
        <div className="act-empty">
          <Empty
            icon={<Icon.Activity size={26} />}
            title={search.trim() ? t("activity.emptyNoMatch") : t("activity.emptyTitle")}
            action={
              search.trim() ? undefined : (
                /* Un bottone solo, e porta a Scambia.
                   Ce n'erano due: «Inquadra un codice», che apriva Scambia, e
                   «Invia qualcosa», che apriva il **pannello della finestra** —
                   quello con «A chi va» e le cinque pastiglie. Era l'ultima
                   porta rimasta a un invio che su questa piattaforma non esiste
                   più: Scambia è il posto da cui si parte, e ci si arriva a
                   rispondere *con chi* prima di *cosa*. E poiché tutti e due i
                   bottoni finiscono lì, uno dei due era la stessa porta
                   disegnata due volte. */
                <div className="empty-actions">
                  <Button size="lg" className="btn-ink" onClick={() => go("exchange")}>
                    <Icon.Scan size={15} /> {t("activity.emptyGo")}
                  </Button>
                </div>
              )
            }
          >
            {search.trim() ? undefined : t("activity.emptyBody")}
          </Empty>
        </div>
      ) : (
        <>
          {offers.map((tx) => (
            <Offer key={tx.key} tx={tx} />
          ))}

          {live.length > 0 && (
            <div>
              <Head
                label={t("activity.inFlight")}
                action={pauseAll ? t("activity.resumeAll") : t("activity.pauseAll")}
                onAction={() => fire(togglePauseAll())}
              />
              <div className="ios-group">
                {live.map((tx) => (
                  <Row key={tx.key} tx={tx} />
                ))}
              </div>
            </div>
          )}

          {/* Quello che è già finito: una riga che si apre, non una lista.
              Su un telefono usato da due settimane la parte "oggi" *era* la
              schermata, e quello che stava ancora succedendo finiva sotto il
              bordo — contato, qui, invece che srotolato. */}
          {today.length > 0 &&
            (openDone ? (
              <div>
                <Head
                  label={t("section.today")}
                  action={t("app.clearFinished", today.length)}
                  onAction={() => fire(clearFinished())}
                />
                <div className="ios-group">
                  {today.map((tx) => (
                    <Row key={tx.key} tx={tx} />
                  ))}
                </div>
              </div>
            ) : (
              <button className="act-done-row" onClick={() => setOpenDone(true)}>
                <span className="ico">
                  <Icon.Check size={18} />
                </span>
                <span className="grow tnum truncate">
                  {/* «Completato» solo se lo è davvero, e ogni altra uscita
                      chiamata con il suo nome. Qui c'era un solo contatore —
                      tutto ciò che non era completato né depositato — stampato
                      come «non riusciti»: un invio **annullato** ci finiva
                      dentro, ed è finito per scelta, non per un guasto. E un
                      invio **depositato** veniva contato fra i completati, che
                      è l'errore opposto: sta in casella e aspetta che lo
                      ritirino, quindi non è finito affatto. */}
                  {today.every((x) => x.status === "completed")
                    ? t("activity.doneToday", today.length)
                    : t(
                        "activity.endedToday",
                        today.length,
                        today.filter((x) => x.status === "failed").length,
                        today.filter((x) => x.status === "cancelled").length,
                        today.filter((x) => x.status === "deposited").length
                      )}
                </span>
                <Icon.ChevronRight size={16} className="ico" />
              </button>
            ))}
        </>
      )}
    </div>
  );
}


/* -- Lasciati (12a) ---------------------------------------------------------
   Link e depositi sono invii ancora vivi, non preferenze: la voce di mezzo di
   Attività, fra quello che sta passando adesso e quello che è finito. Due
   gruppi — i link pubblici, e quello che aspetta in una casella — e un tocco
   su una riga per il link, la revoca o i dettagli. */
function LeftHalf({ query }: { query: string }) {
  const t = useT();
  const deposits = useStore((s) => s.deposits);
  const loading = useStore((s) => s.depositsLoading);
  const error = useStore((s) => s.depositsError);
  const loadDeposits = useStore((s) => s.loadDeposits);
  const peerLabel = useStore((s) => s.peerLabel);
  const startCompose = useStore((s) => s.startCompose);

  // Nothing pushes this list: the relay is asked when the half is chosen.
  useEffect(() => {
    void loadDeposits().catch(() => {});
  }, [loadDeposits]);

  const q = query.trim().toLowerCase();
  const shown = deposits.filter((d) => !q || d.name.toLowerCase().includes(q));
  const links = shown.filter((d) => d.kind === "link");
  const sealed = shown.filter((d) => d.kind !== "link");

  const row = (d: DepositDto) => (
    <LeftRow key={d.id} d={d} who={d.recipient ? peerLabel(d.recipient) : ""} />
  );

  return (
    <>
      <div className="left-intro">{t("activity.leftIntro")}</div>

      {/* Il link si crea dove si guardano i link, **in cima**.
          Stava in fondo, dopo tutta la lista: in una rubrica di depositi di
          qualche settimana significava trovarlo solo scorrendo fino alla fine,
          nella striscia fra l'isola della barra e il bordo dello schermo, che è
          il posto meno visibile che ci sia. In cima è la prima cosa dopo la
          frase che dice cos'è questa mezza schermata — e la domanda che ci si fa
          arrivandoci è proprio «ne faccio un altro». */}
      <div className="ios-group">
        <button className="ios-row action" onClick={() => startCompose({ kind: "link" })}>
          <Icon.Plus size={16} />
          {t("activity.leftNew")}
        </button>
      </div>
      {error && <div className="card card-pad card-err t-sm">{error}</div>}
      {links.length > 0 && (
        <div>
          <div className="ios-head">{t("deposits.sectionLinks")}</div>
          <div className="ios-group">{links.map(row)}</div>
        </div>
      )}
      {sealed.length > 0 && (
        <div>
          <div className="ios-head">{t("activity.leftSealed")}</div>
          <div className="ios-group">{sealed.map(row)}</div>
        </div>
      )}
      {shown.length === 0 && !loading && (
        <div className="act-empty">
          <Empty icon={<Icon.Link size={26} />} title={t("activity.leftNone")} />
        </div>
      )}

    </>
  );
}

function LeftRow({ d, who }: { d: DepositDto; who: string }) {
  const t = useT();
  const revoke = useStore((s) => s.revokeDeposit);
  const [open, setOpen] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const { copy } = useCopy();
  const m = depositMeta(d);
  const isLink = d.kind === "link";
  const handover = isLink ? d.link : d.ticket;
  const tone = m.tone === "ok" ? "ok" : m.tone === "bad" ? "bad" : "warn";

  const items: MenuItem[] = [];
  if (handover) {
    items.push({
      key: "copy",
      label: t(isLink ? "compose.copyLink" : "deposits.shareTicket"),
      icon: <Icon.Copy size={16} />,
      onSelect: () => {
        copy(handover);
        toast.echo(t("common.copied"));
      },
    });
  }
  if (isLink && d.link) {
    items.push({
      key: "open",
      label: t("activity.openLink"),
      icon: <Icon.External size={16} />,
      onSelect: () => {
        openUrl(d.link).catch((e: unknown) => toast.bad(t("deposits.openFailed"), String(e)));
      },
    });
  }
  items.push({
    key: "revoke",
    label: m.revocable ? t("deposits.revoke") : t("common.remove"),
    desc: m.revocable
      ? t(isLink ? "deposits.confirmRevokeLink" : "deposits.confirmRevokeSealed")
      : undefined,
    icon: <Icon.Trash size={16} />,
    danger: true,
    onSelect: () => setConfirm(true),
  });

  return (
    <>
      <button className="big-row" onClick={() => setOpen(true)}>
        <span className={`row-glyph${isLink ? "" : " out"}`}>
          {isLink ? <Icon.Link size={20} /> : <Icon.Mailbox size={20} />}
        </span>
        <span className="grow" style={{ minWidth: 0 }}>
          <span className="big-row-title truncate">{d.name}</span>
          <span className="big-row-sub state">
            <span className={`badge ${tone === "warn" ? "" : tone}`}>{m.text}</span>
            <span className="truncate">{isLink ? m.detail : who ? t("activity.leftFor", who) : m.detail}</span>
          </span>
        </span>
        <Icon.ChevronRight size={16} className="ios-chev" />
      </button>
      <ActionSheet
        open={open}
        onClose={() => setOpen(false)}
        title={d.name}
        badge={{ text: m.text, tone }}
        meta={[fmtBytes(d.size), m.detail].filter(Boolean).join(" · ")}
        items={items}
      />
      <Confirm
        open={confirm}
        title={m.revocable ? t("deposits.confirmRevokeTitle") : t("deposits.confirmRemoveTitle")}
        body={
          m.revocable
            ? isLink
              ? t("deposits.confirmRevokeLink")
              : t("deposits.confirmRevokeSealed")
            : t("deposits.confirmRemoveBody")
        }
        confirmLabel={m.revocable ? t("deposits.revoke") : t("common.remove")}
        danger
        onCancel={() => setConfirm(false)}
        onConfirm={() => {
          setConfirm(false);
          fire(revoke(d.id));
        }}
      />
    </>
  );
}
