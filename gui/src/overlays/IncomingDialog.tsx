// The one screen in the app where a security decision is actually made.
//
// Someone you may or may not know is offering you a file. Everything shown here
// exists to make one question answerable: *is this who I think it is?* So the
// dialog leads with identity, not with the filename — and it is careful about
// three distinctions the interface must never blur:
//
//   * A **display name** is a claim the sender types about themselves. It is not
//     authenticated by anything. It is shown in quotes, and never in the place a
//     saved contact's name would go.
//   * A **saved contact name** is your own label for a key you chose to keep.
//   * A **verified** mark means you compared the fingerprint out of band. Only
//     that one is evidence.
//
// An unknown sender therefore gets their fingerprint shown in full, not tucked
// behind a disclosure: it is the only handle on them that means anything, and
// deciding without it is deciding blind.

import { useMemo, useState } from "react";
import { api } from "../ipc";
import type { PickedItem } from "../types";
import { useStore } from "../store";
import { fmtBytes } from "../format";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import { Badge, Button, Field, TextInput, TrustBadges } from "../ui/Primitives";
import { Avatar, ClaimedName, ExtChip, Fingerprint } from "../ui/Bits";
import { Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";

export function IncomingDialog() {
  const t = useT();
  const offerId = useStore((s) => s.incomingOfferId);
  const close = useStore((s) => s.closeIncoming);
  const transfers = useStore((s) => s.transfers);
  const contactsById = useStore((s) => s.contactsById);
  const accept = useStore((s) => s.accept);
  const reject = useStore((s) => s.reject);
  const addContact = useStore((s) => s.addContact);
  const blockContact = useStore((s) => s.blockContact);
  const downloadDir = useStore((s) => s.status?.download_dir ?? "");

  const [out, setOut] = useState<PickedItem | null>(null);
  const [password, setPassword] = useState("");
  // A deposit sealed with a password looks exactly like any other offer until the
  // fetch refuses it — nothing in the offer says so. So the field appears only
  // once the daemon has actually asked for one, rather than on every arrival
  // where it would read as "this might need a password" about everything.
  const [needsPassword, setNeedsPassword] = useState(false);
  const [busy, setBusy] = useState<"accept" | "reject" | null>(null);
  const [saveAs, setSaveAs] = useState("");
  const [saving, setSaving] = useState(false);

  const tx = offerId ? transfers[`o${offerId}`] : undefined;
  const contact = useMemo(
    () => (tx?.peerId ? contactsById[tx.peerId] : undefined),
    [tx?.peerId, contactsById]
  );

  if (!tx || !offerId) return null;

  const known = !!contact;
  const claimed = tx.senderName?.trim();
  const label =
    contact?.name ?? (claimed ? `“${claimed}”` : t("incoming.unknownSender"));

  const doAccept = async () => {
    setBusy("accept");
    try {
      await accept(offerId, out?.id ?? null, password || null);
      toast.ok(t("incoming.started"), tx.name);
    } catch (e) {
      // The store already reported it. The one refusal worth reacting to here is
      // the missing password: it is recoverable, and the user has nowhere else
      // to supply one.
      if (/password/i.test(String(e))) setNeedsPassword(true);
    } finally {
      setBusy(null);
    }
  };

  const doReject = async () => {
    setBusy("reject");
    try {
      await reject(offerId);
    } catch {
      // reported by the store
    } finally {
      setBusy(null);
    }
  };

  return (
    <Sheet
      open
      onClose={close}
      placement="center"
      title={t("incoming.title")}
      subtitle={t("incoming.subtitle")}
      footer={
        <>
          <Button
            variant="danger"
            onClick={doReject}
            busy={busy === "reject"}
            disabled={busy !== null}
          >
            {t("incoming.reject")}
          </Button>
          <div className="spacer" />
          {/* Focus deliberately lands here and not on Accept: this is the one
              screen in the app where a security decision is made, and it must
              not be one keypress away from a reflex. */}
          <Button onClick={close} disabled={busy !== null} data-autofocus>
            {t("incoming.later")}
          </Button>
          <Button
            variant="in"
            onClick={doAccept}
            busy={busy === "accept"}
            disabled={busy !== null}
          >
            <Icon.Receive size={14} />
            {t("incoming.accept")}
          </Button>
        </>
      }
    >
      {/* --- who ------------------------------------------------------- */}
      <div
        className="card card-pad"
        style={{
          borderColor: known
            ? contact!.verified
              ? "var(--green)"
              : "var(--line-strong)"
            : "var(--amber)",
        }}
      >
        <div className="hstack">
          <Avatar name={label} id={tx.peerId} size={40} ring="in" />
          <div className="grow">
            <div className="hstack-sm wrap">
              <span className="t-head truncate">{label}</span>
              {known && <ClaimedName c={contact!} />}
              {known ? (
                <TrustBadges
                  verified={contact!.verified}
                  trusted={contact!.trusted}
                  blocked={contact!.blocked}
                />
              ) : (
                <Badge kind="warn">
                  <Icon.Alert size={10} /> {t("incoming.notInBook")}
                </Badge>
              )}
            </div>
            {/* The brackets next to the name carry what the ledger knows. This
                line is for the gap: a name on *this* offer that the ledger has
                not recorded yet, which is what an arrival mid-change looks
                like. When it agrees with the ledger it would just say the same
                thing twice. */}
            {known &&
              claimed &&
              claimed !== contact!.display_name.trim() &&
              claimed !== contact!.pending_name.trim() && (
                <div className="t-xs t-mut" style={{ marginTop: 2 }}>
                  {t("incoming.claimedName", claimed)}
                </div>
              )}
          </div>
        </div>

        <div className="divider" style={{ margin: "12px 0" }} />

        <div className="stack-sm">
          <div className="t-label">
            {known ? t("incoming.keyFingerprint") : t("incoming.senderId")}
          </div>
          <Fingerprint value={known ? contact!.fingerprint : (tx.peerId ?? "")} />
          <div className="hint">
            {known && contact!.verified
              ? t("incoming.hintVerified")
              : known
                ? t("incoming.hintKnown")
                : t("incoming.hintUnknown")}
          </div>
        </div>
      </div>

      {/* --- what ------------------------------------------------------ */}
      <div className="card rows">
        <div className="row" style={{ gridTemplateColumns: "38px 1fr auto" }}>
          <ExtChip name={tx.name} />
          <div className="row-main">
            <div className="row-name truncate" title={tx.name}>
              {tx.name}
            </div>
            <div className="row-meta">
              <span className="tnum">{fmtBytes(tx.size)}</span>
            </div>
          </div>
        </div>
        {tx.note && (
          <div style={{ padding: "11px 14px" }}>
            <div className="t-label" style={{ marginBottom: 4 }}>
              {t("incoming.attachedNote")}
            </div>
            <div className="t-sm selectable" style={{ whiteSpace: "pre-wrap" }}>
              {tx.note}
            </div>
          </div>
        )}
      </div>

      {needsPassword && (
        <Field
          label={t("incoming.passwordLabel")}
          hint={t("incoming.passwordHint")}
        >
          {({ id, describedBy }) => (
            <TextInput
              id={id}
              data-autofocus
              aria-describedby={describedBy}
              type="password"
              autoComplete="off"
              value={password}
              onChange={(e) => setPassword(e.currentTarget.value)}
            />
          )}
        </Field>
      )}

      {/* --- where ----------------------------------------------------- */}
      <Field
        label={t("receive.whereLabel")}
        hint={out ? undefined : t("receive.whereHint", downloadDir || "…")}
      >
        {() => (
          <div className="hstack-sm">
            <TextInput
              readOnly
              value={out?.name ?? ""}
              placeholder={downloadDir}
              aria-label={t("receive.whereAria")}
            />
            <Button
              size="sm"
              onClick={async () => {
                // Native, Rust-side: what comes back is an id + folder name,
                // never a path (see `bridge::PickedFiles`).
                const picked = await api.pickFiles(true);
                if (picked.length > 0) setOut(picked[0]);
              }}
            >
              <Icon.Folder size={13} /> {t("receive.choose")}
            </Button>
          </div>
        )}
      </Field>

      {/* --- act on the sender ----------------------------------------- */}
      {!known && (
        <div className="card card-pad stack-sm">
          <div className="t-label">{t("incoming.ifYouKnowThem")}</div>
          <div className="hstack-sm">
            <TextInput
              value={saveAs}
              onChange={(e) => setSaveAs(e.currentTarget.value)}
              placeholder={t("incoming.saveAsPlaceholder")}
              aria-label={t("incoming.saveAsLabel")}
            />
            <Button
              size="sm"
              disabled={!saveAs.trim() || saving}
              busy={saving}
              onClick={async () => {
                setSaving(true);
                try {
                  await addContact(saveAs.trim(), tx.peerId!);
                  toast.ok(
                    t("incoming.savedAs", saveAs.trim()),
                    t("incoming.savedAsDetail")
                  );
                  setSaveAs("");
                } catch {
                  // reported by the store
                } finally {
                  setSaving(false);
                }
              }}
            >
              {t("common.save")}
            </Button>
          </div>
          <div className="hint">{t("incoming.saveNote")}</div>
          <div>
            <Button
              size="sm"
              variant="danger"
              onClick={async () => {
                await blockContact(tx.peerId!);
                await reject(offerId);
                toast.ok(t("incoming.blocked"), t("incoming.blockedDetail"));
              }}
            >
              <Icon.Ban size={13} /> {t("incoming.blockAndReject")}
            </Button>
          </div>
        </div>
      )}
    </Sheet>
  );
}

