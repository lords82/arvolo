// Multi-device: several machines acting as one person.
//
// The mental model this screen has to install, because nothing else in the app
// implies it: linked devices do not *talk to each other*, they **are** each
// other. One identity, one public id, one inbox slot. Anyone who paired with you
// on one machine sees exactly one you. The address book is then kept in step
// through an encrypted cell on that shared inbox — which is why the sync state
// and the identity live on the same screen rather than in two places.
//
// There is no device *list*, and that is not an omission: the relay is
// zero-knowledge and nothing enumerates which machines hold the identity. Saying
// "3 devices linked" would be an invention. The fingerprint is what you
// compare, on each machine, to know they match.

import { fire, useStore } from "../store";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import { Badge, Button, Empty, SwitchRow } from "../ui/Primitives";
import { CopyField, Fingerprint } from "../ui/Bits";
import { toast } from "../ui/Toasts";
import { fmtAgo as ago } from "../format";

export function DevicesView() {
  const t = useT();
  const sync = useStore((s) => s.sync);
  const loading = useStore((s) => s.syncLoading);
  const error = useStore((s) => s.syncError);
  const reload = useStore((s) => s.loadSync);
  const syncNow = useStore((s) => s.syncNow);
  const startPairing = useStore((s) => s.startPairing);
  const config = useStore((s) => s.config);
  const saveConfig = useStore((s) => s.saveConfig);
  const loadConfig = useStore((s) => s.loadConfig);

  if (error && !sync) {
    return (
      <div className="card card-pad t-sm" style={{ borderColor: "var(--red)" }}>
        {error}
        <div style={{ marginTop: 10 }}>
          <Button size="sm" onClick={() => fire(reload())}>
            {t("common.retry")}
          </Button>
        </div>
      </div>
    );
  }

  if (!sync) {
    return (
      <div className="card">
        <Empty icon={<Icon.Devices size={22} />} title={t("common.loading")} />
      </div>
    );
  }

  return (
    <div className="view-narrow stack">
      {/* ---- identity ------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="hstack">
          <span className="tone-in">
            <Icon.Users size={20} />
          </span>
          <div className="grow">
            <div className="t-head">{t("devices.identityTitle")}</div>
            <div className="hint">{t("devices.identityHint")}</div>
          </div>
        </div>

        <div className="stack-sm">
          <div className="t-label">{t("devices.fingerprint")}</div>
          <div
            className="card card-pad"
            style={{ background: "var(--surface-2)" }}
          >
            <Fingerprint value={sync.fingerprint} />
          </div>
          <div className="hint">{t("devices.fingerprintHint")}</div>
        </div>

        <div className="stack-sm">
          <div className="t-label">{t("devices.publicId")}</div>
          <CopyField value={sync.public_id} wrap />
        </div>
      </div>

      {/* ---- pairing -------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">{t("devices.pairTitle")}</div>
        <div className="t-sm t-sec">{t("devices.pairBody")}</div>
        <div className="hstack wrap">
          <Button
            variant="primary"
            onClick={() => fire(startPairing("device_host"))}
          >
            <Icon.Qr size={14} /> {t("devices.showCode")}
          </Button>
          <Button onClick={() => fire(startPairing("device_join"))}>
            <Icon.Key size={14} /> {t("devices.haveCode")}
          </Button>
        </div>
        <div
          className="card card-pad t-xs"
          style={{ borderColor: "var(--amber)", background: "var(--amber-soft)" }}
        >
          <strong>{t("devices.pairWarnLead")}</strong>{" "}
          {t("devices.pairWarnRest")}
        </div>
      </div>

      {/* ---- sync ----------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="hstack">
          <div className="grow">
            <div className="t-head">{t("devices.syncTitle")}</div>
            <div className="hint">{t("devices.syncHint")}</div>
          </div>
          <Badge kind={sync.enabled ? "ok" : "neutral"}>
            {sync.enabled ? t("devices.syncOn") : t("devices.syncOff")}
          </Badge>
        </div>

        <div className="hstack wrap">
          <span className="t-sm t-sec">
            {t("devices.contactCount", sync.contacts)}
          </span>
          <span className="t-sm t-mut">·</span>
          <span className="t-sm t-sec">
            {sync.last_sync
              ? t("devices.lastSync", ago(sync.last_sync))
              : t("devices.neverSynced")}
          </span>
        </div>

        {sync.last_error && (
          <div
            className="card card-pad t-sm"
            style={{ borderColor: "var(--red)" }}
          >
            {t("devices.lastError", sync.last_error)}
          </div>
        )}

        <div className="hstack">
          <Button onClick={() => fire(syncNow())} busy={loading}>
            <Icon.Refresh size={14} /> {t("devices.syncNow")}
          </Button>
        </div>

        <div className="divider" />

        <SwitchRow
          title={t("devices.autoTitle")}
          desc={t("devices.autoDesc")}
          checked={config ? config.sync : sync.enabled}
          onChange={async (v) => {
            await saveConfig({ sync: { set: v } });
            await loadConfig();
            toast.info(
              v ? t("devices.autoOn") : t("devices.autoOff"),
              t("devices.autoDetail")
            );
          }}
        />
      </div>
    </div>
  );
}
