// Everything that has passed between you and one person.
//
// The Casella answers "who, and what last"; this answers the rest. It is a full
// screen rather than a dialog because on a phone a dialog over a list is two
// things fighting for one small rectangle — and because what is in here is not a
// question to dismiss, it is a place to be.
//
// Three parts, in the order they matter: what is happening now (which is the only
// thing with buttons), what happened before, and the one thing you might want to
// do next, which is send them something back.

import { useT } from "../i18n";
import { barClass, fmtAgo, fmtBytes, pct, statusMeta } from "../format";
import { fire, useStore } from "../store";
import type { UITransfer } from "../types";
import { threadsOf } from "../views/CasellaView";
import { Avatar } from "../ui/Bits";
import { Icon } from "../ui/Icons";
import { Button, IconButton } from "../ui/Primitives";

/** Is this the row the thread leads with — something arriving, or a question? */
function isNow(tx: UITransfer): boolean {
  return (
    tx.status === "incoming" ||
    tx.status === "active" ||
    tx.status === "preparing" ||
    tx.status === "offered" ||
    tx.status === "stalled" ||
    tx.status === "paused"
  );
}

export function ThreadSheet() {
  const t = useT();
  const key = useStore((s) => s.threadKey);
  const close = useStore((s) => s.closeThread);
  const transfers = useStore((s) => s.transfers);
  const openIncoming = useStore((s) => s.openIncoming);
  const openSheet = useStore((s) => s.openSheet);
  const reject = useStore((s) => s.reject);

  if (!key) return null;
  const thread = threadsOf(Object.values(transfers)).find((x) => x.key === key);
  if (!thread) return null;

  const now = thread.rows.filter(isNow);
  const before = thread.rows.filter((r) => !isNow(r));
  const verified = thread.rows.some((r) => r.verified);

  return (
    <div className="sheet-full" role="dialog" aria-label={thread.who}>
      <div className="sheet-full-bar">
        <IconButton label={t("common.close")} onClick={close}>
          <Icon.Close size={16} />
        </IconButton>
        <Avatar name={thread.who} id={thread.peerId} size={36} />
        <span className="grow" style={{ minWidth: 0 }}>
          <span className="truncate" style={{ display: "block", fontWeight: 600 }}>
            {thread.who}
          </span>
          <span className="t-xs t-mut">
            {verified ? t("thread.verified") : t("thread.unverified")}
          </span>
        </span>
      </div>

      <div className="sheet-full-body">
        {now.map((tx) => (
          <div className="card card-pad stack" key={tx.key}>
            <div className="section-head">
              <span className="t-head grow">{t("thread.now")}</span>
              <span className="badge">{fmtBytes(tx.size)}</span>
            </div>
            <div className="truncate" style={{ fontWeight: 600 }}>
              {tx.name}
            </div>
            <div className="t-xs t-mut">{statusMeta(tx.status).text}</div>
            {tx.status !== "incoming" && (
              <span className={barClass(tx)}>
                <span style={{ width: `${pct(tx)}%` }} />
              </span>
            )}
            {/* An offer is the one row here with a decision on it. Accepting goes
                through the same dialog the desktop uses — it is where the
                password field and the "save them as a contact" line live, and a
                second way of accepting would be a second thing to keep right. */}
            {tx.status === "incoming" && tx.offerId && (
              <div style={{ display: "flex", gap: 8 }}>
                <Button
                  variant="in"
                  block
                  onClick={() => openIncoming(tx.offerId!)}
                >
                  {t("thread.answer")}
                </Button>
                <Button block onClick={() => fire(reject(tx.offerId!))}>
                  {t("incoming.reject")}
                </Button>
              </div>
            )}
          </div>
        ))}

        {before.length > 0 && (
          <>
            <div className="t-label">{t("thread.before")}</div>
            <div className="card">
              {before.map((tx) => (
                <div className="th-past" key={tx.key}>
                  <span className="grow" style={{ minWidth: 0 }}>
                    <span className="truncate" style={{ display: "block", fontWeight: 600, fontSize: 13.5 }}>
                      {tx.name}
                    </span>
                    <span className="t-xs t-mut">
                      {tx.dir === "in" ? "↓" : "↑"} {fmtBytes(tx.size)} ·{" "}
                      {fmtAgo(tx.firstSeen / 1000)}
                    </span>
                  </span>
                  <span className={`badge ${tx.status === "completed" ? "ok" : ""}`}>
                    {statusMeta(tx.status).text}
                  </span>
                </div>
              ))}
            </div>
          </>
        )}

        <div className="grow" />
        <Button
          variant="primary"
          block
          onClick={() => {
            close();
            openSheet([], thread.peerId || thread.who);
          }}
        >
          <Icon.Send size={14} />
          {t("thread.sendBack", thread.who)}
        </Button>
      </div>
    </div>
  );
}
