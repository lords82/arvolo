// Everything you have left on a relay and can still take back.
//
// This list is the one place in the app that is *not* event-driven, and the
// distinction matters enough to be visible: nothing reports back when a link is
// downloaded or a sealed deposit collected, so the daemon has to go and ask the
// relay each time the panel is built. That means three states, not two — there,
// gone, and **could not ask** — and the third must never be rendered as either
// of the others. A one-shot link that has already been used, shown as "Attivo"
// because the relay was briefly unreachable, is exactly the failure this screen
// exists to prevent. `depositMeta` in format.ts owns that judgement.

import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { fire, useStore } from "../store";
import { useIsPhone } from "../phone";
import { SwipeRow } from "../ui/SwipeRow";
import { depositMeta, fmtBytes } from "../format";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import { Badge, Button, Empty, Segmented } from "../ui/Primitives";
import { CodeHero, ExtChip } from "../ui/Bits";
import { Confirm, Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import type { DepositDto } from "../types";

function DepositRow({ d }: { d: DepositDto }) {
  const t = useT();
  const revoke = useStore((s) => s.revokeDeposit);
  const revoking = useStore((s) => s.revoking.includes(d.id));
  const reload = useStore((s) => s.loadDeposits);
  const peerLabel = useStore((s) => s.peerLabel);
  const [confirm, setConfirm] = useState(false);
  const [share, setShare] = useState(false);
  const isPhone = useIsPhone();

  const m = depositMeta(d);
  const isLink = d.kind === "link";
  // What there is to hand over: a link's URL, or a sealed deposit's `arvm…`
  // ticket. One name for both, because from here they are the same act — the
  // string you pass to someone so they can take the file. Empty only for a
  // deposit made before tickets were kept: that one can still be withdrawn,
  // never handed on, and must therefore offer no button that says otherwise.
  const handover = isLink ? d.link : d.ticket;

  return (
    <>
      {/* Beside the name on a desktop, under it on a phone. A badge and two
          buttons in the third column leave a name three characters wide —
          which is what this row looked like on a phone: "re…", "Attiv". */}
      <SwipeRow
        actions={
          isPhone
            ? [
                {
                  label: m.revocable ? t("deposits.revoke") : t("common.remove"),
                  danger: true,
                  run: () => setConfirm(true),
                },
              ]
            : []
        }
      >
      <div
        className={`row ${isPhone ? "row-stacked" : ""}`}
        style={isPhone ? undefined : { gridTemplateColumns: "38px 1fr auto" }}
      >
        <ExtChip name={d.name} />
        <div className="row-main">
          <div className="row-name truncate" title={d.name}>
            {d.name}
          </div>
          <div className="row-meta">
            <span className={`tone-${m.tone}`} style={{ fontWeight: 600 }}>
              {m.text}
            </span>
            <span className="sep" />
            <span className="tnum">{fmtBytes(d.size)}</span>
            <span className="sep" />
            <span className="truncate">
              {isLink
                ? m.detail
                : t(
                    "deposits.sealedFor",
                    peerLabel(d.recipient || null),
                    m.detail
                  )}
            </span>
          </div>
        </div>
        <div className="hstack-sm dep-actions">
          <Badge kind={isLink ? "dev" : "info"}>
            {isLink ? (
              <>
                <Icon.Link size={10} /> {t("deposits.publicLink")}
              </>
            ) : (
              <>
                <Icon.Mailbox size={10} /> {t("deposits.sealed")}
              </>
            )}
          </Badge>
          {/* One button, not three. The address itself is not printed on the row
              either — an `arvm…` ticket is three hundred characters, and nobody
              reads either of these off a screen. So the row says only that there
              is something to hand over, and the panel behind it does the handing:
              copy, QR, open. Two buttons per row that both mean "give this away"
              only make the reader decide which one they meant.
              It says **which** of the two it is, though. Both were briefly labelled
              "Link", and the first thing that happened was a sealed deposit's
              ticket being copied by someone expecting a downloadable URL. There
              isn't one: that blob is HPKE-sealed to the recipient, so no browser
              — and no sender — can open it, and no `#fragment` could carry the
              key. A label that promises a URL where none can exist is worse than
              a longer word. */}
          {/* The third state — "the relay did not answer" — is the only one with
              something to *do* about it that is not destructive: ask again. It
              is offered here rather than as a refresh button over the whole
              list, because the doubt belongs to this row and the reader is
              looking at it. And the handover button is withheld while the doubt
              lasts: a QR for a file that may already be gone is exactly the
              confident-looking lie this screen exists to avoid. */}
          {d.present === null ? (
            <Button size="sm" onClick={() => fire(reload())}>
              <Icon.Refresh size={13} /> {t("deposits.askAgain")}
            </Button>
          ) : (
            handover && (
              <Button size="sm" onClick={() => setShare(true)}>
                <Icon.Link size={13} />{" "}
                {t(isLink ? "deposits.share" : "deposits.shareTicket")}
              </Button>
            )
          )}
          <Button
            size="sm"
            variant="danger"
            busy={revoking}
            disabled={revoking}
            onClick={() => setConfirm(true)}
          >
            {m.revocable ? t("deposits.revoke") : t("common.remove")}
          </Button>
        </div>
      </div>
      </SwipeRow>

      {/* The panel that produced it, brought back. Same address, same QR, same
          caption — something handed out twice must look identical both times, or
          the second copy invites the question of whether it is the same one.
          The state line above it is the one addition: an address the relay has
          let go still copies and still scans, and a big confident QR for a file
          that is no longer there is exactly the lie this screen exists to avoid. */}
      <Sheet
        open={share && !!handover}
        onClose={() => setShare(false)}
        placement="center"
        title={isLink ? t("deposits.shareTitle") : t("deposits.shareTicketTitle")}
        subtitle={d.name}
        footer={
          <>
            {isLink && (
              <Button
                onClick={() =>
                  openUrl(d.link).catch((e: unknown) =>
                    toast.bad(t("deposits.openFailed"), String(e))
                  )
                }
              >
                <Icon.External size={14} /> {t("deposits.openInBrowser")}
              </Button>
            )}
            <div className="spacer" />
            <Button variant="primary" onClick={() => setShare(false)}>
              {t("common.done")}
            </Button>
          </>
        }
      >
        <div className="stack">
          <div className={`t-sm tone-${m.tone}`} style={{ fontWeight: 600 }}>
            {m.text} <span className="t-sec">· {m.detail}</span>
          </div>
          <CodeHero
            value={handover}
            small
            caption={isLink ? t("send.linkDetail") : t("deposits.ticketDetail")}
          />
          {/* Only the link carries a footnote, and it is about the URL's shape —
              the key after the `#`. A sealed deposit used to get one here saying
              the recipient would probably not need this ticket at all, which is
              true of the mailbox route and useless to someone in the act of
              handing the thing over: it reads as "what you are about to give away
              does not work". The caption above already says what to do with it. */}
          {isLink && <div className="t-sm t-sec">{t("send.linkKeyNote")}</div>}
        </div>
      </Sheet>

      <Confirm
        open={confirm}
        title={
          m.revocable
            ? t("deposits.confirmRevokeTitle")
            : t("deposits.confirmRemoveTitle")
        }
        body={
          m.revocable
            ? isLink
              ? t("deposits.confirmRevokeLink")
              : t("deposits.confirmRevokeSealed")
            : t("deposits.confirmRemoveBody")
        }
        confirmLabel={m.revocable ? t("deposits.revoke") : t("common.remove")}
        danger
        busy={revoking}
        onCancel={() => setConfirm(false)}
        onConfirm={() => {
          setConfirm(false);
          // `revokeDeposit` rethrows after filing `actionError`, and `Confirm`
          // never awaits this callback — so the rejection has to die here.
          fire(revoke(d.id));
        }}
      />
    </>
  );
}

export function DepositsView() {
  const t = useT();
  const deposits = useStore((s) => s.deposits);
  const loading = useStore((s) => s.depositsLoading);
  const error = useStore((s) => s.depositsError);
  const reload = useStore((s) => s.loadDeposits);
  const openSheet = useStore((s) => s.openSheet);
  const isPhone = useIsPhone();
  const [kind, setKind] = useState<"link" | "sealed">("link");

  const links = deposits.filter((d) => d.kind === "link");
  const sealed = deposits.filter((d) => d.kind !== "link");

  return (
    <div className="stack">
      {/* A sentence and two buttons abreast is a desktop toolbar; on a phone it
          squeezed the sentence into a three-word column. Here the sentence is
          just a sentence, refreshing happens on arriving, and the one verb is a
          row of its own — at the end, where this platform puts "add". */}
      {isPhone ? (
        <div className="ios-foot">
          {t("deposits.intro")}
        </div>
      ) : (
      <div className="hstack">
        <div className="grow t-sm t-sec">{t("deposits.intro")}</div>
        <Button size="sm" onClick={() => fire(reload())} busy={loading}>
          <Icon.Refresh size={13} /> {t("common.refresh")}
        </Button>
        {/* Also here, not only in the empty state: once you have a link, the
            reason to make another one does not go away. */}
        <Button
          size="sm"
          variant="primary"
          onClick={() => openSheet([], undefined, "link")}
        >
          <Icon.Send size={13} /> {t("deposits.createLink")}
        </Button>
      </div>
      )}

      {error && (
        <div className="card card-pad card-err t-sm">
          {error}
        </div>
      )}

      {deposits.length === 0 && !loading ? (
        <div className="card">
          <Empty
            icon={<Icon.Link size={22} />}
            title={t("deposits.emptyTitle")}
            action={
              <Button variant="primary" onClick={() => openSheet([], undefined, "link")}>
                <Icon.Send size={14} /> {t("deposits.createLink")}
              </Button>
            }
          >
            {t("deposits.emptyBody")}
          </Empty>
        </div>
      ) : isPhone ? (
        /* One list at a time, chosen from a pair of pills that carry their own
           counts. The two kinds are not two halves of one thing — a public link
           is a URL anybody can fetch, a sealed deposit is a blob only one person
           can open — and on a phone two stacked sections meant scrolling past
           the kind you were not looking for to reach the one you were. */
        <>
          <Segmented
            chips
            label={t("deposits.kindLabel")}
            value={kind}
            onChange={setKind}
            options={[
              {
                value: "link",
                label: `${t("deposits.sectionLinks")} · ${links.length}`,
              },
              {
                value: "sealed",
                label: `${t("deposits.sectionSealed")} · ${sealed.length}`,
              },
            ]}
          />
          <div className="ios-group">
            {(kind === "link" ? links : sealed).map((d) => (
              <DepositRow key={d.id} d={d} />
            ))}
            {(kind === "link" ? links : sealed).length === 0 && (
              <div className="ios-row" style={{ cursor: "default" }}>
                <span className="grow t-sec">
                  {t(
                    kind === "link"
                      ? "deposits.noneLinks"
                      : "deposits.noneSealed"
                  )}
                </span>
              </div>
            )}
          </div>
        </>
      ) : (
        <>
          {links.length > 0 && (
            <div className="section">
              <div className="section-head">
                <span className="t-label">{t("deposits.sectionLinks")}</span>
                <span className="t-xs t-mut tnum">{links.length}</span>
              </div>
              <div className="card rows">
                {links.map((d) => (
                  <DepositRow key={d.id} d={d} />
                ))}
              </div>
            </div>
          )}
          {sealed.length > 0 && (
            <div className="section">
              <div className="section-head">
                <span className="t-label">{t("deposits.sectionSealed")}</span>
                <span className="t-xs t-mut tnum">{sealed.length}</span>
              </div>
              <div className="card rows">
                {sealed.map((d) => (
                  <DepositRow key={d.id} d={d} />
                ))}
              </div>
            </div>
          )}
        </>
      )}

      {/* The verb, in the band this design gives every screen. */}
      {isPhone && deposits.length > 0 && (
        <div className="phone-actions">
          <Button
            variant="in"
            size="lg"
            onClick={() => openSheet([], undefined, "link")}
          >
            <Icon.Link size={15} /> {t("deposits.createLink")}
          </Button>
        </div>
      )}
    </div>
  );
}
