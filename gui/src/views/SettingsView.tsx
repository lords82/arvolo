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

import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { fire, useStore, type ThemeChoice } from "../store";
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

  const [name, setName] = useState("");
  const [relay, setRelay] = useState("");
  const [dirty, setDirty] = useState<{ name?: boolean; relay?: boolean }>({});
  const [busy, setBusy] = useState(false);
  const [confirmRestart, setConfirmRestart] = useState(false);

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
      <div className="card card-pad t-sm" style={{ borderColor: "var(--red)" }}>
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

        <Field label={t("settings.publicIdLabel")}>
          {() => <CopyField value={status?.public_id ?? ""} wrap />}
        </Field>
      </div>

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
                    t("settings.dirUpdatedDetail")
                  );
                }}
              >
                <Icon.Folder size={13} /> {t("settings.change")}
              </Button>
              <Button
                size="sm"
                onClick={() =>
                  openPath(config.download_dir).catch((e: unknown) =>
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
            toast.info(t("settings.saved"), t("settings.savedDetail"));
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
                  openPath(config.config_path).catch((e: unknown) =>
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
          <span className="t-sm t-sec grow">
            {t("settings.versions", status?.version || "?", guiVersion || "?")}
          </span>
          <Button size="sm" onClick={() => setConfirmRestart(true)}>
            <Icon.Refresh size={13} /> {t("settings.restartDaemon")}
          </Button>
        </div>
      </div>

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
