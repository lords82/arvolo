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
import { useStore } from "../store";
import type { UITransfer } from "../types";
import { Avatar } from "../ui/Bits";
import { Icon } from "../ui/Icons";
import { Empty } from "../ui/Primitives";

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
    // An anonymous share or a link has no correspondent. It is not "nobody" —
    // it is a different kind of thing, and it gets its own row below.
    const key = r.peerId || r.peer || "";
    if (!key) continue;
    const t: Thread = by.get(key) ?? {
      key,
      who: r.peer || shortId(key),
      peerId: r.peerId,
      rows: [],
    };
    t.rows.push(r);
    by.set(key, t);
  }
  const threads = [...by.values()];
  for (const t of threads) t.rows.sort((a, b) => b.firstSeen - a.firstSeen);
  threads.sort((a, b) => (b.rows[0]?.firstSeen ?? 0) - (a.rows[0]?.firstSeen ?? 0));
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

function ThreadRow({ thread }: { thread: Thread }) {
  const t = useT();
  const open = useStore((s) => s.openThread);
  const latest = thread.rows[0];
  const busy =
    latest &&
    (latest.status === "active" ||
      latest.status === "preparing" ||
      latest.status === "stalled");
  // Only what the line above does not already say. A deposited send that nobody
  // has picked up yet is the case the design draws a badge for; repeating the
  // status the line just gave is not.
  const parked = latest?.offerStatus === "pending" || latest?.offerStatus === "arrived";

  return (
    <button className="cs-row" onClick={() => open(thread.key)}>
      <Avatar name={thread.who} id={thread.peerId} size={36} ring={ring(latest)} />
      <span className="cs-body">
        <span className="cs-head">
          <span className="truncate cs-who">{thread.who}</span>
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
    </button>
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
    <div className="casella">
      {threads.map((th) => (
        <ThreadRow key={th.key} thread={th} />
      ))}

      {/* A public link belongs to nobody, so it cannot be a correspondent. It is
          still something that happened and can still be revoked, so it is here
          rather than only two taps away under Settings. */}
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

      <div className="t-xs t-mut cs-hint">{t("casella.rowHint")}</div>
    </div>
  );
}
