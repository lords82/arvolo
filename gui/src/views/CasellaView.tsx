// The phone's home: people, not files.
//
// A desktop shows two columns of transfers because a desktop has room for two
// columns and a person sitting in front of one is watching a job run. A phone is
// picked up to answer a question — *did it arrive, and who from* — and the answer
// is a short list of the people you have been dealing with, each saying what the
// last thing between you was.
//
// So this is one row per correspondent, newest first, and the file is the
// subtitle. The rows the desktop draws are all still there, one tap in: opening a
// row opens everything that has passed between you and that person.
//
// The design of record ("Arvolo Mobile 1c — Casella") is what this follows.

import { useT } from "../i18n";
import { fmtAgo, fmtBytes, pct, barClass, statusMeta, shortId } from "../format";
import { fire, useStore } from "../store";
import type { UITransfer } from "../types";
import { Avatar } from "../ui/Bits";
import { Icon } from "../ui/Icons";
import { Empty } from "../ui/Primitives";
import { SwipeRow, type SwipeAction } from "../ui/SwipeRow";
import { api } from "../ipc";

/** Where a transfer goes while nobody has said who it is with. */
export const UNKNOWN = "\u0000unknown";

/** Everything that has passed between us and one correspondent. */
export interface Thread {
  /** Group key: the peer's id when there is one, else the label. */
  key: string;
  /** Who, as a person reads it. */
  who: string;
  /** Their public id, when the row has one. */
  peerId?: string;
  /** Newest first. */
  rows: UITransfer[];
}

/** Group transfers by who they are with, newest first inside and out.
 *
 * Exported for its test: the grouping *is* the screen, and it is the one part of
 * it that can be wrong in a way a screenshot would not show — two spellings of
 * the same person, or a row filed under nobody. */
export function threadsOf(rows: UITransfer[]): Thread[] {
  const by = new Map<string, Thread>();
  for (const r of rows) {
    // A share and a link have no correspondent by construction: nobody was sent
    // them, and they get their own row below rather than a thread.
    if (r.status === "sharing") continue;
    // Everything else without one is a transfer whose sender is not known *yet*
    // — a row built from events before the first snapshot carries an id, a name
    // and a size and nothing about who. Filing it under "unknown" keeps it on
    // screen until the snapshot says; dropping it made an accepted file vanish
    // from the list it was accepted in.
    const key = r.peerId || r.peer || UNKNOWN;
    const t: Thread = by.get(key) ?? {
      key,
      who: r.peer || (key === UNKNOWN ? "" : shortId(key)),
      peerId: r.peerId,
      rows: [],
    };
    t.rows.push(r);
    by.set(key, t);
  }
  const threads = [...by.values()];
  for (const t of threads) t.rows.sort((a, b) => b.firstSeen - a.firstSeen);
  // Arriving first, then the newest.
  //
  // Recency alone is the right order for a list of conversations, and the wrong
  // one for a list that mixes something waiting on you with something you have
  // already dealt with: a file being sent *to* you can need a decision, and a
  // decision does not belong under a record of what you sent this morning. So
  // direction is the first key and time is the second — which leaves the order
  // inside each half exactly as it was.
  threads.sort((a, b) => {
    const arriving = (t: Thread) => (t.rows[0]?.dir === "in" ? 0 : 1);
    return (
      arriving(a) - arriving(b) ||
      (b.rows[0]?.firstSeen ?? 0) - (a.rows[0]?.firstSeen ?? 0)
    );
  });
  return threads;
}

/** The ring around an avatar: which way something is moving, right now. */
function ring(tx: UITransfer | undefined): "in" | "out" | undefined {
  if (!tx) return undefined;
  const moving =
    tx.status === "active" || tx.status === "incoming" || tx.status === "stalled";
  if (!moving) return undefined;
  return tx.dir === "in" ? "in" : "out";
}

function Line({ tx }: { tx: UITransfer }) {
  const t = useT();
  const meta = statusMeta(tx.status);
  return (
    <span className="t-xs t-mut truncate cs-line">
      {tx.dir === "in" ? "↓" : "↑"} {tx.name} · {fmtBytes(tx.size)} ·{" "}
      {tx.status === "completed" && tx.dir === "in"
        ? t("casella.saved")
        : meta.text}
    </span>
  );
}

/** What you would actually do to this row, in the state it is in.
 *
 * One action, chosen by the row rather than offered as a list: a transfer that
 * is running can be held, one that has arrived can be opened, and a deposit
 * nobody has collected can be taken back. Anything else has no obvious verb, and
 * a swipe that reveals a verb you did not want is worse than a swipe that
 * reveals nothing. */
