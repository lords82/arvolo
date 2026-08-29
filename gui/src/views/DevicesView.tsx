// Multi-device: several machines acting as one person.
//
// The mental model this screen has to install, because nothing else in the app
// implies it: linked devices do not *talk to each other*, they **are** each
// other. One identity, one public id, one inbox slot. Anyone who paired with you
// on one machine sees exactly one you. The address book is then kept in step
// through an encrypted cell on that shared inbox — which is why the sync state
// and the identity live on the same screen rather than in two places.
//
// This screen used to say, in as many words, that there was no device list and
// that this was not an omission — the relay is zero-knowledge, nothing
// enumerates which machines hold an identity, and "3 devices linked" would have
// been an invention. The list below does not repeal that: it is not an
// enumeration, it is a **self-registration**. Each machine writes its own row
// into the same encrypted snapshot that carries the address book, so the relay
// still learns nothing, and every claim in it is the machine's own. Which means
// what it shows has to be read as testimony rather than as fact — a row is there
// because something announced itself, not because anything checked.
//
// Three consequences the screen has to carry rather than hide: a machine that
// has never completed a sync round is missing; one that stopped syncing months
// ago is still listed; and "seen" is that machine's own clock, so it is shown as
// elapsed time and never as a light that says "online now". The fingerprint is
// still the only thing you *verify*, and it is still at the top for that reason.

import { useEffect, useState } from "react";
import { fire, useStore } from "../store";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import { Badge, Button, Empty, SwitchRow, TextInput } from "../ui/Primitives";
import { MenuButton } from "../ui/Menu";
import { CopyField, Fingerprint } from "../ui/Bits";
import { Confirm } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import { fmtAgo as ago } from "../format";
import type { DeviceDto } from "../types";

/** The register: what the machines have said about themselves.
 *
 *  Two affordances, and both are shaped by the same rule — a row is written by
 *  the machine it describes. So *rename* appears on this machine's row alone;
 *  offering it on the others would be a field whose value the next sync round
 *  would throw away. *Forget* is the opposite: it is for the machine you are not
 *  at, usually one you no longer have.
 *
 *  Both live in the row's ⋯ menu, as everywhere else in the app, and the one that
 *  does not apply to a row is shown disabled with the reason rather than left
 *  out: "why can I rename here and not there" is exactly the question the rule
 *  above raises, and an absent item cannot answer it. */
