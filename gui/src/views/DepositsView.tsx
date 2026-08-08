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
import { depositMeta, fmtBytes } from "../format";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import { Badge, Button, Empty, IconButton } from "../ui/Primitives";
import { CopyButton, ExtChip } from "../ui/Bits";
import { Confirm } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import type { DepositDto } from "../types";

function DepositRow({ d }: { d: DepositDto }) {
  const t = useT();
  const revoke = useStore((s) => s.revokeDeposit);
  const revoking = useStore((s) => s.revoking.includes(d.id));
  const peerLabel = useStore((s) => s.peerLabel);
  const [confirm, setConfirm] = useState(false);

  const m = depositMeta(d);
  const isLink = d.kind === "link";

  return (
    <>
      <div className="row" style={{ gridTemplateColumns: "38px 1fr auto" }}>
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
          {isLink && d.link && (
            <div className="copyfield" style={{ marginTop: 8 }}>
              <code className="mono">{d.link}</code>
              <IconButton
                label={t("deposits.openInBrowser")}
                onClick={() =>
                  openUrl(d.link).catch((e: unknown) =>
                    toast.bad(t("deposits.openFailed"), String(e))
                  )
                }
              >
                <Icon.External size={14} />
              </IconButton>
              <CopyButton value={d.link} />
            </div>
          )}
        </div>
        <div className="hstack-sm">
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
        onConfirm={async () => {
          setConfirm(false);
          await revoke(d.id);
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

  const links = deposits.filter((d) => d.kind === "link");
  const sealed = deposits.filter((d) => d.kind !== "link");

  return (
    <div className="stack">
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

      {error && (
        <div className="card card-pad t-sm" style={{ borderColor: "var(--red)" }}>
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
    </div>
  );
}
