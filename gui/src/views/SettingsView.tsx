// Settings, and the identity they hang off.
//
// The rule this screen follows everywhere: show what is **in force**, and say
// **where it comes from**. A relay supplied by `ARVOLO_RELAY` or compiled into
// the binary is not a saved setting, and presenting it in an editable field
// would invite the user to change something the environment will keep
// overriding. So each such field carries its provenance, and the field is
// disabled when editing it could not win.
//
// What is deliberately *not* here: temp dir, iroh relay, log level, identity
// path. Those are text-file settings whose wrong value is hard to recover from
// through a dialog. The screen links to `config.toml` instead of half-exposing
// them.

import { useEffect, useMemo, useState } from "react";
import {
  disable as autostartDisable,
  enable as autostartEnable,
  isEnabled as autostartIsEnabled,
} from "@tauri-apps/plugin-autostart";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { api } from "../ipc";
import { fire, TITLE_KEY, useStore, type ThemeChoice } from "../store";
import { useIsPhone } from "../phone";
import { isBusy } from "../update";
import {
  langNames,
  setLangChoice,
  useLangChoice,
  useT,
  type LangChoice,
} from "../i18n";
import { Icon } from "../ui/Icons";
import {
  Badge,
  Button,
  Empty,
  Field,
  Segmented,
  SwitchRow,
  TextInput,
} from "../ui/Primitives";
import { CopyField, Fingerprint } from "../ui/Bits";
import { Confirm } from "../ui/Sheet";
import { toast } from "../ui/Toasts";

const SOURCE_KEY = {
  env: "settings.sourceEnv",
  config: "settings.sourceConfig",
  builtin: "settings.sourceBuiltin",
  none: "settings.sourceNone",
} as const;

