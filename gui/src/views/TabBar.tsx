// The rail, lying down.
//
// Six places and two verbs do not fit across the bottom of a phone, and a bar
// nobody can hit accurately is worse than one place fewer. So the bar carries
// the five things a person reaches for while standing up — the two verbs, the
// two lists they produce, and yourself — and the rest keeps its parity by moving
// under *You*, which is where a phone puts everything that is not a verb.
//
// Send is the centre and looks like it. It is what the app is for, it is the one
// control that is not navigation, and on a phone the middle of the bottom edge is
// the easiest place on the screen to reach with a thumb.

import { phoneTitle, useStore, type Route } from "../store";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";

export function TabBar() {
  const t = useT();
  const route = useStore((s) => s.route);
  const go = useStore((s) => s.go);
  const openSheet = useStore((s) => s.openSheet);
  const transfers = useStore((s) => s.transfers);

  const rows = Object.values(transfers);
  const pending = rows.filter((x) => x.status === "incoming").length;
  const active = rows.filter(
    (x) => x.status === "active" || x.status === "stalled"
  ).length;
  const count = pending || active;

  const place = (route_: Route, icon: JSX.Element, badge?: number) => {
    // One name for one place: the header says it too.
    const label = t(phoneTitle(route_));
    return (
      <button
        className="tab"
        aria-current={route === route_ ? "page" : undefined}
        onClick={() => go(route_)}
        title={label}
      >
        <span className="ico">
          {icon}
          {badge ? (
            <span className={`tab-count tnum ${pending ? "hot" : ""}`}>
              {badge}
            </span>
          ) : null}
        </span>
        <span className="truncate">{label}</span>
      </button>
    );
  };

  return (
    <nav className="tabbar" aria-label={t("rail.nav")}>
      {place("transfers", <Icon.Transfers />, count)}
      {place("people", <Icon.People />)}
      <button
        className="tab tab-send"
        onClick={() => openSheet([])}
        title={t("tab.send")}
      >
        <span className="ico">
          <Icon.Send />
        </span>
        <span className="truncate">{t("tab.send")}</span>
      </button>
      {place("codes", <Icon.Qr />)}
      {place("settings", <Icon.Settings />)}
    </nav>
  );
}
