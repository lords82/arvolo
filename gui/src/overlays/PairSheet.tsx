// Pairing, both kinds, in one sheet.
//
// The two are mechanically identical — a short SPAKE2 code over the relay's
// rendezvous — and consequentially nothing alike:
//
//   * **Contact pairing** trades *public* ids. Nobody learns anything the other
//     did not choose to send, and both ends come away saved *and verified*,
//     because a channel that only forms between two parties who knew the same
//     code authenticates whatever arrives through it.
//   * **Device pairing** hands over this device's *identity secret*. Afterwards
//     the two machines are the same person: one public id, one inbox. Joining
//     **replaces** the joining device's identity, and anything still sealed to
//     the old one stops being openable there.
//
// So the sheet never lets the two share a screen or a button. The device-host
// panel says what it is giving away before it shows a code, and the device-join
// panel refuses to start until the consequence has been ticked.

import { useEffect, useState } from "react";
import { useStore } from "../store";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import { Button, Field, TextInput } from "../ui/Primitives";
import { CodeHero } from "../ui/Bits";
import { Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import type { PairKind } from "../types";

const TITLE = {
  contact_host: "pair.titleContact",
  contact_join: "pair.titleContact",
  device_host: "pair.titleDeviceHost",
  device_join: "pair.titleDeviceJoin",
} as const satisfies Record<PairKind, string>;

const SUB = {
  contact_host: "pair.subContactHost",
  contact_join: "pair.subContactJoin",
  device_host: "pair.subDeviceHost",
  device_join: "pair.subDeviceJoin",
} as const satisfies Record<PairKind, string>;

export function PairSheet() {
  const t = useT();
  const pairing = useStore((s) => s.pairing);
  const start = useStore((s) => s.startPairing);
  const cancel = useStore((s) => s.cancelPairing);
  const clear = useStore((s) => s.clearPairing);
  const restartDaemon = useStore((s) => s.restartDaemon);
  const loadSync = useStore((s) => s.loadSync);

  const [code, setCode] = useState("");
  const [name, setName] = useState("");
  const [understood, setUnderstood] = useState(false);

  const kind = pairing?.kind;
  const phase = pairing?.phase;

  useEffect(() => {
    if (!pairing) {
      setCode("");
      setName("");
      setUnderstood(false);
    }
  }, [pairing]);

  if (!pairing || !kind) return null;

  const joining = kind === "contact_join" || kind === "device_join";
  const isDevice = kind === "device_host" || kind === "device_join";
  // A joining sheet parks in "starting" until the user submits the code: the
  // session is only opened at that point, so there is nothing to wait for yet.
  const notYetStarted = joining && !pairing.session && phase === "starting";

  const close = () => {
    if (phase === "done" || phase === "failed") clear();
    else void cancel();
  };

  const finish = async () => {
    if (pairing.needsRestart) {
      await restartDaemon();
      toast.info(t("pair.restarting"), t("pair.restartingDetail"));
    }
    clear();
    void loadSync();
  };

  return (
    <Sheet
      open
      onClose={close}
      placement="center"
      title={t(TITLE[kind])}
      subtitle={t(SUB[kind])}
      footer={
        phase === "done" ? (
          <>
            <div className="spacer" />
            <Button variant="primary" onClick={finish} data-autofocus>
              {pairing.needsRestart
                ? t("pair.restartAndClose")
                : t("common.done")}
            </Button>
          </>
        ) : phase === "failed" ? (
          <>
            <div className="spacer" />
            <Button onClick={clear}>{t("common.close")}</Button>
            <Button
              variant="primary"
              onClick={() => void start(kind, joining ? code : undefined, name || undefined)}
            >
              {t("common.retry")}
            </Button>
          </>
        ) : notYetStarted ? (
          <>
            <div className="spacer" />
            <Button onClick={close}>{t("common.cancel")}</Button>
            <Button
              variant="primary"
              disabled={!code.trim() || (kind === "device_join" && !understood)}
              onClick={() => void start(kind, code.trim(), name.trim() || undefined)}
            >
              {t("pair.link")}
            </Button>
          </>
        ) : (
          <>
            <div className="spacer" />
            <Button onClick={close}>{t("common.cancel")}</Button>
          </>
        )
      }
    >
      {/* ---- outcome ------------------------------------------------- */}
      {phase === "done" && (
        <div className="stack">
          <div className="hstack">
            <span className="tone-ok">
              <Icon.Check size={22} />
            </span>
            <span className="t-head">{t("pair.done")}</span>
          </div>
          <div className="t-sm t-sec">{pairing.message}</div>
          {pairing.needsRestart && (
            <div
              className="card card-pad t-sm"
              style={{ borderColor: "var(--amber)" }}
            >
              {t("pair.needsRestart")}
            </div>
          )}
        </div>
      )}

      {phase === "failed" && (
        <div className="stack">
          <div className="hstack">
            <span className="tone-bad">
              <Icon.Alert size={20} />
            </span>
            <span className="t-head">{t("pair.failed")}</span>
          </div>
          <div className="t-sm t-sec" style={{ wordBreak: "break-word" }}>
            {pairing.message}
          </div>
        </div>
      )}

      {/* ---- hosting: show the code ---------------------------------- */}
      {!joining && phase !== "done" && phase !== "failed" && (
        <div className="stack">
          {isDevice && (
            <div
              className="card card-pad t-sm"
              style={{ borderColor: "var(--amber)" }}
            >
              <strong>{t("pair.deviceWarnLead")}</strong>{" "}
              {t("pair.deviceWarnRest")}
            </div>
          )}

          {pairing.code ? (
            <>
              <CodeHero
                value={pairing.code}
                caption={
                  isDevice
                    ? t("pair.captionDevice")
                    : t("pair.captionContact")
                }
              />
              <div className="hstack-sm">
                <span className="spinner" />
                <span className="t-sm t-mut">{t("pair.waitingOther")}</span>
              </div>
            </>
          ) : (
            <div className="hstack">
              <span className="spinner" />
              <span className="t-sm t-mut">{t("pair.preparingCode")}</span>
            </div>
          )}

          {!isDevice && <div className="hint">{t("pair.contactNote")}</div>}
        </div>
      )}

      {/* ---- joining --------------------------------------------------- */}
      {joining && phase !== "done" && phase !== "failed" && (
        <div className="stack">
          {kind === "device_join" && (
            <div
              className="card card-pad t-sm"
              style={{ borderColor: "var(--red)" }}
            >
              <strong>{t("pair.joinWarnLead")}</strong>{" "}
              {t("pair.joinWarnRest")}
            </div>
          )}

          {notYetStarted ? (
            <>
              <Field
                label={t("pair.codeLabel")}
                hint={t("pair.codeHint")}
              >
                {({ id, describedBy }) => (
                  <TextInput
                    id={id}
                    data-autofocus
                    aria-describedby={describedBy}
                    className="mono"
                    big
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    value={code}
                    onChange={(e) => setCode(e.currentTarget.value)}
                    placeholder="4821-crater-mango"
                  />
                )}
              </Field>

              {kind === "contact_join" && (
                <Field
                  label={t("pair.nameLabel")}
                  hint={t("pair.nameHint")}
                >
                  {({ id, describedBy }) => (
                    <TextInput
                      id={id}
                      aria-describedby={describedBy}
                      value={name}
                      onChange={(e) => setName(e.currentTarget.value)}
                      placeholder={t("people.addNamePlaceholder")}
                    />
                  )}
                </Field>
              )}

              {kind === "device_join" && (
                <label className="hstack" style={{ cursor: "pointer" }}>
                  <input
                    type="checkbox"
                    checked={understood}
                    onChange={(e) => setUnderstood(e.currentTarget.checked)}
                  />
                  <span className="t-sm">{t("pair.understood")}</span>
                </label>
              )}
            </>
          ) : (
            <div className="hstack">
              <span className="spinner" />
              <span className="t-sm t-mut">{t("pair.waitingMachine")}</span>
            </div>
          )}
        </div>
      )}
    </Sheet>
  );
}
