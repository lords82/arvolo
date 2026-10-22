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
import { useBackDismiss } from "../backstack";
import { barClass, fmtAgo, fmtBytes, metaLine, pct, statusMeta } from "../format";
import { fire, useStore } from "../store";
import type { UITransfer } from "../types";
import { threadsOf } from "../threads";
import { api } from "../ipc";
import { Avatar, ExtChip } from "../ui/Bits";
import { Icon } from "../ui/Icons";
import { Button } from "../ui/Primitives";

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
  const startCompose = useStore((s) => s.startCompose);
  const contacts = useStore((s) => s.contacts);
  const reject = useStore((s) => s.reject);
  const pause = useStore((s) => s.pause);
  const resume = useStore((s) => s.resume);
  // Not a `Sheet`, so it registers for the back gesture itself — see
  // `backstack`. This is the screen the gesture is used on most.
  useBackDismiss(!!key, close);

  if (!key) return null;
  const thread = threadsOf(Object.values(transfers)).find((x) => x.key === key);
  if (!thread) return null;

  const now = thread.rows.filter(isNow);
  const before = thread.rows.filter((r) => !isNow(r));
  const verified = thread.rows.some((r) => r.verified);

  return (
    <div className="sheet-full" role="dialog" aria-label={thread.who}>
      {/* A bar, not a dialog's header: this is one screen in from the list, so
          the way out is the way out of any screen — a chevron and the name of
          where it goes, on the left, with the title between. Going through the
          history rather than straight to `closeThread` is what keeps this and
          the system's own back the same single step. */}
      <div className="sheet-full-bar">
        <button
          className="nav-back"
          onClick={() => (window.history.length > 1 ? window.history.back() : close())}
        >
          <Icon.ChevronLeft size={19} />
          <span className="truncate">{t("casella.title")}</span>
        </button>
        <h2 className="truncate">{thread.who}</h2>
        {/* Sending them something belongs in the bar, as this platform's compose
            button — not as a filled bar across the bottom of a screen you came
            to *read*. It was the loudest thing here, it repeated the "+" the
            app already has, and it named a person in a button, which is a
            sentence pretending to be a control. */}
        <span className="bar-pad">
          <button
            className="bar-btn"
            aria-label={t("thread.sendBack", thread.who)}
            onClick={() => {
              close();
              // **La schermata d'invio è una sola.** Da qui si apriva il
              // pannello della finestra — quello con «A chi va» e le cinque
              // pastiglie — che su un telefono è l'unica porta rimasta a un
              // modo di mandare che il canvas ha sostituito: una domanda («a
              // chi») già risposta, fatta di nuovo, e poi un'altra schermata
              // per rispondere a «cosa». Adesso porta in *Cosa* con la persona
              // già scelta, come ogni altro invio del telefono.
              // Un filo porta il peer come lo ha nominato il daemon: a volte è
              // il nome che gli hai dato tu, a volte l'id nudo di chi non è in
              // rubrica. *Cosa* cerca per nome, quindi qui si risolve finché
              // c'è qualcosa da risolvere — e se non c'è, l'id va bene lo
              // stesso: è un destinatario valido, solo senza faccia.
              {
                const peer = thread.peerId || thread.who;
                const known = contacts.find((c) => c.id === peer || c.name === peer);
                startCompose({ kind: "contact", name: known?.name ?? peer });
              }
            }}
          >
            <Icon.Plus size={19} />
          </button>
        </span>
      </div>

      <div className="sheet-full-body">
        {/* Who this is, said once and properly, where this platform says it:
            under the bar rather than squeezed into it. */}
        <div className="th-who">
          <Avatar name={thread.who} id={thread.peerId} size={56} />
          <span className="th-who-name truncate">{thread.who}</span>
          <span className="t-xs t-mut">
            {verified ? t("thread.verified") : t("thread.unverified")}
          </span>
        </div>

        {now.map((tx) => (
          <div className="card card-pad stack" key={tx.key}>
            <div className="th-now-head">
              <ExtChip name={tx.name} />
              <span className="grow" style={{ minWidth: 0 }}>
                <span className="truncate th-now-name">{tx.name}</span>
                {/* What is actually happening, in the words the board uses:
                    a percentage while it moves, with the rate and what is left
                    of it when the engine knows them. The old card said
                    "downloading" and drew a bar, which answers neither "how far"
                    nor "how long" — the two questions a person watching a
                    transfer is asking. */}
                <span className="th-now-meta">{metaLine(tx)}</span>
              </span>
            </div>
            {tx.status !== "incoming" && (
              <>
                <span className={barClass(tx)}>
                  <span style={{ width: `${pct(tx)}%` }} />
                </span>
                <div className="th-now-bytes">
                  <span className="tnum">{fmtBytes(tx.transferred)}</span>
                  <span className="t-mut"> / </span>
                  <span className="tnum t-mut">{fmtBytes(tx.size)}</span>
                  {/* One control, and the one that applies: hold what is
                      running, take back what nobody has collected. */}
                  {tx.canPause &&
                    (tx.status === "active" || tx.status === "stalled") && (
                      <button
                        className="th-now-act"
                        onClick={() => fire(pause(tx.id))}
                      >
                        {t("row.pause")}
                      </button>
                    )}
                  {tx.status === "paused" && (
                    <button
                      className="th-now-act"
                      onClick={() => fire(resume(tx.id))}
                    >
                      {t("row.resume")}
                    </button>
                  )}
                </div>
              </>
            )}
            {tx.status === "incoming" && (
              <div className="th-now-bytes">
                <span className="tnum t-mut">{fmtBytes(tx.size)}</span>
              </div>
            )}
            {/* An offer is the one row here with a decision on it. Accepting goes
                through the same dialog the desktop uses — it is where the
                password field and the "save them as a contact" line live, and a
                second way of accepting would be a second thing to keep right. */}
            {tx.status === "incoming" && tx.offerId && (
              /* Stacked, filled over quiet: two half-width buttons squeezed
                 side by side is a desktop dialog's row. This platform puts the
                 one thing it recommends on top, whole, and the way out under
                 it in the grey. */
              <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
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
                  {/* A file that arrived and is still here is a file you can
                      open. The badge said "completed" and stopped there, which
                      is a receipt where an action belongs. */}
                  {tx.status === "completed" && tx.dir === "in" && tx.path ? (
                    <button
                      className="th-past-act"
                      onClick={() => void api.openPath(tx.path!)}
                    >
                      {t("common.open")}
                    </button>
                  ) : (
                    <span className={`badge ${tx.status === "completed" ? "ok" : ""}`}>
                      {statusMeta(tx.status).text}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </>
        )}

      </div>
    </div>
  );
}
