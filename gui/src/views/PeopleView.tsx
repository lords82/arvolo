// The address book — and, because this app's promise rests on it, the place
// where trust is granted and taken back.
//
// Three marks, three different things, deliberately never collapsed into one
// "trusted" toggle:
//
//   verified — you compared the fingerprint out of band. Evidence.
//   trusted  — their files download without asking you. A convenience that is
//              only safe *because* of the first, which is why the daemon refuses
//              to grant it to an unverified key unless it is forced, and why the
//              forcing dialog spells out what is being waived.
//   blocked  — their offers are dropped on arrival, silently.
//
// Import/export live here rather than behind a menu because moving an address
// book between machines is a real thing people do, and because the import path
// has one rule worth surfacing: it never rebinds a name you already use, since
// that would be a key change nobody asked about.

import { useEffect, useMemo, useRef, useState } from "react";
import { fire, useStore } from "../store";
import { useIsPhone } from "../phone";
import { api } from "../ipc";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import {
  Badge,
  Button,
  Empty,
  Field,
  Segmented,
  TextInput,
  TrustBadges,
} from "../ui/Primitives";
import {
  Avatar,
  ClaimedName,
  claimText,
  CopyField,
  Fingerprint,
} from "../ui/Bits";
import { fmtAgo, relayHost } from "../format";
import { MenuButton, type MenuItem } from "../ui/Menu";
import { nameCollision } from "../ui/SaveSenderFields";
import { Confirm, Sheet } from "../ui/Sheet";
import { SwipeRow } from "../ui/SwipeRow";
import { toast } from "../ui/Toasts";
import type { ContactDto } from "../types";

type Filter = "all" | "verified" | "trusted" | "blocked";

/** How often the dots are re-asked while the address book is on screen.
 *  Presence is the one thing on this board the engine never pushes, so it is
 *  also the one thing that has to be polled. A minute is short enough that a
 *  dot is not lying for long, and long enough that leaving the window open all
 *  afternoon is not a standing load on the relay. */
const PRESENCE_REFRESH_MS = 60_000;

// ---------------------------------------------------------------------------

/** Reachability, said honestly. Three states, because there are three: here,
 *  away, and *not asked* — the relay may simply not have answered. */
function PresenceDot({ id }: { id: string }) {
  const t = useT();
  const online = useStore((s) => s.presence[id]);
  if (online === undefined || online === null) {
    return (
      <span
        className="dot"
        title={t("people.presenceUnknownTitle")}
        aria-label={t("people.presenceUnknownLabel")}
      />
    );
  }
  return (
    <span
      className={`dot ${online ? "on" : "off"}`}
      title={
        online ? t("people.presenceOnTitle") : t("people.presenceOffTitle")
      }
      aria-label={online ? t("people.presenceOn") : t("people.presenceOff")}
    />
  );
}

/** The line under a name in the list.
 *
 *  Two facts, and only when they say something. *When you last dealt with them*
 *  is what an address book is for — it is how you tell the four people you work
 *  with from the thirty-six you met once — and it is synced, so it reads the same
 *  on every one of your machines. *Where they listen* appears only when it is not
 *  where you do: printing "same relay as you" on every row is a column that says
 *  nothing, which is what the CLI decided for its own list.
 *
 *  A contact with neither gets an empty line rather than filler. Somebody saved a
 *  minute ago and never written to has nothing to report yet, and saying so takes
 *  up a row's worth of attention for no answer. */
function ContactMeta({ c, prefix = "" }: { c: ContactDto; prefix?: string }) {
  const t = useT();
  const mine = useStore((s) => s.status?.relay ?? "");
  const parts: string[] = [];
  if (c.last_exchange) parts.push(t("people.lastExchange", fmtAgo(c.last_exchange)));
  const elsewhere = c.relay && relayHost(c.relay) !== relayHost(mine);
  if (elsewhere) parts.push(t("people.listensOn", relayHost(c.relay!)));
  if (!parts.length) return null;
  // `prefix` is for the phone's row, where this follows the trust word on the
  // same line: the separator has to be part of the text, not a gap that
  // disappears the moment the line wraps.
  return <span className="t-xs t-mut">{prefix}{parts.join(" · ")}</span>;
}

/** The one word this screen uses for where a person stands. The three marks are
 *  deliberately not collapsed anywhere else in the app; here they are read at a
 *  glance down a column, and a column of two-chip combinations is read one line
 *  at a time. The order is the order of consequence, not the order of the
 *  fields. */
function trustWord(c: ContactDto, t: ReturnType<typeof useT>): string {
  if (c.blocked) return t("people.wordBlocked");
  if (c.trusted) return t("people.wordTrusted");
  if (c.verified) return t("people.wordAsks");
  return t("people.notVerified");
}

function trustTone(c: ContactDto): string {
  if (c.blocked) return "bad";
  if (c.trusted) return "ok";
  return "mut";
}

