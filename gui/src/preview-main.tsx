// Dev-only: the phone screens in a browser, on the tests' fake bridge.
//
// Not shipped and not imported by anything the app builds — `vite.preview.config.ts`
// is what serves it. It exists so a change to the phone's materials can be *looked
// at* before it is believed: jsdom lays nothing out, and the window cannot be made
// narrow enough to be a phone without also being a window.

import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { initLang } from "./i18n";
import { harness, dto, PEER_ID } from "./test/mocks";
import { useStore } from "./store";
import "@fontsource-variable/plus-jakarta-sans";
import "@fontsource-variable/inter";
import "./theme.css";

const now = Math.floor(Date.now() / 1000);
const OTHER = "k7f2mnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc79ab1";

// Fixtures at the length real ones turned out to be. The first pass of this file
// used short names, no `last_exchange` and three transfers, and every one of the
// defects the phone showed — a meta line that ran into the next one, a send
// button pushed off the card, a board buried under finished rows — was invisible
// here for exactly that reason. A fixture that is shorter than the real thing is
// a fixture that tests nothing about layout.
harness.snapshot.contacts = [
  dto.contact({ name: "giulia", display_name: "Giulia Bianchi", id: OTHER, verified: true, trusted: true, last_exchange: now - 16 * 86400 }),
  dto.contact({ name: "marco", display_name: "Marco Rinaldi", verified: true, last_exchange: now - 3 * 86400 }),
  dto.contact({ name: "pillow-guitar-canoe", display_name: "", id: "p1ll0wmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7", last_exchange: now - 16 * 86400 }),
  dto.contact({ name: "studio-neri", display_name: "Studio Neri", id: "m3c0mnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc77de" }),
];
harness.snapshot.devices = [
  { id: "aa00000000000000000000000000aa00", name: "localhost", platform: "linux", last_seen: 0, is_this_device: true },
  { id: "bb00000000000000000000000000bb00", name: "MacBook Pro di Lorenzo", platform: "macos", last_seen: now - 35 * 60, is_this_device: false },
  { id: "cc00000000000000000000000000cc00", name: "iPhone di Lorenzo", platform: "ios", last_seen: now - 7 * 60, is_this_device: false },
];
harness.snapshot.presence = { [OTHER]: true, [PEER_ID]: true };
const ME = "if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha";
// Who this phone is, so an offer from `ME` reads as one of your own machines
// (the Prendi row of 15a) rather than as a stranger.
if (harness.snapshot.status) harness.snapshot.status = { ...harness.snapshot.status, public_id: ME };
harness.snapshot.pending = [
  dto.offer({ id: "off1", from: "marco", name: "rilievi-cantiere.zip", size: 312_000_000, known_name: "marco", sender_name: "Marco Rinaldi" }),
  // From one of your own machines: same identity, so *Scambia* offers to take it
  // without anything being framed. The sender name on an offer belongs to the
  // identity rather than to the machine, which is why the row says "one of
  // yours" and not which — see `ExchangeView`.
  dto.offer({ id: "off2", from: ME, name: "Relazione sopralluogo via Manzoni 14 scala B — rev. finale.pdf", size: 41_000_000, sender_name: "Io" }),
];
harness.snapshot.transfers = [
  dto.transfer({ id: 1, direction: "recv", peer: "marco", name: "rilievi-cantiere.zip", total_size: 312_000_000, transferred: 74_000_000, swarm_peers: 3, download_peers: 3 }),
  dto.transfer({ id: 2, direction: "send", peer: OTHER, name: "contratto-firmato.pdf", total_size: 4_800_000, transferred: 2_976_000 }),
  dto.transfer({ id: 3, direction: "send", peer: "studio-neri", name: "pianta-piano-terra.dwg", total_size: 18_400_000, transferred: 0, status: "waiting" }),
];
harness.snapshot.deposits = [
  dto.deposit({ id: "a1b2c3d4", name: "foto-cantiere-lotto-b.zip", size: 184_000_000, expires: now + 4 * 86400 }),
  dto.deposit({ id: "b2c3d4e5", kind: "mailbox", name: "listino-2026.pdf", size: 2_100_000, recipient: "studio-neri", link: "", present: false, max_label: "1" }),
];
// A phone that has been used for a fortnight: the board is mostly things that
// already finished, and the one that is still moving has to survive them.
for (let i = 0; i < 14; i++) {
  harness.snapshot.transfers.push(
    dto.transfer({
      id: 100 + i,
      direction: i % 3 === 0 ? "send" : "recv",
      peer: i % 2 ? "pillow-guitar-canoe" : "giulia",
      name: ["piccolo.txt", "medio.bin", "grosso.bin", "uno.txt", "due.txt"][i % 5],
      total_size: 43 + i * 1000,
      transferred: 43 + i * 1000,
      status: "completed",
      // Half of them from a version that never recorded a date — which is what
      // the phone's restored board actually looks like.
      created: i % 2 ? 0 : now - (i + 1) * 3600,
    })
  );
}

