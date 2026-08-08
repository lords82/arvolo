// Sending, in one panel with four answers to "who is this for?".
//
// The CLI has four verbs here — `send --to`, `code`, `link`, `ticket` — and they
// are genuinely different things, not options on one thing: one needs a saved
// contact, one needs the other person present *now*, one needs nothing but a
// browser, one needs no relay at all. Presenting them as a mode switch rather
// than as four buttons on the rail makes the choice visible without making it
// four separate places to learn.
//
// The mailbox options (scadenza, download, password) appear only under "a un
// contatto", and only once "lascia in casella" is on. That is not tidiness: TTL
// and password apply to a *deposit* and to nothing else, and showing them on a
// live P2P send would promise a protection that is not being applied.

import { useEffect, useMemo, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
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
import { Avatar, CodeHero } from "../ui/Bits";
import { Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import type { ContactDto } from "../types";

/** Re-exported name for the store's `SendMode`, so the switch below and the
 *  callers that preselect it can never drift apart. */
type Mode = SendMode;

const MODE_KEY = {
  contact: "send.modeContact",
  code: "send.modeCode",
  link: "send.modeLink",
  ticket: "send.modeTicket",
} as const satisfies Record<Mode, string>;

const MODE_ORDER: Mode[] = ["contact", "code", "link", "ticket"];

/** The blurb under the mode switch. Each says what the recipient needs, because
 *  that is the only thing that actually decides which mode is right. */
const BLURB = {
  contact: "send.blurbContact",
  code: "send.blurbCode",
  link: "send.blurbLink",
  ticket: "send.blurbTicket",
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

function basename(p: string): string {
  return p.split(/[/\\]/).pop() || p;
}

function ContactPicker({
  contacts,
  value,
  onChange,
}: {
  contacts: ContactDto[];
  value: string;
  onChange: (name: string) => void;
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
        c.id.toLowerCase().startsWith(needle)
    );
  }, [contacts, q]);

  if (!contacts.length) {
    return (
      <div className="card card-pad t-sm t-sec">{t("send.pickerEmpty")}</div>
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
        {shown.map((c) => (
          <button
            key={c.name}
            role="radio"
            aria-checked={value === c.name}
            onClick={() => onChange(c.name)}
            className="row"
            style={{
              gridTemplateColumns: "32px 1fr auto",
              border: 0,
              width: "100%",
              textAlign: "left",
              cursor: "pointer",
              background:
                value === c.name ? "var(--out-soft)" : "var(--surface)",
            }}
          >
            <Avatar name={c.display_name || c.name} id={c.id} size={32} />
            <span className="row-main">
              <span className="row-name truncate">{c.name}</span>
              <span className="row-meta mono" style={{ fontSize: 10.5 }}>
                {c.fingerprint}
              </span>
            </span>
            <span className="hstack-sm">
              <TrustBadges verified={c.verified} trusted={c.trusted} />
              {value === c.name && <Icon.Check size={14} className="tone-out" />}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

export function SendSheet() {
  const t = useT();
  const paths = useStore((s) => s.sheetPaths);
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

  const [mode, setMode] = useState<Mode>("contact");
  const [files, setFiles] = useState<string[]>([]);
  const [to, setTo] = useState("");
  const [note, setNote] = useState("");
  const [asDeposit, setAsDeposit] = useState(false);
  const [ttl, setTtl] = useState(7 * 24 * 3600);
  const [maxDl, setMaxDl] = useState("1");
  const [password, setPassword] = useState("");
  const [keepCode, setKeepCode] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<Result | null>(null);

  const open = paths !== null;

  // Reopening must not inherit the last send's answers — least of all its
  // password, which would silently protect a different file for a different
  // person. Everything resets except the files the sheet was opened with.
  useEffect(() => {
    if (!open) return;
    setFiles(paths ?? []);
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
    setTtl(7 * 24 * 3600);
  }, [open, paths, presetTo, presetMode]);

  const pick = async (directory: boolean) => {
    const picked = await openDialog({ multiple: true, directory });
    if (!picked) return;
    const list = Array.isArray(picked) ? picked : [picked];
    setFiles((f) => Array.from(new Set([...f, ...list])));
  };

  const canSubmit =
    files.length > 0 && (mode !== "contact" || to !== "") && !busy;
  // A link is deposited from a single payload; the daemon's CreateLink takes one
  // path. Packing several into an archive first is a send-side job the link path
  // does not do, so the sheet says so rather than silently sending only the first.
  const linkTooMany = mode === "link" && files.length > 1;

  const submit = async () => {
    setBusy(true);
    try {
      if (mode === "contact") {
        if (asDeposit) {
          const max = maxDl.trim() === "" ? null : Number(maxDl);
          const r = await depositAction(
            to,
            files,
            note,
            ttl,
            Number.isFinite(max as number) && max !== null ? max : null,
            password || null
          );
          setResult({
            kind: "deposit",
            value: r.ticket,
            detail: t("send.depositResult", to),
          });
        } else {
          await send(to, files, note);
          toast.ok(t("send.onItsWay", to), t("send.onItsWayDetail"));
          close();
        }
      } else if (mode === "code") {
        const r = await code(files, keepCode);
        setResult({
          kind: "code",
          value: r.code,
          detail: keepCode
            ? t("send.codeKeepDetail")
            : t("send.codeOnceDetail"),
        });
      } else if (mode === "link") {
        const max = maxDl.trim() === "" ? null : Number(maxDl);
        const url = await link(
          files[0],
          ttl,
          Number.isFinite(max as number) && max !== null ? max : null
        );
        setResult({
          kind: "link",
          value: url,
          detail: t("send.linkDetail"),
        });
      } else {
        const r = await ticket(files);
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
      subtitle={result ? t("send.subtitleReady") : t("send.subtitle")}
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
              {mode === "contact"
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
        </div>
      ) : (
        <div className="stack">
          {/* --- files ------------------------------------------------- */}
          <Field
            label={t("send.filesLabel")}
            hint={t("send.filesHint")}
          >
            {() => (
              <div className="stack-sm">
                {files.length > 0 && (
                  <div className="card rows" style={{ maxHeight: 160, overflowY: "auto" }}>
                    {files.map((p) => (
                      <div
                        key={p}
                        className="hstack"
                        style={{ padding: "8px 11px" }}
                      >
                        <Icon.Folder size={14} className="t-mut" />
                        <span className="grow truncate t-sm" title={p}>
                          {basename(p)}
                        </span>
                        <button
                          className="icon-btn"
                          aria-label={t("send.filesRemove", basename(p))}
                          onClick={() =>
                            setFiles((f) => f.filter((x) => x !== p))
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
                  label={t("send.modeLabel")}
                  value={mode}
                  onChange={setMode}
                  options={MODE_ORDER.map((m) => ({
                    value: m,
                    label: t(MODE_KEY[m]),
                  }))}
                />
                <div className="hint">{t(BLURB[mode])}</div>
              </div>
            )}
          </Field>

          {mode === "contact" && (
            <ContactPicker contacts={contacts} value={to} onChange={setTo} />
          )}

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

          {mode === "contact" && (
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
                        onChange={(e) => setMaxDl(e.currentTarget.value)}
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
                    value={maxDl}
                    onChange={(e) => setMaxDl(e.currentTarget.value)}
                    placeholder={t("send.maxDownloadsPlaceholder")}
                  />
                )}
              </Field>
            </div>
          )}

          {(mode === "code" || mode === "link") && !relay && (
            <div className="card card-pad t-sm" style={{ borderColor: "var(--red)" }}>
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

