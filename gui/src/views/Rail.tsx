// The app's spine.
//
// Three tiers, in the order a person needs them: who you are, what you do, and
// where things are. The two verbs sit above the places because they are what the
// app is *for* — everything below them is a record of having done one of them.
//
// The identity chip at the top is not decoration. This app's whole proposition
// is that the person on the other end can be sure it is you, and the fingerprint
// under your name is the thing they check. Putting it in the frame rather than
// three clicks into a settings screen is the point.

import { useStore, type Route } from "../store";
import { Icon } from "../ui/Icons";
import { Avatar } from "../ui/Bits";
import { Kbd, modKey } from "../ui/Primitives";

interface Place {
  route: Route;
  label: string;
  icon: JSX.Element;
}

const PLACES: Place[] = [
  { route: "transfers", label: "Trasferimenti", icon: <Icon.Transfers /> },
  { route: "people", label: "Persone", icon: <Icon.People /> },
  { route: "deposits", label: "Link e depositi", icon: <Icon.Link /> },
  { route: "history", label: "Cronologia", icon: <Icon.History /> },
  { route: "devices", label: "I tuoi dispositivi", icon: <Icon.Devices /> },
  { route: "settings", label: "Impostazioni", icon: <Icon.Settings /> },
];

export function Rail() {
  const route = useStore((s) => s.route);
  const go = useStore((s) => s.go);
  const status = useStore((s) => s.status);
  const connected = useStore((s) => s.connected);
  const openSheet = useStore((s) => s.openSheet);
  const openReceive = useStore((s) => s.openReceive);
  const transfers = useStore((s) => s.transfers);
  const contacts = useStore((s) => s.contacts);
  const setPaletteOpen = useStore((s) => s.setPaletteOpen);

  const rows = Object.values(transfers);
  const pending = rows.filter((t) => t.status === "in arrivo").length;
  const active = rows.filter(
    (t) => t.status === "in corso" || t.status === "in stallo"
  ).length;

  const myName = status?.display_name || "Io";

  const counts: Partial<Record<Route, { n: number; hot?: boolean }>> = {
    transfers: pending
      ? { n: pending, hot: true }
      : active
        ? { n: active }
        : { n: 0 },
    people: { n: contacts.length },
  };

  return (
    <nav className="rail" aria-label="Navigazione principale">
      <div className="rail-top" />

      <button
        className="rail-me"
        onClick={() => go("settings")}
        title="La tua identità e le impostazioni"
      >
        <Avatar name={myName} id={status?.public_id} size={30} />
        <span className="grow truncate">
          <span
            className="truncate"
            style={{ display: "block", fontWeight: 600, fontSize: 13 }}
          >
            {myName}
          </span>
          <span
            className="truncate mono"
            style={{ display: "block", fontSize: 10.5, color: "var(--ink-mut)" }}
          >
            {status?.fingerprint || "identità non ancora letta"}
          </span>
        </span>
        <span
          className={`dot ${connected ? "on" : "bad"}`}
          title={connected ? "Daemon connesso" : "Daemon non raggiungibile"}
        />
      </button>

      <div className="rail-group">
        <button
          className="rail-verb send"
          onClick={() => openSheet([])}
          title="Invia…"
        >
          <span className="ico">
            <Icon.Send />
          </span>
          <span>Invia…</span>
        </button>
        <button className="rail-verb recv" onClick={openReceive} title="Ricevi…">
          <span className="ico">
            <Icon.Receive />
          </span>
          <span>Ricevi…</span>
        </button>
      </div>

      <div className="rail-group">
        <div className="t-label">Raccolte</div>
        {PLACES.map((p) => {
          const c = counts[p.route];
          return (
            <button
              key={p.route}
              className="rail-item"
              aria-current={route === p.route ? "page" : undefined}
              onClick={() => go(p.route)}
              title={p.label}
            >
              <span className="ico">{p.icon}</span>
              <span className="grow truncate">{p.label}</span>
              {c && c.n > 0 && (
                <span className={`rail-count tnum ${c.hot ? "hot" : ""}`}>
                  {c.n}
                </span>
              )}
            </button>
          );
        })}
      </div>

      <div className="rail-foot">
        <button
          className="rail-item"
          onClick={() => setPaletteOpen(true)}
          title={`Cerca ed esegui (${modKey}K)`}
        >
          <span className="ico">
            <Icon.Search />
          </span>
          <span className="grow truncate">Cerca ed esegui</span>
          <Kbd>mod</Kbd>
          <Kbd>K</Kbd>
        </button>
      </div>
    </nav>
  );
}
