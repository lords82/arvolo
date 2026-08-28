// Who you are, and the few things that follow from it.
//
// The desktop's settings window is a window: a dozen controls, grouped, all
// visible because there is room. The design of record does not put that on a
// phone. It opens with the identity — the thing this whole app is about, and the
// thing somebody on the other end checks — then the one decision a phone
// actually changes (what arrives without asking), then the places the bottom bar
// had no room for, then a line saying which build this is.
//
// Everything else the desktop offers is still here, one row down, under "all
// settings". A phone that could not reach a setting would not be the same app.

import { useT } from "../i18n";
import { fire, useStore } from "../store";
import { Avatar, ShortId } from "../ui/Bits";
import { Icon } from "../ui/Icons";
import { Segmented } from "../ui/Primitives";

/** One row that leads somewhere, with what is behind it said on the right. */
function Place({
  label,
  note,
  onClick,
}: {
  label: string;
  note?: string;
  onClick: () => void;
}) {
  return (
    <button className="place-link" onClick={onClick}>
      <span className="grow truncate">{label}</span>
      {note ? <span className="t-sm t-mut">{note}</span> : null}
      <Icon.ChevronRight size={15} />
    </button>
  );
}

export function YouView() {
  const t = useT();
  const go = useStore((s) => s.go);
  const status = useStore((s) => s.status);
  const config = useStore((s) => s.config);
  const save = useStore((s) => s.saveConfig);
  const devices = useStore((s) => s.devices);
  const deposits = useStore((s) => s.deposits);
  const guiVersion = useStore((s) => s.guiVersion);

  const name = status?.display_name || t("you.unnamed");
  const policy = config?.auto_accept || "trusted";
  const live = deposits.filter((d) => !d.expired).length;

  return (
    <div className="you">
      {/* The identity, first and whole. It is what the person on the other end
          checks, and burying it three taps into a settings screen would be
          saying it is a preference. */}
      <button className="card card-pad you-me" onClick={() => go("advanced")}>
        <Avatar name={name} id={status?.public_id} size={44} />
        <span className="grow" style={{ minWidth: 0 }}>
          <span className="truncate you-name">{name}</span>
          {status?.public_id ? <ShortId value={status.public_id} /> : null}
        </span>
        <Icon.ChevronRight size={16} />
      </button>

      {/* The one decision that is different on a phone — see `AutoAccept` in the
          engine, where the same three words become the rule. */}
      <div className="card card-pad stack">
        <div className="t-head">{t("settings.autoAcceptTitle")}</div>
        <Segmented
          block
          label={t("settings.autoAcceptTitle")}
          value={policy}
          onChange={(v) => fire(save({ auto_accept: { set: v } }))}
          options={[
            { value: "never", label: t("settings.autoAcceptNever") },
            { value: "trusted", label: t("settings.autoAcceptTrusted") },
            { value: "everyone", label: t("settings.autoAcceptEveryone") },
          ]}
        />
        <div className="t-xs t-mut">
          {t(
            policy === "never"
              ? "settings.autoAcceptHintNever"
              : policy === "everyone"
                ? "settings.autoAcceptHintEveryone"
                : "settings.autoAcceptHintTrusted"
          )}
        </div>
      </div>

      <div className="card">
        <Place
          label={t("title.devices")}
          note={devices.length ? String(devices.length) : undefined}
          onClick={() => go("devices")}
        />
        <Place
          label={t("title.deposits")}
          note={live ? t("you.liveCount", live) : undefined}
          onClick={() => go("deposits")}
        />
        <Place label={t("title.history")} onClick={() => go("history")} />
        <Place
          label={t("you.allSettings")}
          note={status?.relay ? t("you.relaySet") : undefined}
          onClick={() => go("advanced")}
        />
      </div>

      {/* Not "in the keychain": the identity is a file in this app's private
          storage. It will be the keychain when something puts it there, and a
          line that said so first would be the app claiming a guarantee it does
          not have. */}
      <div className="t-xs t-mut you-foot">
        {t("you.footer", guiVersion || "…")}
      </div>
    </div>
  );
}
