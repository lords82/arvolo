// What a share has actually done.
//
// A share is not a transfer and the difference is the whole reason this panel
// exists. A transfer has a destination, a percentage and an end; a share has none
// of the three — it sits there being available, and the only questions worth
// asking about it are *has anyone taken it*, *is anyone taking it now*, and *what
// is it costing me*. Drawn as a transfer it answered none of them: a served ticket
// showed 100% for ever and read as stuck, and the seeding a finished download
// turns into showed 0% and read as a send of a file the user never sent.
//
// Two things this panel is careful about:
//
//   * **Copies, not people.** An anonymous ticket carries no identity, by design.
//     One person fetching twice is two copies here and there is no way to know
//     otherwise, so the wording never says "people".
//   * **A row nobody created explains itself.** Seeding starts on its own when a
//     download finishes. Told plainly — you downloaded this, and are now making it
//     available — the row stops looking like a bug, and the way to turn the whole
//     behaviour off is right there rather than buried.

import { useStore } from "../store";
import { fmtAgo, fmtBytes } from "../format";
import { useT } from "../i18n";
import { Button } from "../ui/Primitives";
import { ExtChip } from "../ui/Bits";
import { Sheet } from "../ui/Sheet";

/** One number and what it means, stacked. */
function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="stat">
      <div className="stat-value tnum">{value}</div>
      <div className="stat-label t-xs t-mut">{label}</div>
    </div>
  );
}

export function ShareSheet() {
  const t = useT();
  const id = useStore((s) => s.shareOpen);
  const close = useStore((s) => s.closeShare);
  const transfers = useStore((s) => s.transfers);
  const cancel = useStore((s) => s.cancel);
  const go = useStore((s) => s.go);

  const tx = id === null ? undefined : transfers[`t${id}`];
  if (id === null || !tx) return null;

  const stop = () => {
    void cancel(tx.id).catch(() => {});
    close();
  };

  return (
    <Sheet
      open
      onClose={close}
      title={t("share.title")}
      subtitle={tx.name}
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            {t("common.close")}
          </Button>
          <Button variant="danger" onClick={stop}>
            {t("share.stop")}
          </Button>
        </>
      }
    >
      <div className="hstack-sm" style={{ marginBottom: 14 }}>
        <ExtChip name={tx.name} />
        <div className="truncate">
          <div className="row-name truncate" title={tx.name}>
            {tx.name}
          </div>
          <div className="t-xs t-mut tnum">{fmtBytes(tx.size)}</div>
        </div>
      </div>

      {/* Why this row exists. Only for a share that started itself — for a ticket
          the user asked for, saying "you created this" would be noise. */}
      {tx.fromDownload > 0 && (
        <p className="t-sm" style={{ marginBottom: 14 }}>
          {t("share.fromDownload", fmtAgo(tx.fromDownload))}{" "}
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              close();
              go("settings");
            }}
          >
            {t("share.seedingSetting")}
          </Button>
        </p>
      )}

      <div className="stats">
        <Stat label={t("share.copies")} value={String(tx.copiesServed)} />
        <Stat label={t("share.now")} value={String(tx.downloadPeers)} />
        <Stat
          label={t("share.lastPickup")}
          // Never taken is not "0 ago" and not blank: it is the answer to the
          // question, and the most likely one for a share nobody has used.
          value={tx.lastPickup > 0 ? fmtAgo(tx.lastPickup) : t("share.never")}
        />
        <Stat label={t("share.uploaded")} value={fmtBytes(tx.bytesServed)} />
      </div>

      <p className="t-xs t-mut" style={{ marginTop: 14 }}>
        {t("share.countsNote")}
      </p>
    </Sheet>
  );
}