/** The row this platform draws for a person: a face, a name, one line about
 *  them, what is worth reading at a glance, and the way in. The verbs are not
 *  here — they are one screen further, where each has room for its own word. */
function PhoneRow({ c }: { c: ContactDto }) {
  const t = useT();
  const openPerson = useStore((s) => s.openPerson);
  const startCompose = useStore((s) => s.startCompose);
  const blockContact = useStore((s) => s.blockContact);
  const unblockContact = useStore((s) => s.unblockContact);
  return (
    // The one thing you do *to* a person rather than with them. Blocking used
    // to be four taps in — row, menu, item, confirm — for something whose whole
    // point is that you want it done now.
    <SwipeRow
      actions={[
        c.blocked
          ? {
              label: t("people.menuUnblock"),
              run: () => fire(unblockContact(c.name)),
            }
          : {
              label: t("people.menuBlock"),
              danger: true,
              // Blocking takes the person off every list but *Blocked*, so from
              // where the finger is it looks exactly like a delete: the row is
              // there, then it is not, and nothing says why or where it went.
              // A long pull commits without a button being pressed, which makes
              // the two together a destruction nobody confirmed.
              //
              // So the act says its own name and carries its own way back. Not
              // a confirmation dialog in front of it: this platform answers a
              // reversible act with an undo *after* it, and a question in front
              // of every block would make the fast path — which is the whole
              // reason the swipe exists — slower than the menu it replaced.
              run: () =>
                fire(
                  blockContact(c.name).then(() =>
                    toast.info(t("people.blocked", c.name), t("people.blockedWhere"), {
                      label: t("common.undo"),
                      run: () => fire(unblockContact(c.name)),
                    })
                  )
                ),
            },
      ]}
    >
    <div className="people-row">
      <button className="ios-row grow" onClick={() => openPerson(c.name)}>
        <Avatar name={c.display_name || c.name} id={c.id} size={42} />
        <span className="grow" style={{ minWidth: 0 }}>
          <span className="row-name hstack-sm" title={c.name}>
            <PresenceDot id={c.id} />
            <span className="truncate">{c.name}</span>
          </span>
          {/* Trust as a *word*, on the line under the name, rather than as two
              chips beside it. Two marks in chips took the width the send button
              needs and pushed it off a 402px screen at the second badge; and
              "verified but not trusted" says more as the sentence it actually
              means — you will be asked before anything of theirs lands — than as
              the absence of a second chip. */}
          {/* One line, and it *truncates* rather than wrapping. Wrapped, it made
              every row a different height and pushed the send button off the
              card — and the two halves ran together into "Chiede confermaultimo
              scambio 16 giorni fa", because a flex gap is not a separator when
              the line breaks. The dot is written, so it is there in the reading
              as well as in the spacing. */}
          <span className="row-meta truncate">
            <span className={`trust-word ${trustTone(c)}`}>{trustWord(c, t)}</span>
            <ContactMeta c={c} prefix=" · " />
          </span>
        </span>
        {/* No chevron beside a send button: two glyphs at the end of one row,
            one of which is not a target, is the row asking to be read twice. The
            chevron stays where it is the only way in. */}
        {c.blocked ? <Icon.ChevronRight size={15} className="ios-chev" /> : null}
      </button>
      {/* Sending is one tap from the row, not two. The address book is opened
          *in order to send* far more often than it is opened to read about
          somebody, and the design of record puts the verb on the line for
          exactly that reason. Blocked people do not get one: the daemon would
          refuse, and a button that cannot work is worse than no button. */}
      {!c.blocked && (
        <button
          className="people-send"
          aria-label={t("people.sendTo", c.name)}
          onClick={() => startCompose({ kind: "contact", name: c.name })}
        >
          <Icon.ArrowUpRight size={18} />
        </button>
      )}
    </div>
    </SwipeRow>
  );
}

