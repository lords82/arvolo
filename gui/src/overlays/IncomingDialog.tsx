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
// deciding without it is deciding blind. It arrives stamped on the offer, so
// seeing it costs nothing — the dialog used to show a stranger the raw base32 id
// and invite them to save the person in order to be given words to compare,
// which is a decision surface asking you to commit before it will inform you.
//
// The name field is a *field*, not a form. Nothing is written to the address book
// until one of the footer buttons is pressed. That is not a nicety: the previous
// shape had its own Save button, so the address book moved mid-decision, this
// component re-rendered the sender as known, and the whole panel — the place you
// had just been reading — vanished from under the pointer.

import { useMemo, useRef, useState } from "react";
import { api } from "../ipc";
import type { PickedItem } from "../types";
import { useStore } from "../store";
import { fmtBytes, slugName } from "../format";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import { Badge, Button, Field, TextInput, TrustBadges } from "../ui/Primitives";
import { Avatar, ClaimedName, ExtChip, Fingerprint } from "../ui/Bits";
import { nameCollision, SaveSenderFields } from "../ui/SaveSenderFields";
import { Sheet } from "../ui/Sheet";
import { useIsPhone } from "../phone";
import { toast } from "../ui/Toasts";

/** Reads which offer is on screen, and gives the body a fresh mount per offer.
 *
 *  This component is mounted for the life of the window and returns `null` when
 *  nothing is waiting, so every `useState` below it would otherwise outlive the
 *  offer that filled it — the second arrival would inherit the first one's
 *  proposed name, chosen folder and typed password. */
export function IncomingDialog() {
  const offerId = useStore((s) => s.incomingOfferId);
  if (!offerId) return null;
  return <IncomingBody key={offerId} offerId={offerId} />;
}