function actionsFor(
  tx: UITransfer | undefined,
  t: ReturnType<typeof useT>
): SwipeAction[] {
  const pause = useStore.getState().pause;
  const resume = useStore.getState().resume;
  const cancel = useStore.getState().cancel;
  if (!tx) return [];
  // Something moving has two answers and they are different answers: hold it,
  // meaning come back to it — and give it up, meaning do not. Offering only the
  // first left the second a trip into the row, into the person, into a menu.
  if (tx.canPause && (tx.status === "active" || tx.status === "stalled")) {
    return [
      { label: t("row.pause"), run: () => fire(pause(tx.id)) },
      { label: t("row.cancel"), danger: true, run: () => fire(cancel(tx.id)) },
    ];
  }
  if (tx.status === "paused") {
    return [
      { label: t("row.resume"), run: () => fire(resume(tx.id)) },
      { label: t("row.cancel"), danger: true, run: () => fire(cancel(tx.id)) },
    ];
  }
  if (tx.dir === "out" && tx.offerStatus === "pending") {
    return [
      { label: t("row.takeBack"), danger: true, run: () => fire(cancel(tx.id)) },
    ];
  }
  // Anything that is over. Every one of these rows used to return nothing, and
  // a row that reveals nothing is indistinguishable from a gesture that is not
  // implemented — which is what a mailbox full of finished sends looked like:
  // the swipe was "broken" everywhere, having been written and working.
  //
  // Removing is the verb this platform puts on the end of every row of a list
  // of things that have happened, and it is the one that is always true. What
  // arrived also opens, and that goes first because it is what you swiped for.
  if (
    tx.status === "completed" ||
    tx.status === "failed" ||
    tx.status === "cancelled"
  ) {
    const removeRow = useStore.getState().removeRow;
    const open =
      tx.dir === "in" && tx.path
        ? [{ label: t("common.open"), run: () => void api.openPath(tx.path!) }]
        : [];
    return [
      ...open,
      {
        label: t("row.remove"),
        danger: true,
        run: () => fire(removeRow(`t${tx.id}`)),
      },
    ];
  }
  return [];
}

function ThreadRow({ thread }: { thread: Thread }) {
  const t = useT();
  const open = useStore((s) => s.openThread);
  const latest = thread.rows[0];
  // What has a *proportion* to draw. Preparing has none — nothing has left this
  // machine yet — and an empty bar sitting there for a minute is exactly the
  // "stuck at zero" reading that state exists to avoid; the line above it says
  // what is happening instead.
  const busy =
    latest && (latest.status === "active" || latest.status === "stalled");
  // Only what the line above does not already say. A deposited send that nobody
  // has picked up yet is the case the design draws a badge for; repeating the
  // status the line just gave is not.
  const parked = latest?.offerStatus === "pending" || latest?.offerStatus === "arrived";

  return (
    <SwipeRow actions={actionsFor(latest, t)}>
    <button className="cs-row" onClick={() => open(thread.key)}>
      <Avatar name={thread.who} id={thread.peerId} size={36} ring={ring(latest)} />
      <span className="cs-body">
        <span className="cs-head">
          {/* The name and its count are one thing and have to sit together:
              left to itself the name took the whole line and pushed the number
              against the clock, where "2  21 minuti fa" reads as a time and not
              as a count. This platform puts it immediately after the name, and
              the name is what truncates when there is no room. */}
          <span className="cs-name">
            <span className="truncate cs-who">
              {thread.who || t("casella.unknownWho")}
            </span>
          {/* How many things are in here, when it is more than one.
              A mailbox row shows the latest and only the latest, which is right
              — and leaves no way to tell one file from five. Two files sent one
              after the other produced a single row naming the second, and the
              first was nowhere on the screen it had arrived on: not lost, not
              late, just silent. A conversation says how long it is; this is the
              same number, in the same place this platform puts it. */}
            {thread.rows.length > 1 && (
              <span className="cs-count">{thread.rows.length}</span>
            )}
          </span>
          <span className="t-xs t-mut">{fmtAgo(latest.firstSeen / 1000)}</span>
        </span>
        <Line tx={latest} />
        {busy && (
          <span className={`${barClass(latest)} cs-track`}>
            <span style={{ width: `${pct(latest)}%` }} />
          </span>
        )}
        {parked && (
          <span className="cs-badges">
            <span className="badge warn">{t("casella.notTakenYet")}</span>
          </span>
        )}
      </span>
      <Icon.ChevronRight size={15} className="ios-chev" />
    </button>
    </SwipeRow>
  );
}

export function CasellaView() {
  const t = useT();
  const transfers = useStore((s) => s.transfers);
  const deposits = useStore((s) => s.deposits);
  const go = useStore((s) => s.go);
  const openSheet = useStore((s) => s.openSheet);

  const threads = threadsOf(Object.values(transfers));
  const links = deposits.filter((d) => d.kind === "link");

  if (!threads.length && !links.length) {
    return (
      <div className="view-narrow" style={{ paddingTop: 24 }}>
        <div className="card">
          <Empty
            icon={<Icon.Send size={22} />}
            title={t("transfers.firstRunTitlePhone")}
            action={
              <button className="btn btn-primary" onClick={() => openSheet([])}>
                <Icon.Send size={14} />
                {t("transfers.firstRunAction")}
              </button>
            }
          >
            {t("transfers.firstRunBodyPhone")}
          </Empty>
        </div>
      </div>
    );
  }

  return (
    <div className="ios-page casella">
      <div className="ios-group">
        {threads.map((th) => (
          <ThreadRow key={th.key} thread={th} />
        ))}
      </div>

      {/* A public link belongs to nobody, so it cannot be a correspondent. It is
          still something that happened and can still be revoked, so it is here
          rather than only two taps away under Settings — its own group, because
          it is a different kind of thing and this platform says that with a gap
          rather than with a label. */}
      {links.length > 0 && (
      <div className="ios-group">
      {links.map((d) => (
        <button
          key={d.id}
          className="cs-row"
          onClick={() => go("deposits")}
          title={t("casella.openLinks")}
        >
          <span className="cs-glyph">
            <Icon.Link size={16} />
          </span>
          <span className="cs-body">
            <span className="cs-head">
              <span className="truncate cs-who">{t("casella.publicLink")}</span>
              <span className="t-xs t-mut">{fmtAgo(d.created)}</span>
            </span>
            <span className="t-xs t-mut truncate cs-line">{d.name}</span>
          </span>
        </button>
      ))}
      </div>
      )}

      <div className="ios-foot">{t("casella.rowHint")}</div>
    </div>
  );
}
