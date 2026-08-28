// Sending, in one panel with four answers to "who is this for?".
//
// The CLI has one verb with four modes here — `send --to`, `send --code`,
// `send --link`, and the bare `send` that writes a `.arvolo` ticket file — and
// they are genuinely different things, not options on one thing: one needs a
// saved contact, one needs the other person present *now*, one needs nothing
// but a browser, one needs no relay at all. Presenting them as a mode switch
// rather than as four buttons on the rail makes the choice visible without
// making it four separate places to learn.
//
// The mailbox options (scadenza, download, password) appear only under "a un
// contatto", and only once "lascia in casella" is on. That is not tidiness: TTL
// and password apply to a *deposit* and to nothing else, and showing them on a
// live P2P send would promise a protection that is not being applied.

import { useEffect, useMemo, useState } from "react";
import { api } from "../ipc";
import { SELF_RECIPIENT, useStore } from "../store";
import { useIsPhone } from "../phone";
import type { SendMode } from "../store";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import {
  Badge,
  Button,
  Field,
  Segmented,
  Switch,
  TextInput,
  Textarea,
  TrustBadges,
} from "../ui/Primitives";
import { Avatar, ClaimedName, CodeHero } from "../ui/Bits";
import { Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import type { ContactDto, DeviceDto, PickedItem } from "../types";

/** Re-exported name for the store's `SendMode`, so the switch below and the
 *  callers that preselect it can never drift apart. */
type Mode = SendMode;

const MODE_KEY = {
  contact: "send.modeContact",
  self: "send.modeSelf",
  code: "send.modeCode",
  link: "send.modeLink",
  ticket: "send.modeTicket",
} as const satisfies Record<Mode, string>;

// Your own devices sit next to "a un contatto" because those two are the ones
// that go to somebody named; the other three are ways of handing something over
// to whoever turns up.
const MODE_ORDER: Mode[] = ["contact", "self", "code", "link", "ticket"];

/** The blurb under the mode switch, on two levels. The first line says what the
 *  *recipient* needs, which is the only thing that decides which mode is right;
 *  the second says what it costs, for whoever is still choosing. Somebody who
 *  already knows reads four short lines instead of four paragraphs. */
const BLURB = {
  contact: "send.blurbContact",
  self: "send.blurbSelf",
  code: "send.blurbCode",
  link: "send.blurbLink",
  ticket: "send.blurbTicket",
} as const satisfies Record<Mode, string>;

const BLURB_MORE = {
  contact: "send.blurbContactMore",
  self: "send.blurbSelfMore",
  code: "send.blurbCodeMore",
  link: "send.blurbLinkMore",
  ticket: "send.blurbTicketMore",
} as const satisfies Record<Mode, string>;

const TTL_CHOICES = [
  { secs: 3600, key: "send.ttl1h" },
  { secs: 24 * 3600, key: "send.ttl1d" },
  { secs: 7 * 24 * 3600, key: "send.ttl7d" },
  { secs: 30 * 24 * 3600, key: "send.ttl30d" },
] as const;

interface Result {
  kind: "code" | "link" | "ticket" | "deposit" | "sent";
  value: string;
  detail?: string;
}

/** The two "quanti download" fields accept exactly two things: empty, or a whole
 *  number of at least one. Anything else is not a value the daemon can take —
 *  the IPC field is a `u32`, so a decimal or a minus sign is a round-trip error,
 *  and a stray letter used to turn into `NaN` and then, silently, into "no limit
 *  at all". Filtering as the user types keeps the invalid state from existing at
 *  all, which is why the submit path needs no second opinion about it.
 *
 *  Leading zeros go too: they make "0" — a deposit nobody may ever collect —
 *  unreachable, and "007" reads back as the 7 it will actually be. Nine digits
 *  is the ceiling because ten of them can overflow the `u32` on the wire, and a
 *  billion downloads is already "no limit" by any reading. */
function digitsOnly(raw: string): string {
  return raw.replace(/\D/g, "").replace(/^0+/, "").slice(0, 9);
}

/** This identity's own machines, as a destination. */
interface SelfTarget {
  fingerprint: string;
  /** False when nothing yet suggests a second machine holds this identity — the
   *  send still works, it just has nowhere to land until one is paired. */
  paired: boolean;
}

/** "Ai tuoi dispositivi": which of them, and the caveats.
 *
 *  It briefly held an identity card too — the destination's name, which the
 *  selected tab already says, over the fingerprint, which answers a question
 *  nobody asks *while sending*: the fingerprint is for comparing somebody else's
 *  identity out loud, so on this panel it read as a code to hand over, which is
 *  the one thing it is not. What is left is the choice and what it costs.
 *
 *  "Whichever is awake" stays the default, and not only because it is the old
 *  behaviour: aiming at a machine that turns out to be asleep means waiting for
 *  it, so choosing one should be a decision, never something the sheet did for
 *  you. */
function SelfPanel({
  self,
  devices,
  value,
  onChange,
}: {
  self: SelfTarget;
  devices: DeviceDto[];
  value: string | null;
  onChange: (device: string | null) => void;
}) {
  const t = useT();
  // Only the *other* machines are worth listing: this one does not take its own
  // offer (the sender skips it, by design), so aiming here would be a send that
  // waits for a machine that is already here.
  const others = devices.filter((d) => !d.is_this_device);
  return (
    <div className="stack-sm">
      {!self.paired && (
        <div
          className="card card-pad t-xs"
          style={{
            borderColor: "var(--amber)",
            background: "var(--amber-soft)",
          }}
        >
          {t("send.selfUnpairedHint")}
        </div>
      )}

      {others.length > 0 && (
        <Field label={t("send.deviceLabel")}>
          {() => (
            <div className="stack-sm">
              <div
                className="card rows"
                style={{ maxHeight: 200, overflowY: "auto" }}
                role="radiogroup"
                aria-label={t("send.deviceLabel")}
              >
                <DeviceChoice
                  label={t("send.deviceAny")}
                  meta={null}
                  selected={value === null}
                  onClick={() => onChange(null)}
                />
                {others.map((d) => (
                  <DeviceChoice
                    key={d.id}
                    label={d.name}
                    meta={d.platform || null}
                    selected={value === d.name}
                    onClick={() => onChange(d.name)}
                  />
                ))}
              </div>
              <div className="hint">
                {value === null
                  ? t("send.deviceAnyHint")
                  : t("send.deviceOneHint")}
              </div>
            </div>
          )}
        </Field>
      )}
    </div>
  );
}

function DeviceChoice({
  label,
  meta,
  selected,
  onClick,
}: {
  label: string;
  meta: string | null;
  selected: boolean;
  onClick: () => void;
}) {
  return (
    <button
      role="radio"
      aria-checked={selected}
      onClick={onClick}
      className="row"
      style={{
        gridTemplateColumns: "1fr auto",
        border: 0,
        width: "100%",
        textAlign: "left",
        cursor: "pointer",
        background: selected ? "var(--out-soft)" : "var(--surface)",
      }}
    >
      <span className="row-main">
        <span className="row-name truncate">{label}</span>
        {meta && <span className="row-meta">{meta}</span>}
      </span>
      <span className="hstack-sm">
        {selected && <Icon.Check size={14} className="tone-out" />}
      </span>
    </button>
  );
}

function ContactPicker({
  contacts,
  value,
  onChange,
}: {
  contacts: ContactDto[];
  value: string;
  onChange: (to: string) => void;
}) {
  const t = useT();
  const [q, setQ] = useState("");
  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const usable = contacts.filter((c) => !c.blocked);
    if (!needle) return usable;
    return usable.filter(
      (c) =>
        c.name.toLowerCase().includes(needle) ||
        c.display_name.toLowerCase().includes(needle) ||
        c.pending_name.toLowerCase().includes(needle) ||
        c.id.toLowerCase().startsWith(needle)
    );
  }, [contacts, q]);

  if (!contacts.length) {
    return (
      <div className="card card-pad">
        <div style={{ fontWeight: 570 }}>{t("send.pickerEmptyTitle")}</div>
        <div className="t-sm t-sec" style={{ marginTop: 2 }}>
          {t("send.pickerEmpty")}
        </div>
      </div>
    );
  }

  return (
    <div className="stack-sm">
      <TextInput
        placeholder={t("send.pickerSearch")}
        value={q}
        onChange={(e) => setQ(e.currentTarget.value)}
        aria-label={t("send.pickerSearch")}
      />
      <div
        className="card rows"
        style={{ maxHeight: 232, overflowY: "auto" }}
        role="radiogroup"
        aria-label={t("send.pickerRecipient")}
      >
        {shown.length === 0 && (
          <div className="t-sm t-mut" style={{ padding: 14 }}>
            {t("send.pickerNoMatch", q)}
          </div>
        )}
        {shown.map((c) => {
          // What travels for this row. Normally the name — but a contact called
          // "me" would be read by the daemon as the reserved word and the file
          // would go to our own machines instead of to them. Their id says the
          // same thing with nothing reserved about it.
          const wire = c.name === SELF_RECIPIENT ? c.id : c.name;
          return (
            <button
              key={c.name}
              role="radio"
              aria-checked={value === wire}
              onClick={() => onChange(wire)}
              className="row"
              style={{
                gridTemplateColumns: "32px 1fr auto",
                border: 0,
                width: "100%",
                textAlign: "left",
                cursor: "pointer",
                background:
                  value === wire ? "var(--out-soft)" : "var(--surface)",
              }}
            >
              <Avatar name={c.display_name || c.name} id={c.id} size={32} />
              <span className="row-main">
                <span className="row-name hstack-sm">
                  <span className="truncate">{c.name}</span>
                  <ClaimedName c={c} />
                </span>
                <span className="row-meta mono" style={{ fontSize: 10.5 }}>
                  {c.fingerprint}
                </span>
              </span>
              <span className="hstack-sm">
                <TrustBadges verified={c.verified} trusted={c.trusted} />
                {value === wire && <Icon.Check size={14} className="tone-out" />}
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

export function SendSheet() {
  const isPhone = useIsPhone();
  const t = useT();
  const picks = useStore((s) => s.sheetPicks);
  const presetTo = useStore((s) => s.sheetTo);
  const presetMode = useStore((s) => s.sheetMode);
  const close = useStore((s) => s.closeSheet);
  const contacts = useStore((s) => s.contacts);
  const send = useStore((s) => s.send);
  const depositAction = useStore((s) => s.deposit);
  const ticket = useStore((s) => s.ticket);
  const code = useStore((s) => s.code);
  const link = useStore((s) => s.link);
  const relay = useStore((s) => s.status?.relay ?? null);
  const status = useStore((s) => s.status);
  const sync = useStore((s) => s.sync);
  const loadSync = useStore((s) => s.loadSync);
  const devices = useStore((s) => s.devices);
  const loadDevices = useStore((s) => s.loadDevices);
  const contactsById = useStore((s) => s.contactsById);

  const [mode, setMode] = useState<Mode>("contact");
  const [files, setFiles] = useState<PickedItem[]>([]);
  const [to, setTo] = useState("");
  const [note, setNote] = useState("");
  const [asDeposit, setAsDeposit] = useState(false);
  const [ttl, setTtl] = useState(7 * 24 * 3600);
  // Separate per mode: the mailbox default is 1 pickup, a link's is unlimited.
  // One shared field made a link die after the first download while its own
  // hint promised "no limits".
  const [maxDl, setMaxDl] = useState("1");
  const [maxDlLink, setMaxDlLink] = useState("");
  const [password, setPassword] = useState("");
  const [keepCode, setKeepCode] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<Result | null>(null);
  /** Which of our machines to aim at; `null` is "whichever is awake", which is
   *  what every send did before there was a register to choose from. */
  const [device, setDevice] = useState<string | null>(null);

  const open = picks !== null;

  // Reopening must not inherit the last send's answers — least of all its
  // password, which would silently protect a different file for a different
  // person. Everything resets except the files the sheet was opened with.
  useEffect(() => {
    if (!open) return;
    setFiles(picks ?? []);
    setTo(presetTo ?? "");
    // Opened from a section that already means one way of sending — honour it.
    setMode(presetMode ?? "contact");
    setResult(null);
    setBusy(false);
    setNote("");
    setPassword("");
    setAsDeposit(false);
    setKeepCode(false);
    setMaxDl("1");
    setMaxDlLink("");
    setDevice(null);
    setTtl(7 * 24 * 3600);
  }, [open, picks, presetTo, presetMode]);

  // Sync state is what the Devices screen loads, and the sheet can be the first
  // thing a session opens. Fetch it here so "no other machine holds this identity
  // yet" is a warning we can give rather than one we happen to have.
  useEffect(() => {
    if (open && !sync) void loadSync().catch(() => {});
  }, [open, sync, loadSync]);

  // The register, for the "which of them" list. Refetched whenever the sheet
  // opens rather than cached for the session: a machine paired two minutes ago
  // should be on the list the first time you look for it.
  useEffect(() => {
    if (open) void loadDevices().catch(() => {});
  }, [open, loadDevices]);

  /** Our own machines as a destination. Identity comes from `status`, which is
   *  loaded before anything paints; `sync` only decides the warning, so a slow or
   *  failed sync fetch costs the caveat, never the destination. */
  const self: SelfTarget | null = status
    ? { fingerprint: status.fingerprint, paired: sync?.paired ?? true }
    : null;
  const toSelf = mode === "self";

  /** The two modes that push to a named recipient, and therefore share the note,
   *  the mailbox toggle and everything under it. */
  const pushes = mode === "contact" || toSelf;

  /** Who this is going to, on the wire. In `self` mode the recipient is the mode
   *  — there is nothing to pick, and the reserved word is what the daemon
   *  resolves against our own identity. */
  const recipient = toSelf ? SELF_RECIPIENT : to;

  /** What that recipient is called on screen. Never the wire value: a contact
   *  called "me" travels as their id, and an id is not a name.
   *
   *  Only the contact mode needs it. Sending to yourself does not splice a name
   *  into a sentence at all — "In consegna a I tuoi dispositivi" is not Italian,
   *  and "Unterwegs zu Ihre Geräte" is not German. Those toasts are written
   *  whole, per language, below. */
  const toLabel = contactsById[to]?.name ?? to;

  /** The note only reaches somebody else. Cleared on the wire rather than on the
   *  keystroke, so text typed for a contact survives a look at another mode. */
  const outgoingNote = toSelf ? "" : note;

  const pick = async (directory: boolean) => {
    // The dialog is native and Rust-side; what comes back are registered ids
    // plus display names — never paths (see `bridge::PickedFiles`).
    const picked = await api.pickFiles(directory);
    if (picked.length === 0) return;
    setFiles((f) => {
      const seen = new Set(f.map((x: PickedItem) => x.id));
      return [...f, ...picked.filter((x) => !seen.has(x.id))];
    });
  };

  // Only "a un contatto" has a recipient left to choose: every other mode either
  // names one by being itself, or hands the file to whoever turns up.
  const canSubmit =
    files.length > 0 && (mode !== "contact" || to !== "") && !busy;
  // A link is deposited from a single payload; the daemon's CreateLink takes one
  // path. Packing several into an archive first is a send-side job the link path
  // does not do, so the sheet says so rather than silently sending only the first.
  const linkTooMany = mode === "link" && files.length > 1;

  const submit = async () => {
    setBusy(true);
    try {
      if (pushes) {
        if (asDeposit) {
          // `digitsOnly` already ruled out everything but "" and a positive
          // integer, so empty is the only case left to map — and it means "let
          // the daemon pick", which is one pickup.
          const r = await depositAction(
            recipient,
            files.map((f) => f.id),
            outgoingNote,
            ttl,
            maxDl === "" ? null : Number(maxDl),
            password || null,
            device
          );
          setResult({
            kind: "deposit",
            value: r.ticket,
            detail: toSelf
              ? t("send.depositResultSelfDetail")
              : t("send.depositResultDetail", toLabel),
          });
          // The outcome as a toast, the way the live send already reports it;
          // the caption under the ticket then only has to answer "why am I
          // being shown this at all", which is the part that is not obvious.
          toast.ok(
            toSelf
              ? t("send.depositResultSelf")
              : t("send.depositResult", toLabel)
          );
        } else {
          await send(recipient, files.map((f) => f.id), outgoingNote, device);
          toast.ok(
            toSelf ? t("send.onItsWaySelf") : t("send.onItsWay", toLabel),
            // Sending to yourself has a different "and now?": no one has to
            // accept it, but a machine has to be awake to take it.
            toSelf ? t("send.onItsWaySelfDetail") : t("send.onItsWayDetail")
          );
          close();
        }
      } else if (mode === "code") {
        const r = await code(files.map((f) => f.id), keepCode);
        setResult({
          kind: "code",
          value: r.code,
          detail: keepCode
            ? t("send.codeKeepDetail")
            : t("send.codeOnceDetail"),
        });
      } else if (mode === "link") {
        // Empty here is the deliberate "illimitati" the placeholder promises,
        // and now the *only* way to ask for it.
        const url = await link(
          files[0].id,
          ttl,
          maxDlLink === "" ? null : Number(maxDlLink)
        );
        setResult({
          kind: "link",
          value: url,
          detail: t("send.linkDetail"),
        });
      } else {
        const r = await ticket(files.map((f) => f.id));
        setResult({
          kind: "ticket",
          value: r.ticket,
          detail: t("send.ticketDetail"),
        });
      }
    } catch {
      // `act` in the store already recorded the reason and raised the toast.
    } finally {
      setBusy(false);
    }
  };

  const totalNote =
    files.length === 0
      ? null
      : files.length === 1
        ? t("send.countOne")
        : t("send.countMany", files.length);

  return (
    <Sheet
      open={open}
      onClose={close}
      title={result ? t("send.titleReady") : t("send.title")}
      // No subtitle once there is a result: the thing to hand over is right
      // there under it, and "Hand over what you see below" only said so again.
      subtitle={result ? undefined : t("send.subtitle")}
      footer={
        result ? (
          <>
            <div className="spacer" />
            <Button variant="primary" onClick={close}>
              {t("common.done")}
            </Button>
          </>
        ) : (
          <>
            <span className="t-xs t-mut truncate">{totalNote}</span>
            <div className="spacer" />
            <Button onClick={close} disabled={busy}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="primary"
              onClick={submit}
              busy={busy}
              disabled={!canSubmit || linkTooMany}
            >
              <Icon.Send size={14} />
              {pushes
                ? asDeposit
                  ? t("send.submitDeposit")
                  : t("send.submitSend")
                : mode === "code"
                  ? t("send.submitCode")
                  : mode === "link"
                    ? t("send.submitLink")
                    : t("send.submitTicket")}
            </Button>
          </>
        )
      }
    >
      {result ? (
        <div className="stack">
          {result.kind === "sent" ? null : (
            <CodeHero
              value={result.value}
              small={result.kind !== "code"}
              caption={result.detail}
            />
          )}
          {result.kind === "link" && (
            <div className="t-sm t-sec">{t("send.linkKeyNote")}</div>
          )}
          {/* A ticket is a blob nobody retypes: offer it as a `.arvolo` file —
              the CLI's own default artefact — shareable over any channel. */}
          {result.kind === "ticket" && (
            <Button
              onClick={async () => {
                const base =
                  files[0]?.name?.replace(/[/\\]/g, "_") || "ticket";
                const saved = await api.saveTicket(
                  `${base}.arvolo`,
                  result.value
                );
                if (saved)
                  toast.ok(t("send.arvoloSaved", saved), t("send.arvoloSavedDetail"));
              }}
            >
              {t("send.saveArvolo")}
            </Button>
          )}
        </div>
      ) : (
        <div className="stack">
          {/* --- files ------------------------------------------------- */}
          <Field
            label={t("send.filesLabel")}
            hint={isPhone ? undefined : t("send.filesHint")}
          >
            {() => (
              <div className="stack-sm">
                {files.length > 0 && (
                  <div className="card rows" style={{ maxHeight: 160, overflowY: "auto" }}>
                    {files.map((p) => (
                      <div
                        key={p.id}
                        className="hstack"
                        style={{ padding: "8px 11px" }}
                      >
                        <Icon.Folder size={14} className="t-mut" />
                        <span className="grow truncate t-sm" title={p.name}>
                          {p.name}
                        </span>
                        <button
                          className="icon-btn"
                          aria-label={t("send.filesRemove", p.name)}
                          onClick={() =>
                            setFiles((f) => f.filter((x) => x.id !== p.id))
                          }
                        >
                          <Icon.Close size={13} />
                        </button>
                      </div>
                    ))}
                  </div>
                )}
                <div className="hstack-sm">
                  <Button size="sm" onClick={() => pick(false)}>
                    <Icon.Plus size={13} /> {t("send.pickFiles")}
                  </Button>
                  <Button size="sm" onClick={() => pick(true)}>
                    <Icon.Folder size={13} /> {t("send.pickFolder")}
                  </Button>
                </div>
              </div>
            )}
          </Field>

          {/* --- mode -------------------------------------------------- */}
          <Field label={t("send.whoLabel")}>
            {() => (
              <div className="stack-sm">
                <Segmented
                  block
                  wrap
                  label={t("send.modeLabel")}
                  value={mode}
                  onChange={setMode}
                  options={MODE_ORDER.map((m) => ({
                    value: m,
                    label: t(MODE_KEY[m]),
                  }))}
                />
                <div className="hint">
                  {t(BLURB[mode])}
                  <div className="t-xs t-mut">{t(BLURB_MORE[mode])}</div>
                </div>
              </div>
            )}
          </Field>

          {mode === "contact" && (
            <ContactPicker contacts={contacts} value={to} onChange={setTo} />
          )}

          {toSelf && self && (
            <SelfPanel
              self={self}
              devices={devices}
              value={device}
              onChange={setDevice}
            />
          )}

          {/* No note when the file is for you. It rides inside the offer, and the
              only screen that ever shows one is the accept/reject dialog — which
              never opens for your own identity, because an offer from it is
              auto-accepted. The field would have collected words nobody reads. */}
          {(mode === "contact" || mode === "code") && (
            <Field
              label={t("send.noteLabel")}
              hint={t("send.noteHint")}
            >
              {({ id, describedBy }) => (
                <Textarea
                  id={id}
                  aria-describedby={describedBy}
                  value={note}
                  maxLength={280}
                  onChange={(e) => setNote(e.currentTarget.value)}
                  placeholder={t("send.notePlaceholder")}
                />
              )}
            </Field>
          )}

          {mode === "code" && (
            <div className="switch-row">
              <div className="grow">
                <div style={{ fontWeight: 570 }}>{t("send.keepCodeTitle")}</div>
                <div className="hint">{t("send.keepCodeDesc")}</div>
              </div>
              <Switch
                checked={keepCode}
                onChange={setKeepCode}
                label={t("send.keepCodeLabel")}
              />
            </div>
          )}

          {pushes && (
            <>
              <div className="switch-row">
                <div className="grow">
                  <div style={{ fontWeight: 570 }}>
                    {t("send.depositTitle")}
                  </div>
                  <div className="hint">{t("send.depositDesc")}</div>
                </div>
                <Switch
                  checked={asDeposit}
                  onChange={setAsDeposit}
                  label={t("send.depositLabel")}
                />
              </div>

              {asDeposit && (
                <div
                  className="stack"
                  style={{
                    padding: 14,
                    borderRadius: "var(--r-md)",
                    background: "var(--surface-2)",
                    border: "1px solid var(--line)",
                  }}
                >
                  {/* A deposit is sealed once and collected once, and to your own
                      machines that is a trap the field below can spring: the
                      default of one pickup means the first machine to wake takes
                      it and the others never see it. Say it where the number is. */}
                  {toSelf && (
                    <div className="t-xs t-sec">{t("send.selfDepositHint")}</div>
                  )}
                  <Field label={t("send.expiresAfter")}>
                    {() => (
                      <Segmented
                        block
                        label={t("send.depositTtlLabel")}
                        value={String(ttl)}
                        onChange={(v) => setTtl(Number(v))}
                        options={TTL_CHOICES.map((c) => ({
                          value: String(c.secs),
                          label: t(c.key),
                        }))}
                      />
                    )}
                  </Field>
                  <Field
                    label={t("send.maxPickupsLabel")}
                    hint={t("send.maxPickupsHint")}
                  >
                    {({ id, describedBy }) => (
                      <TextInput
                        id={id}
                        aria-describedby={describedBy}
                        className="tnum"
                        inputMode="numeric"
                        value={maxDl}
                        onChange={(e) =>
                          setMaxDl(digitsOnly(e.currentTarget.value))
                        }
                        /* Empty is now reachable (the "1" can be deleted) and
                           means the daemon's default, which is this. A bare
                           digit needs no translation. */
                        placeholder="1"
                      />
                    )}
                  </Field>
                  <Field
                    label={t("send.passwordLabel")}
                    hint={t("send.passwordHint")}
                  >
                    {({ id, describedBy }) => (
                      <TextInput
                        id={id}
                        aria-describedby={describedBy}
                        type="password"
                        autoComplete="new-password"
                        value={password}
                        onChange={(e) => setPassword(e.currentTarget.value)}
                        placeholder={t("send.passwordPlaceholder")}
                      />
                    )}
                  </Field>
                </div>
              )}
            </>
          )}

          {mode === "link" && (
            <div className="stack">
              {linkTooMany && (
                <div className="card card-pad t-sm" style={{ borderColor: "var(--amber)" }}>
                  {t("send.linkTooMany")}
                </div>
              )}
              <Field label={t("send.expiresAfter")}>
                {() => (
                  <Segmented
                    block
                    label={t("send.linkTtlLabel")}
                    value={String(ttl)}
                    onChange={(v) => setTtl(Number(v))}
                    options={TTL_CHOICES.map((c) => ({
                      value: String(c.secs),
                      label: t(c.key),
                    }))}
                  />
                )}
              </Field>
              <Field
                label={t("send.maxDownloadsLabel")}
                hint={t("send.maxDownloadsHint")}
              >
                {({ id, describedBy }) => (
                  <TextInput
                    id={id}
                    aria-describedby={describedBy}
                    className="tnum"
                    inputMode="numeric"
                    value={maxDlLink}
                    onChange={(e) =>
                      setMaxDlLink(digitsOnly(e.currentTarget.value))
                    }
                    placeholder={t("send.maxDownloadsPlaceholder")}
                  />
                )}
              </Field>
            </div>
          )}

          {(mode === "code" || mode === "link") && !relay && (
            <div className="card card-pad card-err t-sm">
              {t("send.noRelay")}
            </div>
          )}

          {mode === "ticket" && (
            <div className="hstack-sm">
              <Badge kind="info">
                <Icon.Lock size={10} /> {t("send.noArvoloRelay")}
              </Badge>
            </div>
          )}
        </div>
      )}
    </Sheet>
  );
}