export function SettingsView() {
  const isPhone = useIsPhone();
  const go = useStore((s) => s.go);
  const t = useT();
  const lang = useLangChoice();
  const config = useStore((s) => s.config);
  const loading = useStore((s) => s.configLoading);
  const error = useStore((s) => s.configError);
  const reload = useStore((s) => s.loadConfig);
  const save = useStore((s) => s.saveConfig);
  const status = useStore((s) => s.status);
  const guiVersion = useStore((s) => s.guiVersion);
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const restartDaemon = useStore((s) => s.restartDaemon);
  const update = useStore((s) => s.update);
  const checkForUpdate = useStore((s) => s.checkForUpdate);
  const setAutoCheck = useStore((s) => s.setAutoCheck);
  const startUpdate = useStore((s) => s.startUpdate);
  const transfers = useStore((s) => s.transfers);
  const busyBoard = useMemo(
    () => isBusy(Object.values(transfers).map((tx) => tx.status)),
    [transfers]
  );

  // One line that says where the update flow stands. "Up to date" is only ever
  // claimed after a check that actually completed — before that it says when the
  // last one was, which is the honest answer and also the useful one.
  const updateLine =
    update.phase === "checking"
      ? t("settings.updateChecking")
      : update.phase === "installed"
        ? t("settings.updateReopen")
        : update.phase === "error"
          ? t("settings.updateFailed", update.error ?? "")
          : update.phase === "downloading"
            ? t("settings.updateInstalling")
            : update.info
              ? t("settings.updateFound", update.info.version)
              : update.manual
                ? t("settings.updateNone")
                : t("settings.updateIdle");

  const [name, setName] = useState("");
  const [relay, setRelay] = useState("");
  const [dirty, setDirty] = useState<{ name?: boolean; relay?: boolean }>({});
  const [busy, setBusy] = useState(false);
  const [confirmRestart, setConfirmRestart] = useState(false);
  const [confirmUpdate, setConfirmUpdate] = useState(false);

  // Asked of the OS, not of our config: the login entry lives in the platform's
  // own registry (Login Items, the Run key, ~/.config/autostart), so the OS is
  // the only honest answer. `null` = the answer has not arrived (or the
  // platform refused) — the switch stays disabled rather than guessing.
  const [autostart, setAutostart] = useState<boolean | null>(null);
  useEffect(() => {
    autostartIsEnabled().then(setAutostart).catch(() => setAutostart(null));
  }, []);

  // Reset the drafts whenever a fresh config lands, but never clobber a field
  // the user is mid-edit in: a background refresh must not eat what they typed.
  useEffect(() => {
    if (!config) return;
    setName((n) => (dirty.name ? n : config.display_name));
    setRelay((r) => (dirty.relay ? r : config.relay_configured));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config]);

  if (error && !config) {
    return (
      <div className="card card-pad card-err t-sm">
        {error}
        <div style={{ marginTop: 10 }}>
          <Button size="sm" onClick={() => void reload()}>
            {t("common.retry")}
          </Button>
        </div>
      </div>
    );
  }
  if (!config) {
    return (
      <div className="card">
        <Empty icon={<Icon.Settings size={22} />} title={t("common.loading")} />
      </div>
    );
  }

  const relayLocked = config.relay_source === "env";

  // Both of these hang off an `onClick`, which does not await what it calls, and
  // `saveConfig` rethrows after filing the failure in `actionError`. Left as
  // async handlers the refusal escaped as an unhandled rejection — reported
  // nowhere the user can see, and blamed on whatever ran next. Same reasoning as
  // the confirm callbacks further down.
  const saveName = () => {
    setBusy(true);
    fire(
      save({ display_name: name.trim() ? { set: name.trim() } : "clear" })
        .then(() => {
          // Only on success: a field still marked clean after a refused write
          // would disable Save and strand the edit.
          setDirty((d) => ({ ...d, name: false }));
          toast.ok(t("settings.nameSaved"), t("settings.nameSavedDetail"));
        })
        .finally(() => setBusy(false))
    );
  };

  const saveRelay = () => {
    setBusy(true);
    fire(
      save({ relay: relay.trim() ? { set: relay.trim() } : "clear" })
        .then(() => {
          setDirty((d) => ({ ...d, relay: false }));
          toast.info(t("settings.relaySaved"), t("settings.relaySavedDetail"));
        })
        .finally(() => setBusy(false))
    );
  };

  return (
    <div className="view-narrow stack">
      {/* ---- identity ------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">{t("settings.whoYouAre")}</div>

        <Field
          label={t("settings.nameLabel")}
          hint={t("settings.nameHint")}
        >
          {({ id, describedBy }) => (
            <div className="hstack-sm">
              <TextInput
                id={id}
                aria-describedby={describedBy}
                value={name}
                maxLength={64}
                onChange={(e) => {
                  setName(e.currentTarget.value);
                  setDirty((d) => ({ ...d, name: true }));
                }}
                placeholder={t("settings.namePlaceholder")}
              />
              <Button
                size="sm"
                disabled={!dirty.name || busy}
                busy={busy && dirty.name}
                onClick={saveName}
              >
                {t("common.save")}
              </Button>
            </div>
          )}
        </Field>

        <Field
          label={t("settings.fingerprintLabel")}
          hint={t("settings.fingerprintHint")}
        >
          {() => (
            <div
              className="card card-pad"
              style={{ background: "var(--surface-2)" }}
            >
              <Fingerprint value={status?.fingerprint ?? ""} />
            </div>
          )}
        </Field>

        {/* The form to hand somebody — id *and* relay. The id on its own says who
            you are and not where you listen, so a friend given only that reaches
            you if their client happens to default to your relay, and silently not
            otherwise. Shown as one field rather than two: nobody needs the half
            that cannot be acted on, and it is readable in front of the `@` anyway.
            The bare id remains for a daemon too old to assemble the pair. */}
        <Field label={t("settings.publicIdLabel")}>
          {() => (
            <CopyField
              value={status?.shareable_id || status?.public_id || ""}
              wrap
            />
          )}
        </Field>
      </div>

      {/* ---- what arrives on its own ----------------------------------- */}
      {/* A phone is why this is a setting. On a desktop the window is usually
          open and a question costs a glance; here the same question is a
          notification, a walk back to the app and a tap before anything moves —
          and the opposite want is just as real, because a device you carry is a
          device somebody else can be holding.

          Three answers, so a segmented control rather than a switch: "trusted
          only" is a real position and not the halfway point between the other
          two, and a two-state control would have had to drop one of them. */}
      {isPhone && (
        <div className="card card-pad stack">
          <div className="t-head">{t("settings.autoAcceptTitle")}</div>
          <Segmented
            block
            label={t("settings.autoAcceptTitle")}
            value={config.auto_accept || "trusted"}
            onChange={(v) => fire(save({ auto_accept: { set: v } }))}
            options={[
              { value: "never", label: t("settings.autoAcceptNever") },
              { value: "trusted", label: t("settings.autoAcceptTrusted") },
              { value: "everyone", label: t("settings.autoAcceptEveryone") },
            ]}
          />
          <div className="t-xs t-mut">
            {t(
              config.auto_accept === "never"
                ? "settings.autoAcceptHintNever"
                : config.auto_accept === "everyone"
                  ? "settings.autoAcceptHintEveryone"
                  : "settings.autoAcceptHintTrusted"
            )}
          </div>
        </div>
      )}

      {/* ---- the places the bottom bar has no room for ----------------- */}
      {/* Five tabs is what a thumb can hit; six places and two verbs is what
          the app has. The three that did not fit are here rather than gone —
          the phone reaches everything the desktop does, by one more tap. */}
      {isPhone && (
        <div className="card">
          <div className="card-pad t-head">{t("settings.morePlaces")}</div>
          {(["deposits", "history", "devices"] as const).map((r) => (
            <button key={r} className="place-link" onClick={() => go(r)}>
              <span className="grow truncate">{t(TITLE_KEY[r])}</span>
              <Icon.ChevronRight size={15} />
            </button>
          ))}
        </div>
      )}

      {/* ---- appearance ----------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">{t("settings.appearance")}</div>
        <Field label={t("settings.theme")}>
          {() => (
            <Segmented
              block
              label={t("settings.theme")}
              value={theme}
              onChange={(v) => setTheme(v as ThemeChoice)}
              options={[
                { value: "system", label: t("settings.themeSystem") },
                { value: "light", label: t("settings.themeLight") },
                { value: "dark", label: t("settings.themeDark") },
              ]}
            />
          )}
        </Field>
        {/* The language names are endonyms and stay put whatever the current
            language is: somebody hunting for German is looking for "Deutsch".
            Only "System" is translated, because it is a sentence about the
            machine rather than the name of a language. */}
        <Field label={t("settings.language")} hint={t("settings.languageHint")}>
          {() => (
            <Segmented
              block
              label={t("settings.language")}
              value={lang}
              onChange={(v) => setLangChoice(v as LangChoice)}
              options={[
                { value: "system", label: t("settings.languageAuto") },
                ...langNames(),
              ]}
            />
          )}
        </Field>
      </div>

      {/* ---- startup --------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">{t("settings.startup")}</div>
        <SwitchRow
          title={t("settings.autostartTitle")}
          desc={t("settings.autostartDesc")}
          checked={autostart ?? false}
          disabled={autostart === null}
          onChange={(v) => {
            // Optimistic, then verified: the OS call can refuse (no autostart
            // dir, a sandboxed install), and on refusal the switch snaps back
            // and the reason lands in a toast instead of nowhere.
            setAutostart(v);
            void (v ? autostartEnable() : autostartDisable()).catch(
              (e: unknown) => {
                setAutostart(!v);
                toast.bad(t("settings.autostartFailed"), String(e));
              }
            );
          }}
        />
      </div>

      {/* ---- rete ----------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="hstack">
          <div className="grow t-head">{t("settings.network")}</div>
          <Badge kind={config.relay ? "ok" : "warn"}>
            <Icon.Relay size={10} />
            {config.relay ? t("settings.relayOn") : t("settings.relayOff")}
          </Badge>
        </div>

        <Field
          label={t("settings.relayLabel")}
          hint={
            relayLocked
              ? t("settings.relayLocked")
              : t(
                  "settings.relayHint",
                  config.relay ?? t("settings.relayNone"),
                  // An unrecognised provenance is shown raw rather than dropped:
                  // a newer daemon may well name one this build has not learnt.
                  config.relay_source in SOURCE_KEY
                    ? t(SOURCE_KEY[config.relay_source as keyof typeof SOURCE_KEY])
                    : config.relay_source
                )
          }
        >
          {({ id, describedBy }) => (
            <div className="hstack-sm">
              <TextInput
                id={id}
                aria-describedby={describedBy}
                className="mono"
                spellCheck={false}
                disabled={relayLocked}
                value={relay}
                onChange={(e) => {
                  setRelay(e.currentTarget.value);
                  setDirty((d) => ({ ...d, relay: true }));
                }}
                placeholder={config.relay ?? t("settings.relayPlaceholder")}
              />
              <Button
                size="sm"
                disabled={!dirty.relay || relayLocked || busy}
                onClick={saveRelay}
              >
                {t("common.save")}
              </Button>
            </div>
          )}
        </Field>

        <div className="hint">{t("settings.relayNote")}</div>
      </div>

      {/* ---- files ---------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">{t("settings.files")}</div>

        <Field
          label={t("settings.downloadDirLabel")}
          hint={
            config.download_dir_from_env
              ? t("settings.downloadDirEnv")
              : t("settings.downloadDirHint")
          }
        >
          {({ id, describedBy }) => (
            <div className="hstack-sm">
              <TextInput
                id={id}
                aria-describedby={describedBy}
                readOnly
                value={config.download_dir}
              />
              <Button
                size="sm"
                disabled={config.download_dir_from_env || busy}
                onClick={async () => {
                  const picked = await openDialog({ directory: true });
                  if (typeof picked !== "string") return;
                  await save({ download_dir: { set: picked } });
                  toast.info(
                    t("settings.dirUpdated"),
                    t("common.afterRestart")
                  );
                }}
              >
                <Icon.Folder size={13} /> {t("settings.change")}
              </Button>
              <Button
                size="sm"
                onClick={() =>
                  api.openPath(config.download_dir).catch((e: unknown) =>
                    toast.bad(t("settings.cannotOpen"), String(e))
                  )
                }
              >
                {t("common.open")}
              </Button>
            </div>
          )}
        </Field>

        <div className="divider" />

        <SwitchRow
          title={t("settings.seedTitle")}
          desc={t("settings.seedDesc")}
          checked={config.seed ?? true}
          onChange={async (v) => {
            await save({ seed: { set: v } });
            toast.info(t("settings.saved"), t("common.afterRestart"));
          }}
        />
      </div>

      {/* ---- advanced ------------------------------------------------- */}
      <div className="card card-pad stack">
        <div className="t-head">{t("settings.advanced")}</div>

        <Field
          label={t("settings.configFileLabel")}
          hint={t("settings.configFileHint")}
        >
          {() => (
            <div className="hstack-sm">
              <TextInput readOnly value={config.config_path} className="mono" />
              <Button
                size="sm"
                onClick={() =>
                  api.openPath(config.config_path).catch((e: unknown) =>
                    toast.bad(t("settings.cannotOpen"), String(e))
                  )
                }
              >
                <Icon.External size={13} /> {t("common.open")}
              </Button>
            </div>
          )}
        </Field>

        <Field
          label={t("settings.identityKeyLabel")}
          hint={t("settings.identityKeyHint")}
        >
          {() => (
            <TextInput readOnly value={config.identity_path} className="mono" />
          )}
        </Field>

        <div className="divider" />

        <div className="hstack wrap">
          <div className="grow">
            <div className="t-sm t-sec">
              {t("settings.versions", status?.version || "?", guiVersion || "?")}
            </div>
            {/* Which daemon this window is actually talking to. A released
                install, a build from a checkout and one this app spawned can all
                exist on the same machine, and everything else on this screen
                describes the identity — which they share. It sits here, directly
                above the button that restarts it, because that is the moment the
                question gets asked. */}
            {!!status?.pid && (
              <div
                className="t-xs t-mut mono truncate"
                title={status.exe || undefined}
              >
                {t("settings.daemonProcess", status.pid, status.exe || "?")}
              </div>
            )}
          </div>
          <Button size="sm" onClick={() => setConfirmRestart(true)}>
            <Icon.Refresh size={13} /> {t("settings.restartDaemon")}
          </Button>
        </div>

        <div className="divider" />

        {/* Updates sit directly under the versions, because that is the moment
            the question gets asked. The button ignores the weekly cadence and
            always answers — including when the answer is "nothing new", which
            the automatic check never says out loud. */}
        <div className="hstack wrap">
          <div className="grow">
            <div style={{ fontWeight: 570 }}>{t("settings.updatesTitle")}</div>
            <div className="t-xs t-mut">{updateLine}</div>
          </div>
          {update.info ? (
            // The banner is hidden while a transfer runs, so without this the
            // only way to reach the install would be to wait for the board to go
            // quiet — with no hint that that was what was needed.
            <Button
              size="sm"
              variant="in"
              disabled={update.phase === "downloading"}
              onClick={() => (busyBoard ? setConfirmUpdate(true) : startUpdate())}
            >
              {t("app.updateNow")}
            </Button>
          ) : (
            <Button
              size="sm"
              disabled={update.phase === "checking"}
              onClick={() => fire(checkForUpdate(true))}
            >
              <Icon.Refresh size={13} /> {t("settings.checkUpdates")}
            </Button>
          )}
        </div>

        <SwitchRow
          title={t("settings.autoUpdateTitle")}
          desc={t("settings.autoUpdateDesc")}
          checked={update.autoCheck}
          onChange={(v) => setAutoCheck(v)}
        />
      </div>

      {/* Nothing is lost either way — the daemon writes its resumable records as
          it goes, not at shutdown — but interrupting a running transfer is not
          something to do to someone without asking. */}
      <Confirm
        open={confirmUpdate}
        title={t("settings.confirmUpdateTitle")}
        body={t("settings.confirmUpdateBody")}
        confirmLabel={t("app.updateNow")}
        onCancel={() => setConfirmUpdate(false)}
        onConfirm={() => {
          setConfirmUpdate(false);
          startUpdate();
        }}
      />

      <Confirm
        open={confirmRestart}
        title={t("settings.confirmRestartTitle")}
        body={t("settings.confirmRestartBody")}
        confirmLabel={t("app.restart")}
        onCancel={() => setConfirmRestart(false)}
        onConfirm={() => {
          setConfirmRestart(false);
          // Only announce the restart if it was actually accepted; the failure
          // is already reported through `actionError`, and nothing awaits this.
          fire(
            restartDaemon().then(() =>
              toast.info(t("settings.restarting"), t("settings.restartingDetail"))
            )
          );
        }}
      />

      {loading && <div className="t-xs t-mut">{t("settings.refreshing")}</div>}
    </div>
  );
}