function PersonRow({ c }: { c: ContactDto }) {
  const t = useT();
  const isPhone = useIsPhone();
  if (isPhone) return <PhoneRow c={c} />;
  const openSheet = useStore((s) => s.openSheet);
  const openPerson = useStore((s) => s.openPerson);
  const markUnverified = useStore((s) => s.markUnverified);
  const markTrusted = useStore((s) => s.markTrusted);
  const markUntrusted = useStore((s) => s.markUntrusted);
  const blockContact = useStore((s) => s.blockContact);
  const unblockContact = useStore((s) => s.unblockContact);
  const acceptName = useStore((s) => s.acceptName);
  const removeContact = useStore((s) => s.removeContact);

  const [confirmRemove, setConfirmRemove] = useState(false);
  const [confirmForce, setConfirmForce] = useState(false);

  const items: MenuItem[] = [
    {
      key: "detail",
      label: t("people.menuDetails"),
      icon: <Icon.Info size={13} />,
      onSelect: () => openPerson(c.name),
    },
    c.verified
      ? {
          key: "unverify",
          label: t("people.menuUnverify"),
          icon: <Icon.Shield size={13} />,
          onSelect: () => fire(markUnverified(c.name)),
        }
      : {
          key: "verify",
          label: t("people.menuVerify"),
          icon: <Icon.Shield size={13} />,
          onSelect: () => openPerson(c.name),
        },
    c.trusted
      ? {
          key: "untrust",
          label: t("people.menuUntrust"),
          desc: t("people.menuUntrustDesc"),
          icon: <Icon.Star size={13} />,
          onSelect: () => fire(markUntrusted(c.name)),
        }
      : {
          key: "trust",
          label: t("people.menuTrust"),
          desc: t("people.menuTrustDesc"),
          icon: <Icon.Star size={13} />,
          onSelect: () => {
            if (c.verified) fire(markTrusted(c.name, false));
            else setConfirmForce(true);
          },
        },
    c.blocked
      ? {
          key: "unblock",
          label: t("people.menuUnblock"),
          icon: <Icon.Ban size={13} />,
          separated: true,
          onSelect: () => fire(unblockContact(c.name)),
        }
      : {
          key: "block",
          label: t("people.menuBlock"),
          icon: <Icon.Ban size={13} />,
          danger: true,
          separated: true,
          onSelect: () => fire(blockContact(c.name)),
        },
    {
      key: "remove",
      label: t("people.menuRemove"),
      icon: <Icon.Trash size={13} />,
      danger: true,
      onSelect: () => setConfirmRemove(true),
    },
  ];

  return (
    <>
      {/* A row, like every other list in the app. It was a card, on the theory
          that a person is an entity rather than an event and so deserves a face
          of their own. What the grid actually bought was one tile adrift in a
          wide window at small address books, and at large ones it threw away the
          thing a list does best: the same attribute in the same place on every
          line, so "who is online" and "who is unverified" are read down a column
          instead of hunted inside forty tiles.

          The fingerprint did not come across, and that is deliberate. It is the
          one string in this app that must never be shown truncated — it exists
          to be read out loud and compared, and an ellipsis through the middle of
          it makes that impossible. A row cannot promise it the width, so it
          stays whole in the details panel, which is where comparing happens
          anyway. What the row carries under the name is not identity at all —
          see `ContactMeta`. */}
      <div className={`row ${c.blocked ? "is-done" : ""}`}>
        <Avatar name={c.display_name || c.name} id={c.id} size={34} />

        <div className="row-main">
          {/* `truncate` goes on the name, never on the line that holds the dot.
              It carries `overflow: hidden`, and the online dot's pulse is drawn
              as a 6px box-shadow ring *outside* its 7px box — put the two on the
              same element and the ring is sliced off flat against the left edge.
              The name still ellipsises; `.row-main` above is `min-width: 0`, so
              the flex line can shrink to let it. */}
          <div
            className={`row-name hstack-sm ${isPhone ? "tap" : ""}`}
            title={c.name}
            onClick={isPhone ? () => openPerson(c.name) : undefined}
          >
            <PresenceDot id={c.id} />
            <span className="truncate">{c.name}</span>
            <ClaimedName c={c} />
          </div>
          {/* What the row is asked, once there are forty of them: when did I last
              deal with this person, and — when it is not the ordinary answer —
              where they listen. The truncated public id that used to live here
              answered neither: the name above already recognises them, and
              verifying is done against the whole fingerprint in the panel. */}
          <div className="row-meta">
            <ContactMeta c={c} />
          </div>

          {/* The name itself is already up there in brackets, readable without
              deciding anything. What is left for this box is the one thing the
              brackets cannot say: that acknowledging changes nothing about how
              the contact is named here. */}
          {c.pending_name && (
            <div
              className="card card-pad t-xs hstack-sm wrap"
              style={{ borderColor: "var(--amber)", padding: 10, marginTop: 8 }}
            >
              <span className="grow">{t("people.claimedNote", c.name)}</span>
              <Button size="sm" onClick={() => fire(acceptName(c.name))}>
                {t("people.acknowledge")}
              </Button>
            </div>
          )}
        </div>

        {/* Four controls per row is a desktop's answer, and it is a good one
            there: a pointer is precise and the row is wide. A phone has neither,
            so the row itself is the way in — tapping it opens the person, where
            every verb already lives — and what stays beside the name is the one
            thing that is read rather than pressed. */}
        <div className="row-actions">
          <TrustBadges
            verified={c.verified}
            trusted={c.trusted}
            blocked={c.blocked}
          />
          {!c.verified && !c.blocked && (
            <Badge kind="warn" title={t("people.notVerifiedTitle")}>
              {t("people.notVerified")}
            </Badge>
          )}
          {!isPhone && (
            <>
              <Button
                size="sm"
                variant="primary"
                disabled={c.blocked}
                onClick={() => openSheet([], c.name, "contact")}
              >
                <Icon.Send size={13} /> {t("people.send")}
              </Button>
              <Button size="sm" onClick={() => openPerson(c.name)}>
                {t("people.details")}
              </Button>
            </>
          )}
          <MenuButton items={items} label={t("people.rowActions", c.name)}>
            <Icon.More size={15} />
          </MenuButton>
        </div>
      </div>

      <Confirm
        open={confirmRemove}
        title={t("people.confirmRemoveTitle", c.name)}
        body={t("people.confirmRemoveBody")}
        confirmLabel={t("common.remove")}
        danger
        onCancel={() => setConfirmRemove(false)}
        onConfirm={() => {
          setConfirmRemove(false);
          fire(removeContact(c.name));
        }}
      />

      <Confirm
        open={confirmForce}
        title={t("people.confirmForceTitle")}
        body={
          <>
            {t("people.confirmForceBody", c.name)}
            <div style={{ marginTop: 10 }}>
              <Fingerprint value={c.fingerprint} />
            </div>
          </>
        }
        confirmLabel={t("people.confirmForceLabel")}
        cancelLabel={t("people.confirmForceCancel")}
        danger
        onCancel={() => setConfirmForce(false)}
        onConfirm={() => {
          setConfirmForce(false);
          fire(markTrusted(c.name, true));
        }}
      />
    </>
  );
}