function IncomingBody({ offerId }: { offerId: string }) {
  const t = useT();
  const isPhone = useIsPhone();
  const close = useStore((s) => s.closeIncoming);
  const transfers = useStore((s) => s.transfers);
  const contacts = useStore((s) => s.contacts);
  const contactsById = useStore((s) => s.contactsById);
  const accept = useStore((s) => s.accept);
  const reject = useStore((s) => s.reject);
  const addContact = useStore((s) => s.addContact);
  const blockContact = useStore((s) => s.blockContact);
  const downloadDir = useStore((s) => s.status?.download_dir ?? "");

  const tx = transfers[`o${offerId}`];

  const [out, setOut] = useState<PickedItem | null>(null);
  const [password, setPassword] = useState("");
  // A deposit sealed with a password looks exactly like any other offer until the
  // fetch refuses it — nothing in the offer says so. So the field appears only
  // once the daemon has actually asked for one, rather than on every arrival
  // where it would read as "this might need a password" about everything.
  const [needsPassword, setNeedsPassword] = useState(false);
  const [busy, setBusy] = useState<"accept" | "reject" | null>(null);
  const [saveAs, setSaveAs] = useState(() => slugName(tx?.senderName));
  const [edited, setEdited] = useState(false);

  const contact = useMemo(
    () => (tx?.peerId ? contactsById[tx.peerId] : undefined),
    [tx?.peerId, contactsById]
  );
  // Whether the sender was a stranger when this dialog opened. Read once: the
  // question "do you want to keep them?" belongs to this decision, and must not
  // be withdrawn mid-sentence because the book moved.
  const wasUnknown = useRef(!contact).current;

  if (!tx) return null;

  const known = !!contact;
  const claimed = tx.senderName?.trim();
  const label =
    contact?.name ?? (claimed ? `“${claimed}”` : t("incoming.unknownSender"));
  // A stranger has no `ContactDto` to carry words, so the offer brings its own.
  const fingerprint = contact?.fingerprint ?? tx.fingerprint ?? "";
  const chosenName = saveAs.trim();
  const clash = nameCollision(contacts, saveAs, tx.peerId);
  const willSave = wasUnknown && !known && !!chosenName && !clash;

  const doAccept = async () => {
    setBusy("accept");
    try {
      // Saving first, and only accepting if it worked: a refusal here is the
      // collision guard, and taking the file anyway would leave the user with
      // the download they asked for and none of the naming they also asked for,
      // with nothing on screen still able to explain the difference.
      if (willSave) await addContact(chosenName, tx.peerId!);
      await accept(offerId, out?.id ?? null, password || null);
      toast.echo(t("incoming.started"), tx.name);
    } catch (e) {
      // The store already reported it. The one refusal worth reacting to here is
      // the missing password: it is recoverable, and the user has nowhere else
      // to supply one.
      //
      // The daemon names it — `needsPassword` — so this no longer depends on the
      // engine's English keeping the word "password" in it. The old test stays as
      // the fallback for a daemon too old to send a code: without it, upgrading
      // the app before the daemon would take the field away.
      const code = (e as { code?: unknown } | null)?.code;
      const named = typeof code === "string" && code !== "";
      if (named ? code === "needsPassword" : /password/i.test(String(e))) {
        setNeedsPassword(true);
      }
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
      // The scrim does not dismiss: this panel changes shape as it is used, and a
      // decision must not be closable by a click that landed where a control was
      // a moment ago. Escape, "Later" and the × all still close it.
      dismissOnScrim={false}
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
            disabled={busy !== null || !!clash}
          >
            <Icon.Receive size={14} />
            {willSave ? t("incoming.acceptAndSave") : t("incoming.accept")}
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
            {/* And the same gap for somebody you have never saved. Without it a
                key that introduced itself as one person and came back as another
                is indistinguishable from a key that has only ever used the second
                name — which is the whole of what a display name can tell you. */}
            {!known && claimed && tx.knownName && tx.knownName !== claimed && (
              <div className="t-xs tone-warn" style={{ marginTop: 2 }}>
                {t("incoming.wasCalled", tx.knownName)}
              </div>
            )}
          </div>
        </div>

        <div className="divider" style={{ margin: "12px 0" }} />

        <div className="stack-sm">
          <div className="t-label">
            {known ? t("incoming.keyFingerprint") : t("incoming.senderId")}
          </div>
          {/* The offer carries no fingerprint against a daemon too old to stamp
              one. Falling back to the raw id keeps the identity on screen rather
              than showing an empty space where the evidence should be. */}
          <Fingerprint value={fingerprint || (tx.peerId ?? "")} />
          <div className="hint">
            {known && contact!.verified
              ? t("incoming.hintVerified")
              : known
                ? t("incoming.hintKnown")
                : fingerprint
                  ? t("incoming.hintStranger")
                  : t("incoming.hintRawId")}
          </div>
          {!known && tx.seenCount !== undefined && (
            <div className={tx.seenCount === 0 ? "hint tone-warn" : "hint"}>
              {tx.seenCount === 0
                ? t("incoming.firstTime")
                : t("incoming.seenBefore", String(tx.seenCount))}
            </div>
          )}
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
      {/* Not on a phone. There the destination is the app's own storage and
          cannot be chosen — `pick_folders` does not exist on mobile, so the
          button opened nothing — and what the field showed was
          `/data/user/0/it.termox.arvolo/incoming`, a path that means something
          to a filesystem and nothing to a person. Saying where files go is a
          job for the screen that will let you open them; a dead control on the
          one screen where a security decision is made is worse than silence. */}
      {!isPhone && (
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
      )}

      {/* --- act on the sender ----------------------------------------- */}
      {wasUnknown && (
        <div className="card card-pad stack-sm">
          <div className="t-label">{t("incoming.ifYouKnowThem")}</div>
          {known ? (
            // Saved by this dialog's own Accept, or from another window while it
            // sat open. Kept in place rather than removed: the panel is where the
            // user was reading, and a card that disappears as it is used reads as
            // the whole dialog going away.
            <div className="hstack-sm">
              <Icon.Check size={13} className="tone-ok" />
              <span className="t-sm">
                {t("incoming.savedInline", contact!.name)}
              </span>
            </div>
          ) : (
            <SaveSenderFields
              value={saveAs}
              onChange={(v) => {
                setSaveAs(v);
                setEdited(true);
              }}
              contacts={contacts}
              peerId={tx.peerId}
              suggested={!edited}
            />
          )}
          <div>
            <Button
              size="sm"
              variant="danger"
              disabled={busy !== null}
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
