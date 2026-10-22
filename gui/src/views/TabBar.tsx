// The rail, lying down: three places, and the one in the middle is where
// everything starts.
//
// This bar has been through five shapes, and the reason for this one is worth
// keeping rather than the rule it breaks. It held four places and no verb, on the
// argument that a tab bar on this platform is navigation and a filled button in
// the middle of one is somebody else's convention. Then it held *Invia* in the
// middle as a disc. Then five places, with *Scambia* in the middle as a camera
// and *Invia* beside it as a fourth — which asked "a chi" twice, once in each.
// Then three places on a bar that was the floor continuing.
//
// The design of record ("Arvolo Mobile 2 — Scambia", sections 14–15) makes it
// an island: a white capsule floating over the screen, with **Scambia** in the
// middle as a navy pill that always carries its name — it is the place everything
// starts from, and the bar says so wherever you are. Attività and Persone are a
// glyph and a word; a blue dot over Attività says something is moving, or
// somebody is waiting for an answer.

import { phoneTitle, useStore, type Route } from "../store";
import { useT } from "../i18n";
import { Icon } from "../ui/Icons";
import { haptic } from "../haptics";
import type { UIStatus } from "../types";

/** The states the dot stands for: moving, held, or waiting on a decision. */
const BUSY: UIStatus[] = ["active", "preparing", "offered", "stalled", "incoming"];

export function TabBar() {
  const t = useT();
  const route = useStore((s) => s.route);
  const go = useStore((s) => s.go);
  const transfers = useStore((s) => s.transfers);

  const busy = Object.values(transfers).some((x) => BUSY.includes(x.status));

  const place = (
    route_: Route,
    icon: JSX.Element,
    opts: { dot?: boolean; disc?: boolean } = {}
  ) => {
    // One name for one place: the header says it too.
    const label = t(phoneTitle(route_));
    return (
      <button
        className={`tab${opts.disc ? " tab-disc" : ""}`}
        aria-current={route === route_ ? "page" : undefined}
        onClick={() => {
          // The system's tick on a change of place, and nothing on the tab you
          // are already on: that tap does nothing, and should feel like it.
          if (route !== route_) haptic.picked();
          go(route_);
        }}
        title={label}
      >
        {opts.disc ? (
          <span className="verb-disc">
            {icon}
            <span>{label}</span>
          </span>
        ) : (
          <>
            <span className="ico">{icon}</span>
            <span className="truncate">{label}</span>
            {opts.dot ? <span className="tab-dot" aria-hidden /> : null}
          </>
        )}
      </button>
    );
  };

  return (
    <nav className="tabbar" aria-label={t("rail.nav")}>
      {place("transfers", <Icon.Activity size={22} />, { dot: busy })}
      {place("exchange", <Icon.Scan size={20} />, { disc: true })}
      {place("people", <Icon.People size={22} />)}
    </nav>
  );
}
