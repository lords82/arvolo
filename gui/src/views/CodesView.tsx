// The code you are showing, on a screen of its own.
//
// A code exists to be handed over: held up to a camera across a table, or read
// out loud down a phone line. Both of those want the same thing — the square as
// big as the screen allows, and the words under it large enough to dictate — and
// neither of them wants a screen with a second half. So the half that read codes
// moved to *Ricevi*, where the camera already is, and what is left here is the
// showing.
//
// Nothing on this screen counts down. A code made by this app lasts for as long
// as the app is in front of you and its send is alive, and a timer would be a
// number invented to look precise: the daemon does not send an expiry for a live
// code, and what actually ends one is leaving the app or withdrawing it. Both of
// those are said in words instead.

import { fire, useStore } from "../store";
import { fmtBytes } from "../format";
import { useT } from "../i18n";
import { CodeHero } from "../ui/Bits";
import { Icon } from "../ui/Icons";
import { Button, Empty } from "../ui/Primitives";

export function CodesView() {
  const t = useT();
  const transfers = useStore((s) => s.transfers);
  const startCompose = useStore((s) => s.startCompose);
  const openShare = useStore((s) => s.openShare);
  const cancel = useStore((s) => s.cancel);

  // Every send answering to a code right now. Usually one; more than one is
  // legitimate and the list says so rather than picking a winner.
  const hosted = Object.values(transfers).filter((x) => x.code);

  if (!hosted.length) {
    return (
      <div className="ios-page">
        <div className="ios-group">
          <Empty
            icon={<Icon.Qr size={22} />}
            title={t("codes.noneTitle")}
            action={
              <Button
                variant="in"
                onClick={() => startCompose({ kind: "code" })}
              >
                <Icon.Send size={14} />
                {t("codes.hostAction")}
              </Button>
            }
          >
            {t("codes.noneBody")}
          </Empty>
        </div>
      </div>
    );
  }

  return (
    <div className="ios-page codes">
      {hosted.map((tx) => (
        <section key={tx.key} className="stack">
          <div className="ios-group code-card">
            <CodeHero
              value={tx.code!}
              qrSize={212}
              caption={t("codes.readItOut")}
            />
          </div>

          {/* What is happening, in words and without a clock. Nothing on this
              screen counts down: what ends a code is leaving the app or
              withdrawing it, and a timer would be a number invented to look
              precise. The dot is the same one the canvas draws — a state, not a
              progress bar, because there is no progress yet. */}
          <div className="code-state">
            <span className="code-dot" aria-hidden />
            {t("codes.waitingOther")}
          </div>

          <div className="ios-foot code-what tnum">
            {t("codes.bothOnline", fmtBytes(tx.size))}
          </div>

          <div className="ios-foot">{t("codes.pauseNote")}</div>

          {/* Withdrawing is what actually ends a code, so it is on the screen
              rather than behind a swipe on a row somewhere else — a live code is
              an open door, and the way to shut it belongs next to the thing that
              is open. Beside it, the other way to hand a code over: the system's
              own share sheet, for when the person is not in the room. */}
          <div className="phone-actions code-actions">
            <Button size="lg" onClick={() => openShare(tx.id)}>
              <Icon.Link size={15} />
              {t("common.share")}
            </Button>
            <Button variant="danger" size="lg" onClick={() => fire(cancel(tx.id))}>
              {t("codes.withdraw")}
            </Button>
          </div>
        </section>
      ))}
    </div>
  );
}