harness.snapshot.history = [
  { id: "h1", direction: "send", peer: "giulia", name: "foto-caldaia-01.heic", total_size: 3_200_000, transferred: 3_200_000, status: "completed", created: now - 3600 },
  { id: "h2", direction: "recv", peer: "anna", name: "preventivo-2026.xlsx", total_size: 840_000, transferred: 840_000, status: "completed", created: now - 7200 },
  { id: "h3", direction: "send", peer: "luca", name: "video-sopralluogo.mp4", total_size: 1_400_000_000, transferred: 1_100_000_000, status: "failed: connessione interrotta", created: now - 86400 },
];

// The phone's library and clipboard, as the compose screen (11b) sees them:
// six squares in the warm palette stand in for the last pictures taken.
harness.photos = ["#c9b8a8", "#8a9bb0", "#b5a06c", "#7c8f7a", "#a98d9c", "#6f7f8c", "#bcd3ec", "#c4cad6", "#e2ecf7", "#9cc4ef", "#dde1e9", "#a3b1c6", "#c9b8a8", "#8a9bb0", "#b5a06c"].map((c, i) => ({
  id: `p${i}`,
  name: `IMG_${4120 + i}.HEIC`,
  isVideo: i === 3,
  duration: i === 3 ? 74 : 0,
  thumb: `data:image/svg+xml;utf8,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="120" height="120"><rect width="120" height="120" fill="${c}"/></svg>`)}`,
}));

initLang();

// Which screen to open on, from the query string: `?route=people`, `?route=exchange`,
// `?sheet=send`. The preview is driven by screenshots as often as by a finger —
// and a screenshot cannot tap its way to the fourth tab.
const q = new URLSearchParams(location.search);
const route = q.get("route");
if (route) useStore.setState({ route: route as never, stack: [route as never] });
// `?quiet=1`: nothing passing, so the roots show without the card over the bar
// (15b); `?pick=marco,giulia`: Scambia with those faces ticked (16a).
if (q.get("quiet")) {
  harness.snapshot.transfers = harness.snapshot.transfers.filter((x) => x.status === "completed");
}
// `?deps=9`: una lista di depositi lunga come quella di chi usa l'app da un
// mese — che è l'unica in cui si vede se l'ultima riga arriva sotto la barra.
const nDeps = Number(q.get("deps") ?? 0);
if (nDeps > 0) {
  harness.snapshot.deposits = Array.from({ length: nDeps }, (_, i) =>
    dto.deposit({
      id: `d${i}`,
      kind: i % 3 === 2 ? "mailbox" : "link",
      name: ["foto-cantiere-lotto-b.zip", "listino-2026.pdf", "planimetria-piano-terra_v3.dwg"][i % 3],
      size: 1_000_000 * (i + 3),
      expires: now + (i + 1) * 86400,
      recipient: i % 3 === 2 ? "studio-neri" : undefined,
    })
  );
}
// `?code=1`: un codice acceso, cioè la schermata Codici con qualcosa dentro —
// che altrimenti in anteprima non si vede mai, perché nessuna fixture ne ha uno.
if (q.get("code")) {
  harness.snapshot.transfers = [
    dto.transfer({
      id: 7,
      direction: "send",
      name: "rilievi-cantiere.zip",
      total_size: 312_000_000,
      transferred: 0,
      status: "sharing",
      code: "4821-crater-mango",
    }),
  ];
}
const pick = q.get("pick");
if (pick) useStore.setState({ picking: pick.split(",") });
// `?stack=settings,advanced`: a pushed screen, with the way back in the bar.
const stack = q.get("stack");
if (stack) {
  const s = stack.split(",") as never[];
  useStore.setState({ route: s[s.length - 1], stack: s });
}
if (q.get("sheet") === "send") useStore.getState().openSheet([]);
// `?files=3`: arrived from Condividi with that many pictures in hand (11e).
const nFiles = Number(q.get("files") ?? 0);
if (nFiles > 0) {
  useStore.getState().setComposeFiles(
    Array.from({ length: nFiles }, (_, i) => ({ id: `s${i}`, name: `IMG_${4120 + i}.jpg`, size: 4_700_000 + i * 100_000, isDir: false }))
  );
}
// `?compose=contact:marco` | `self:MacBook Pro di Lorenzo` | `link`: the second
// question (11b–11d), with the first already answered.
const compose = q.get("compose");
if (compose) {
  const [kind, arg] = compose.split(":");
  const to =
    kind === "link"
      ? { kind: "link" as const }
      : kind === "self"
        ? { kind: "self" as const, device: arg }
        : // `group:marco,giulia`: l'invio a più persone (16a/17b), che altrimenti
          // si raggiunge solo con un tocco lungo e tre tocchi.
          kind === "group"
          ? { kind: "group" as const, names: arg.split(",") }
          : { kind: "contact" as const, name: arg };
  useStore.setState({ route: "exchange", stack: ["exchange"] });
  window.setTimeout(() => useStore.getState().startCompose(to), 50);
}
// Through the store, or its own theme effect puts "system" back on the next tick.
if (q.get("theme")) useStore.getState().setTheme(q.get("theme") as "dark" | "light");
// `?rt=1`: the glass as "Reduce transparency" leaves it, which a browser cannot
// be asked to simulate — see `.app-phone.rt` in theme.css.
if (q.get("rt")) document.documentElement.classList.add("rt");
// `?dt=1.35`: the text at a system size the browser cannot be asked for — the
// factor `installDynamicType` would have written (see `phone.ts`).
if (q.get("dt")) document.documentElement.dataset.dt = q.get("dt")!;
// `?kb=300`: a keyboard that tall, as `keyboard.ts` would have reported it.
if (q.get("kb")) {
  document.documentElement.style.setProperty("--kb", `${q.get("kb")}px`);
  document.documentElement.dataset.kb = "1";
  document.documentElement.dataset.kbPin = "1";
}
// `?click=.act-tool:last-child`: one tap after the first paint, for the states
// a screenshot cannot reach by itself (the search dock, a sheet, a menu).
// `?hold=.act-row`: a finger that goes down on that element and stays there,
// which is what opens a row's context menu.
const hold = q.get("hold");
if (hold) {
  window.setTimeout(() => {
    const el = document.querySelector<HTMLElement>(hold);
    if (!el) return;
    const r = el.getBoundingClientRect();
    const touch = new Touch({ identifier: 1, target: el, clientX: r.left + 40, clientY: r.top + r.height / 2 });
    el.dispatchEvent(new TouchEvent("touchstart", { touches: [touch], bubbles: true, cancelable: true }));
  }, 600);
}
const click = q.get("click");
if (click) {
  window.setTimeout(() => {
    document.querySelector<HTMLElement>(click)?.click();
  }, 400);
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
