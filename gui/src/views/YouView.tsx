// Who you are, and the few things that follow from it.
//
// The desktop's settings window is a window: a dozen controls, grouped, all
// visible because there is room. The design of record does not put that on a
// phone. It opens with the identity — the thing this whole app is about, and the
// thing somebody on the other end checks — then the one decision a phone
// actually changes (what arrives without asking), then the places the bottom bar
// had no room for, then a line saying which build this is.
//
// Everything else the desktop offers is one row in, behind the identity at the
// top: that row *is* the way to the settings, and it opens on the name and the
// id it was tapped for. There used to be a second row saying the same thing at
// the bottom — two doors onto one screen, which is a menu describing itself
// rather than a screen you can find your way around.

import { useT } from "../i18n";
import { useStore } from "../store";
import { Avatar, ShortId } from "../ui/Bits";
import { Icon } from "../ui/Icons";
import { Place } from "../ui/Rows";

export function YouView() {
  const t = useT();
  const push = useStore((s) => s.push);
  const status = useStore((s) => s.status);
  const config = useStore((s) => s.config);
  const devices = useStore((s) => s.devices);
  const deposits = useStore((s) => s.deposits);
  const guiVersion = useStore((s) => s.guiVersion);

  const name = status?.display_name || t("you.unnamed");
  // What the engine falls back to here, said again on this side: a row that
  // showed "Fidati" while a phone was in fact asking would be the screen
  // disagreeing with the app. See `AutoAccept::default_here`.
  const policy = config?.auto_accept || "never";
  const live = deposits.filter((d) => !d.expired).length;

  return (
    <div className="ios-page you">
      {/* The identity, first and whole. It is what the person on the other end
          checks, and burying it three taps into a settings screen would be
          saying it is a preference. */}
      <div className="ios-group">
        <button className="you-me" onClick={() => push("advanced")}>
          <Avatar name={name} id={status?.public_id} size={44} />
          <span className="grow" style={{ minWidth: 0 }}>
            <span className="truncate you-name">{name}</span>
            {status?.public_id ? <ShortId value={status.public_id} /> : null}
          </span>
          <Icon.ChevronRight size={16} />
        </button>
        {/* Directly under the identity, and not filed under "elsewhere", because
            it is not somewhere else: your other machines are the same person as
            the name above them — one id, one mailbox — and the row that adds one
            hands over this device's identity secret. It belongs to the identity
            it changes. */}
        <Place
          label={t("title.devices")}
          note={devices.length ? String(devices.length) : undefined}
          onClick={() => push("devices")}
        />
      </div>

      {/* The one decision that is different on a phone. A row saying what is in
          force, pushing to where the three answers are — see `AutoAccept` in the
          engine, where the same three words become the rule. */}
      <div className="ios-head">{t("you.receiving")}</div>
      <div className="ios-group">
        <Place
          label={t("settings.autoAcceptTitle")}
          note={t(
            policy === "never"
              ? "settings.autoAcceptNever"
              : policy === "everyone"
                ? "settings.autoAcceptEveryone"
                : "settings.autoAcceptTrusted"
          )}
          onClick={() => push("autoAccept")}
        />
      </div>

      <div className="ios-head">{t("you.elsewhere")}</div>
      <div className="ios-group">
        <Place
          label={t("title.deposits")}
          note={live ? t("you.liveCount", live) : undefined}
          onClick={() => push("deposits")}
        />
        <Place label={t("title.history")} onClick={() => push("history")} />
      </div>

      {/* Not "in the keychain": the identity is a file in this app's private
          storage. It will be the keychain when something puts it there, and a
          line that said so first would be the app claiming a guarantee it does
          not have. */}
      <div className="ios-foot you-foot">
        {t("you.footer", guiVersion || "…")}
      </div>
    </div>
  );
}