// ---------------------------------------------------------------------------

function AddPersonSheet({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const t = useT();
  const contacts = useStore((s) => s.contacts);
  const addContact = useStore((s) => s.addContact);
  const rebindContact = useStore((s) => s.rebindContact);
  const [name, setName] = useState("");
  const [id, setId] = useState("");
  const [busy, setBusy] = useState(false);
  // The name is already somebody else's. Re-pointing it is a real thing to want
  // — a contact reinstalled and has a new key — but it drops their verified and
  // trusted marks, so it is a question, not a side effect. The daemon refuses the
  // plain save for exactly this case; this is the only way past it.
  const [confirmRebind, setConfirmRebind] = useState(false);
  const clash = nameCollision(contacts, name, undefined);

  const done = (label: string, detail: string) => {
    toast.echo(label, detail);
    setName("");
    setId("");
    setConfirmRebind(false);
    onClose();
  };

  const submit = async () => {
    if (clash) {
      setConfirmRebind(true);
      return;
    }
    setBusy(true);
    try {
      await addContact(name.trim(), id.trim());
      done(t("people.addSaved", name.trim()), t("people.addSavedDetail"));
    } catch {
      // reported by the store
    } finally {
      setBusy(false);
    }
  };

  const doRebind = async () => {
    setBusy(true);
    try {
      await rebindContact(name.trim(), id.trim());
      done(t("people.rekeyed", name.trim()), t("people.rekeyedDetail"));
    } catch {
      // reported by the store
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      open={open}
      onClose={onClose}
      placement="center"
      title={t("people.addTitle")}
      subtitle={t("people.addSubtitle")}
      // A task, so on a phone the decision moves into the sheet's top bar —
      // cancel on the left, the saving verb on the right — like every form
      // this platform presents.
      bar={{
        cancel: (
          <button className="bar-btn" onClick={onClose} disabled={busy}>
            {t("common.cancel")}
          </button>
        ),
        confirm: (
          <button
            className="bar-btn strong"
            onClick={submit}
            disabled={!name.trim() || !id.trim() || busy}
          >
            {t("common.save")}
          </button>
        ),
      }}
      footer={
        <>
          <div className="spacer" />
          <Button onClick={onClose} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="primary"
            busy={busy}
            disabled={!name.trim() || !id.trim() || busy}
            onClick={submit}
          >
            {t("common.save")}
          </Button>
        </>
      }
    >
      <Field
        label={t("people.addNameLabel")}
        hint={clash ? t("people.addNameTaken", clash.name) : undefined}
      >
        {({ id: fid, describedBy }) => (
          <TextInput
            id={fid}
            data-autofocus
            aria-describedby={describedBy}
            value={name}
            onChange={(e) => setName(e.currentTarget.value)}
            placeholder={t("people.addNamePlaceholder")}
          />
        )}
      </Field>
      <Field
        label={t("people.addIdLabel")}
        hint={t("people.addIdHint")}
      >
        {({ id: fid, describedBy }) => (
          <TextInput
            id={fid}
            aria-describedby={describedBy}
            className="mono"
            spellCheck={false}
            autoCapitalize="off"
            value={id}
            onChange={(e) => setId(e.currentTarget.value)}
            placeholder="if2xmne…"
          />
        )}
      </Field>
      <div className="card card-pad t-sm t-sec">{t("people.addTip")}</div>
      <Confirm
        open={confirmRebind}
        danger
        busy={busy}
        title={t("people.confirmRekeyTitle", name.trim())}
        body={
          <>
            <div>{t("people.confirmRekeyBody", name.trim())}</div>
            <div style={{ marginTop: 8 }}>
              <div className="t-label">{t("people.confirmRekeyCurrent")}</div>
              <Fingerprint value={clash?.fingerprint ?? ""} />
            </div>
          </>
        }
        confirmLabel={t("people.confirmRekeyDo")}
        onConfirm={doRebind}
        onCancel={() => setConfirmRebind(false)}
      />
    </Sheet>
  );
}

// ---------------------------------------------------------------------------

/** The detail sheet: everything about one person, and the only place the
 *  verification ritual can be completed. It is a ritual on purpose — reading the
 *  fingerprint and confirming are two separate acts, so marking someone verified
 *  can never be a side effect of glancing at their card. */
function PersonSheet() {
  const t = useT();
  const name = useStore((s) => s.personOpen);
  const close = useStore((s) => s.openPerson);
  const contacts = useStore((s) => s.contacts);
  const markVerified = useStore((s) => s.markVerified);
  const markUnverified = useStore((s) => s.markUnverified);
  const renameContact = useStore((s) => s.renameContact);
  const acceptName = useStore((s) => s.acceptName);

  const c = contacts.find((x) => x.name === name);
  const openThread = useStore((s) => s.openThread);
  const transfers = useStore((s) => s.transfers);
  const [checked, setChecked] = useState(false);
  const [newName, setNewName] = useState("");

  // This component stays mounted between people, and on a phone the way out is
  // the history rather than `onClose` — so the reset has to follow the person,
  // not the closing. Without it, the verify checkbox ticked for one contact
  // greets the next one already ticked.
  useEffect(() => {
    setChecked(false);
    setNewName("");
  }, [name]);

  // The grouping keys a thread by the peer's id when it has one, and by the
  // label otherwise — so the row has to ask the same question `threadsOf` does,
  // and only offers itself when there is in fact something to open.
  const threadKey = c?.id || c?.name || "";
  const hasThread = Object.values(transfers).some(
    (x) => (x.peerId || x.peer) === threadKey && x.status !== "sharing"
  );

  if (!c) return null;

  return (
    <Sheet
      open
      onClose={() => {
        setChecked(false);
        setNewName("");
        close(null);
      }}
      title={c.name}
      // A contact is content you drilled into, not a task: on a phone this
      // arrives as a pushed screen, from the list whose name the back button
      // carries.
      pushFrom={t("title.people")}
      // `<ClaimedName>` is always a truthy element even when it renders
      // nothing, and `Sheet` would keep an empty subtitle line for it.
      subtitle={claimText(c, t) ? <ClaimedName c={c} /> : undefined}
    >
      <div className="hstack">
        <Avatar name={c.display_name || c.name} id={c.id} size={48} />
        <div className="hstack-sm wrap grow">
          <TrustBadges
            verified={c.verified}
            trusted={c.trusted}
            blocked={c.blocked}
          />
        </div>
      </div>

      {/* Everything that has passed between the two of you. This used to be the
          phone's home screen — one row per correspondent, opening the thread —
          and when the home went back to being the board the thread came here,
          which is where somebody looking for "what did I send them" would go
          anyway: a person's own screen. */}
      {hasThread && (
        <button className="ios-row card" onClick={() => openThread(threadKey)}>
          <Icon.History size={16} className="t-sec" />
          <span className="grow">{t("person.thread")}</span>
          <Icon.ChevronRight size={15} className="ios-chev" />
        </button>
      )}

      {/* Same offer as the row, because this panel is where someone lands when
          they open a contact to look into exactly this. */}
      {c.pending_name && (
        <div
          className="card card-pad t-sm hstack-sm wrap"
          style={{ borderColor: "var(--amber)" }}
        >
          <span className="grow">{t("people.claimedNote", c.name)}</span>
          <Button size="sm" onClick={() => fire(acceptName(c.name))}>
            {t("people.acknowledge")}
          </Button>
        </div>
      )}

      <Field
        label={t("person.fingerprint")}
        hint={t("person.fingerprintHint")}
      >
        {() => (
          <div
            className="card card-pad"
            style={{ background: "var(--surface-2)" }}
          >
            <Fingerprint value={c.fingerprint} />
          </div>
        )}
      </Field>

      <Field label={t("person.publicId")}>
        {() => <CopyField value={c.id} wrap />}
      </Field>

      {c.verified ? (
        <div className="card card-pad stack-sm">
          <div className="hstack-sm">
            <Icon.Shield className="tone-ok" />
            <strong>{t("person.verified")}</strong>
          </div>
          <div className="t-sm t-sec">{t("person.verifiedBody")}</div>
          <div>
            <Button size="sm" onClick={() => fire(markUnverified(c.name))}>
              {t("person.unverify")}
            </Button>
          </div>
        </div>
      ) : (
        <div
          className="card card-pad stack-sm"
          style={{ borderColor: "var(--amber)" }}
        >
          <strong>{t("person.notVerifiedYet")}</strong>
          <div className="t-sm t-sec">{t("person.notVerifiedBody")}</div>
          <label className="hstack" style={{ cursor: "pointer" }}>
            <input
              type="checkbox"
              checked={checked}
              onChange={(e) => setChecked(e.currentTarget.checked)}
            />
            <span className="t-sm">{t("person.compared", c.name)}</span>
          </label>
          <div>
            <Button
              size="sm"
              variant="primary"
              disabled={!checked}
              onClick={() => {
                setChecked(false);
                fire(markVerified(c.name));
              }}
            >
              <Icon.Shield size={13} /> {t("person.markVerified")}
            </Button>
          </div>
        </div>
      )}

      <Field label={t("person.rename")} hint={t("person.renameHint")}>
        {({ id }) => (
          <div className="hstack-sm">
            <TextInput
              id={id}
              value={newName}
              onChange={(e) => setNewName(e.currentTarget.value)}
              placeholder={c.name}
            />
            <Button
              size="sm"
              disabled={!newName.trim() || newName.trim() === c.name}
              onClick={async () => {
                const to = newName.trim();
                await renameContact(c.name, to);
                setNewName("");
                close(to);
              }}
            >
              {t("person.rename")}
            </Button>
          </div>
        )}
      </Field>
    </Sheet>
  );
}

// ---------------------------------------------------------------------------

/** Everything the address book can *do* besides read itself: the two file
 *  dialogs and the add-by-id sheet's switch. A hook rather than part of the
 *  screen because two components need it: the screen, whose desktop toolbar
 *  calls these directly, and the bar menu the phone gets instead. */
function usePeopleActions() {
  const t = useT();
  const contacts = useStore((s) => s.contacts);
  const addContact = useStore((s) => s.addContact);
  const [addOpen, setAddOpen] = useState(false);
  const [importing, setImporting] = useState(false);
  const busyRef = useRef(false);

  /** Export mirrors `arvolo contacts export` byte for byte: name, id and the two
   *  marks, and nothing that describes *this* machine. */
  const doExport = async () => {
    const rows = contacts.map((c) => ({
      name: c.name,
      id: c.id,
      verified: c.verified,
      trusted: c.trusted,
    }));
    try {
      // The dialog happens on the native side; cancelling resolves to null.
      const saved = await api.exportContacts(
        t("people.exportFilename"),
        JSON.stringify(rows, null, 2)
      );
      if (saved === null) return;
      toast.ok(
        rows.length === 1
          ? t("people.exportedOne")
          : t("people.exportedMany", rows.length),
        t("people.exportDetail")
      );
    } catch (e) {
      toast.bad(t("people.exportFailed"), String(e));
    }
  };

  const doImport = async () => {
    if (busyRef.current) return;
    const text = await api.importContacts();
    if (text === null) return; // cancelled in the native dialog
    busyRef.current = true;
    setImporting(true);
    try {
      const rows = JSON.parse(text);
      if (!Array.isArray(rows)) throw new Error(t("people.importNotAList"));

      const existing = new Set(contacts.map((c) => c.name));
      let added = 0;
      let skipped = 0;
      for (const row of rows) {
        const name = typeof row?.name === "string" ? row.name : null;
        const id = typeof row?.id === "string" ? row.id : null;
        if (!name || !id) continue;
        // Never rebind a name already in the book: an import that overwrote an
        // existing contact's key would be a key change nobody asked about.
        if (existing.has(name)) {
          skipped++;
          continue;
        }
        try {
          await addContact(name, id);
          added++;
          existing.add(name);
        } catch {
          skipped++;
        }
      }
      toast.ok(
        added === 1
          ? t("people.importedOne")
          : t("people.importedMany", added),
        t("people.importDetail", skipped)
      );
    } catch (e) {
      toast.bad(t("people.importFailed"), String(e));
    } finally {
      busyRef.current = false;
      setImporting(false);
    }
  };

  return { contacts, addOpen, setAddOpen, importing, doExport, doImport };
}

/** The address book's verbs, where this platform keeps a screen's verbs: a "+"
 *  in the bar, opening a menu. A filled button the width of the screen sitting
 *  above the list was the loudest thing on it, and it was shouting about the
 *  rarest thing done there — the list itself is what the screen is for.
 *
 *  Rendered by the phone header when the People tab is up, so the menu's sheet
 *  has to come along: this component is the verbs *and* the screens they open. */
export function PeopleBarMenu() {
  const t = useT();
  const startPairing = useStore((s) => s.startPairing);
  const { contacts, addOpen, setAddOpen, doExport, doImport } =
    usePeopleActions();

  // «Scambia contatti» is the card at the top of the screen, and only there:
  // the menu holds the other ways in.
  const items: MenuItem[] = [
    {
      key: "have-code",
      label: t("people.haveCode"),
      icon: <Icon.Qr size={13} />,
      onSelect: () => fire(startPairing("contact_join")),
    },
    {
      key: "by-id",
      label: t("people.byId"),
      icon: <Icon.Plus size={13} />,
      onSelect: () => setAddOpen(true),
    },
    {
      key: "import",
      label: t("people.import"),
      icon: <Icon.Folder size={13} />,
      onSelect: () => doImport(),
      separated: true,
    },
    {
      key: "export",
      label: t("people.export"),
      icon: <Icon.Copy size={13} />,
      onSelect: () => doExport(),
      disabled: !contacts.length,
    },
  ];

  return (
    <>
      <MenuButton items={items} label={t("people.moreActions")}>
        <Icon.Plus size={17} />
      </MenuButton>
      <AddPersonSheet open={addOpen} onClose={() => setAddOpen(false)} />
    </>
  );
}

export function PeopleView() {
  const t = useT();
  const push = useStore((s) => s.push);
  const contacts = useStore((s) => s.contacts);
  const startPairing = useStore((s) => s.startPairing);
  const loadPresence = useStore((s) => s.loadPresence);
  const presenceLoading = useStore((s) => s.presenceLoading);

  // Presence is a photograph, and people come and go while the window sits
  // open, so one probe on arrival is not enough: a dot left alone goes quietly
  // wrong and nothing in the UI admits it. Hence a probe on arrival *and* on a
  // timer, plus one the moment the window comes back — that is the instant the
  // reading is both most stale and most likely to be read.
  //
  // The dependency is the id list, not the route: navigation and the address
  // book landing are two different moments — the snapshot is still in flight on
  // a fresh launch — and probing at the first one asks the relay about an empty
  // list. Mounting re-runs this anyway, so opening the screen still refreshes.
  const ids = contacts.map((c) => c.id).join(",");
  useEffect(() => {
    if (!ids) return;
    // A hidden window asks for nothing: nobody is reading the dots, and the
    // answer would be stale again by the time it is.
    const probe = () => {
      if (!document.hidden) fire(loadPresence());
    };
    probe();
    const timer = window.setInterval(probe, PRESENCE_REFRESH_MS);
    document.addEventListener("visibilitychange", probe);
    window.addEventListener("focus", probe);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", probe);
      window.removeEventListener("focus", probe);
    };
  }, [ids, loadPresence]);

  const [filter, setFilter] = useState<Filter>("all");
  const [q, setQ] = useState("");
  /** The phone's search is a glyph beside the title (12a); this is whether the
   *  field is out. Closing it also clears the filter. */
  const [finding, setFinding] = useState(false);
  const { addOpen, setAddOpen, importing, doExport, doImport } =
    usePeopleActions();

  const isPhone = useIsPhone();

  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return contacts.filter((c) => {
      if (filter === "verified" && !c.verified) return false;
      if (filter === "trusted" && !c.trusted) return false;
      if (filter === "blocked" && !c.blocked) return false;
      if (filter !== "blocked" && c.blocked) return false;
      if (!needle) return true;
      return (
        c.name.toLowerCase().includes(needle) ||
        c.display_name.toLowerCase().includes(needle) ||
        c.pending_name.toLowerCase().includes(needle) ||
        c.id.toLowerCase().startsWith(needle)
      );
    });
  }, [contacts, filter, q]);

  const blockedCount = contacts.filter((c) => c.blocked).length;

  // The phone's two groups. `presence` is a three-valued answer — yes, no, and
  // the relay did not say — and only the first of the three earns the top
  // group: a dot nobody could confirm is not a person you can reach now.
  const presence = useStore((s) => s.presence);
  const online = shown.filter((c) => presence[c.id] === true);
  const offline = shown.filter((c) => presence[c.id] !== true);

  return (
    <div className="stack">
      {/* One way in, at the top of the screen and filled, with what it costs
          written under it. The design of record (7i) is firm about this being
          *one* thing rather than two: swapping codes is what two people do while
          standing in front of each other, and it is the only one of the five
          ways into the address book that is not an errand. The other four —
          typing a code you were given, adding a bare id, importing, exporting —
          are in the bar's menu, which is where errands go.

          It was two buttons side by side, and the second one ("I have a code")
          read as the equal-weight alternative it is not: it is the other half of
          the same exchange, reached from the other person's screen. */}
      {/* La testa della schermata (sezione 12): il titolo, le vie per entrare
          in rubrica, e l'ingranaggio. */}
      {isPhone && (
        <div className="root-head">
          <h2 className="large-title">{t("title.people")}</h2>
          <div className="root-tools">
            <button
              className="act-tool"
              aria-label={t("people.searchLabel")}
              aria-pressed={finding}
              onClick={() => {
                if (finding) setQ("");
                setFinding((v) => !v);
              }}
            >
              <Icon.Search size={19} />
            </button>
            <span className="tool-wrap">
              <PeopleBarMenu />
            </span>
            <button
              className="act-tool"
              aria-label={t("title.settings")}
              onClick={() => push("settings")}
            >
              <Icon.Settings size={20} />
            </button>
          </div>
        </div>
      )}
      {isPhone && (
        <button
          className="people-swap"
          onClick={() => fire(startPairing("contact_host"))}
        >
          <Icon.Qr size={20} />
          <span className="grow" style={{ minWidth: 0 }}>
            <span className="people-swap-title">{t("people.swap")}</span>
            <span className="people-swap-sub">{t("people.swapBoth")}</span>
          </span>
          <Icon.ChevronRight size={16} />
        </button>
      )}

      {!isPhone && (
      <div className="hstack wrap">
        <Button variant="primary" onClick={() => fire(startPairing("contact_host"))}>
          <Icon.Qr size={14} /> {t("people.swap")}
        </Button>
        <Button onClick={() => fire(startPairing("contact_join"))}>
          {t("people.haveCode")}
        </Button>
        <Button onClick={() => setAddOpen(true)}>
          <Icon.Plus size={13} /> {t("people.byId")}
        </Button>
        <div className="spacer grow" />
        <Button size="sm" onClick={doExport} disabled={!contacts.length}>
          {t("people.export")}
        </Button>
        <Button size="sm" onClick={doImport} busy={importing}>
          {t("people.import")}
        </Button>
        <Button
          size="sm"
          onClick={() => fire(loadPresence())}
          busy={presenceLoading}
          title={t("people.whoIsOnlineTitle")}
        >
          <Icon.Relay size={13} /> {t("people.whoIsOnline")}
        </Button>
      </div>
      )}

      <div className={isPhone ? "stack-sm people-find" : "hstack wrap"}>
        <Segmented
          block={isPhone}
          label={t("people.filterLabel")}
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: t("people.filterAll") },
            { value: "verified", label: t("people.filterVerified") },
            { value: "trusted", label: t("people.filterTrusted") },
            {
              value: "blocked",
              label: blockedCount
                ? t("people.filterBlockedN", blockedCount)
                : t("people.filterBlocked"),
            },
          ]}
        />
        {(!isPhone || finding) && (
          <div className="grow" style={isPhone ? undefined : { maxWidth: 280 }}>
            <TextInput
              autoFocus={isPhone}
              value={q}
              onChange={(e) => setQ(e.currentTarget.value)}
              placeholder={t("people.searchPlaceholder")}
              aria-label={t("people.searchLabel")}
            />
          </div>
        )}
      </div>

      {shown.length === 0 ? (
        <div className={isPhone ? "act-empty" : "card"}>
          <Empty
            icon={<Icon.People size={22} />}
            title={
              contacts.length === 0
                ? t("people.emptyNone")
                : t("people.emptyNoMatch")
            }
            action={
              contacts.length === 0 ? (
                // The quickest way first, and the fallback under it — the design
                // of record's 7r, and the same pair of stacked buttons an empty
                // Attività offers. On a desktop the row of ways in is above the
                // list already, so one button is enough there.
                isPhone ? (
                  <div className="empty-actions">
                    <Button
                      size="lg"
                      className="btn-ink"
                      onClick={() => fire(startPairing("contact_host"))}
                    >
                      <Icon.Qr size={15} /> {t("people.swap")}
                    </Button>
                    <Button size="lg" onClick={() => setAddOpen(true)}>
                      <Icon.Plus size={15} /> {t("people.byId")}
                    </Button>
                  </div>
                ) : (
                  <Button
                    variant="primary"
                    onClick={() => fire(startPairing("contact_host"))}
                  >
                    <Icon.Qr size={14} /> {t("people.swap")}
                  </Button>
                )
              ) : undefined
            }
          >
            {contacts.length === 0
              ? t("people.emptyNoneBody")
              : t("people.emptyNoMatchBody")}
          </Empty>
        </div>
      ) : isPhone ? (
        /* Two groups, and the first one is the answer to the question a phone
           is usually holding: is this person reachable *right now*, or is this
           going to sit in their mailbox. One list sorted by name makes that a
           thing you read off forty dots one at a time. Presence the relay could
           not answer for is not online — it goes below, where the row says so
           in words. */
        <>
          {[
            { key: "on", title: t("people.groupOnline"), rows: online },
            { key: "off", title: t("people.groupOffline"), rows: offline },
          ]
            .filter((g) => g.rows.length > 0)
            .map((g) => (
              <div key={g.key}>
                <div className="ios-head hstack-sm">
                  <span>{g.title}</span>
                  <span className="count-soft tnum">{g.rows.length}</span>
                </div>
                <div className="ios-group">
                  {g.rows.map((c) => (
                    <PersonRow key={c.name} c={c} />
                  ))}
                </div>
              </div>
            ))}
        </>
      ) : (
        <div className="card rows">
          {shown.map((c) => (
            <PersonRow key={c.name} c={c} />
          ))}
        </div>
      )}

      <AddPersonSheet open={addOpen} onClose={() => setAddOpen(false)} />
      <PersonSheet />
    </div>
  );
}