function DeviceList() {
  const t = useT();
  const devices = useStore((s) => s.devices);
  const rename = useStore((s) => s.renameThisDevice);
  const forget = useStore((s) => s.forgetDevice);
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [confirmForget, setConfirmForget] = useState<DeviceDto | null>(null);

  const now = Math.floor(Date.now() / 1000);

  // The edit is committed here and nowhere else, so every way of committing it —
  // Enter, the Save button — goes through the same check and the same toast.
  const commit = () => {
    const name = draft.trim();
    if (!name) return;
    setEditing(null);
    fire(
      rename(name).then(() =>
        toast.echo(t("devices.renamed"), t("devices.renamedDetail"))
      )
    );
  };

  return (
    <div className="card card-pad stack">
      <div>
        <div className="t-head">{t("devices.listTitle")}</div>
        <div className="hint">{t("devices.listHint")}</div>
      </div>

      {devices.length === 0 ? (
        <div className="t-sm t-mut">{t("devices.listEmpty")}</div>
      ) : (
        <div className="card rows">
          {devices.map((d) => (
            <div
              key={d.id}
              className="row"
              style={{ gridTemplateColumns: "28px 1fr auto" }}
            >
              <span className="tone-in hstack" style={{ justifyContent: "center" }}>
                <Icon.Devices size={18} />
              </span>
              <span className="row-main">
                {editing === d.id ? (
                  // No commit-or-cancel on blur. It used to close on blur, which
                  // meant the two gestures a person actually makes after typing a
                  // name — click Save, click anywhere — both threw the name away
                  // before anything could read it, with nothing on screen to say
                  // so. The field now stays open until one of the two buttons
                  // beside it is pressed.
                  <TextInput
                    autoFocus
                    value={draft}
                    aria-label={t("devices.renameTitle")}
                    onChange={(e) => setDraft(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") commit();
                      if (e.key === "Escape") setEditing(null);
                    }}
                  />
                ) : (
                  <span className="row-name hstack-sm">
                    <span className="truncate">{d.name}</span>
                    {d.is_this_device && (
                      <Badge kind="info">{t("devices.thisDevice")}</Badge>
                    )}
                  </span>
                )}
                <span className="row-meta">
                  {/* Elapsed, never a dot: the stamp is that machine's own clock,
                      written once per sync round, and it is clamped to now so two
                      disagreeing clocks cannot report a sighting in the future. */}
                  {d.last_seen === 0
                    ? t("devices.neverSeen")
                    : t("devices.seenAgo", ago(Math.min(d.last_seen, now)))}
                  {d.platform && ` · ${d.platform}`}
                </span>
              </span>
              <span className="hstack-sm">
                {editing === d.id ? (
                  <>
                    <Button
                      size="sm"
                      variant="primary"
                      disabled={!draft.trim()}
                      onClick={commit}
                    >
                      {t("common.save")}
                    </Button>
                    <Button size="sm" onClick={() => setEditing(null)}>
                      {t("common.cancel")}
                    </Button>
                  </>
                ) : (
                  <MenuButton
                    label={t("devices.rowActions", d.name)}
                    items={[
                      {
                        key: "rename",
                        label: t("devices.rename"),
                        icon: <Icon.Devices size={13} />,
                        disabled: !d.is_this_device,
                        desc: d.is_this_device
                          ? undefined
                          : t("devices.renameThereDesc"),
                        onSelect: () => {
                          setDraft(d.name);
                          setEditing(d.id);
                        },
                      },
                      {
                        key: "forget",
                        label: t("devices.forget"),
                        icon: <Icon.Trash size={13} />,
                        danger: !d.is_this_device,
                        separated: true,
                        // The daemon refuses it anyway — a machine that forgot
                        // itself would announce itself again five minutes later —
                        // so the menu says why instead of sending it to be told.
                        disabled: d.is_this_device,
                        desc: d.is_this_device
                          ? t("devices.forgetHereDesc")
                          : undefined,
                        onSelect: () => setConfirmForget(d),
                      },
                    ]}
                  >
                    <Icon.More size={15} />
                  </MenuButton>
                )}
              </span>
            </div>
          ))}
        </div>
      )}

      {editing !== null && <div className="hint">{t("devices.renameHint")}</div>}

      <Confirm
        open={confirmForget !== null}
        title={t("devices.confirmForgetTitle", confirmForget?.name ?? "")}
        body={t("devices.confirmForgetBody")}
        confirmLabel={t("devices.forget")}
        danger
        onCancel={() => setConfirmForget(null)}
        onConfirm={() => {
          const who = confirmForget;
          setConfirmForget(null);
          if (who)
            fire(
              forget(who.name).then(() =>
                toast.echo(t("devices.forgotten"), who.name)
              )
            );
        }}
      />
    </div>
  );
}

export function DevicesView() {
  const t = useT();
  const sync = useStore((s) => s.sync);
  // The shared identity is this daemon's identity, so the shareable spelling it
  // reports is the same one — and the field below is for handing out.
  const shareableId = useStore((s) => s.status?.shareable_id);
  const loading = useStore((s) => s.syncLoading);
  const error = useStore((s) => s.syncError);
  const reload = useStore((s) => s.loadSync);
  const syncNow = useStore((s) => s.syncNow);
  const startPairing = useStore((s) => s.startPairing);
  const openSheet = useStore((s) => s.openSheet);
  const config = useStore((s) => s.config);
  const saveConfig = useStore((s) => s.saveConfig);
  const loadConfig = useStore((s) => s.loadConfig);
  const loadDevices = useStore((s) => s.loadDevices);

  // The register is projected by the sync round, so it is stale by definition
  // between rounds; fetching on open is what makes a pairing that just happened
  // visible without waiting five minutes for the next one.
  useEffect(() => {
    void loadDevices().catch(() => {});
  }, [loadDevices]);

  if (error && !sync) {
    return (
      <div className="card card-pad card-err t-sm">
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
          <CopyField value={shareableId || sync.public_id} wrap />
        </div>

        {/* The one thing a shared identity is *for*, reachable from the screen
            that explains it. It opens the ordinary send sheet with the recipient
            already chosen — same panel, same options, nothing to learn twice. */}
        <div className="hstack">
          <Button onClick={() => openSheet([], undefined, "self")}>
            <Icon.Send size={14} /> {t("devices.sendToSelf")}
          </Button>
        </div>
      </div>

      <DeviceList />

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
          {/* The lead only. What "linked" costs — whoever types the code
              becomes you — is on the pairing sheet, where the code is actually
              on screen about to be read out. Here it sat sixteen lines under a
              paragraph that had just said the same thing. */}
          <strong>{t("devices.pairWarnLead")}</strong>
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
          <div className="card card-pad card-err t-sm">
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
              t("common.afterRestart")
            );
          }}
        />
      </div>
    </div>
  );
}
