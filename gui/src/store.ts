// The single source of truth for the board: a Zustand store seeded from a daemon
// snapshot and then mutated purely by pushed engine events (no polling).

import { create } from "zustand";
import { api, onConnected, onDaemonError, onEngineEvent } from "./ipc";
import { shortId } from "./format";
import { t } from "./i18n";
import { toast } from "./ui/Toasts";
import type {
  ConfigDto,
  ConfigPatch,
  ContactDto,
  DepositDto,
  EngineEvent,
  HistoryDto,
  Method,
  OfferDto,
  PairKind,
  StatusDto,
  SyncDto,
  TransferDto,
  UIStatus,
  UITransfer,
} from "./types";

/** The six places the app can be. One at a time, always — the previous model
 *  used three independent booleans and could describe two panels owning the main
 *  pane at once, which is a state nothing could draw. */
export type Route =
  | "transfers"
  | "people"
  | "deposits"
  | "history"
  | "devices"
  | "settings";

/** What each place is called, as a dictionary key rather than as words. The rail,
 *  the header and the command palette all name the same six places; a second
 *  list of literals would be a second list to keep in step. */
export const TITLE_KEY = {
  transfers: "title.transfers",
  people: "title.people",
  deposits: "title.deposits",
  history: "title.history",
  devices: "title.devices",
  settings: "title.settings",
} as const satisfies Record<Route, string>;

export type ThemeChoice = "system" | "light" | "dark";

/** The four ways a send can reach someone. Lives here rather than in the send
 *  sheet because callers elsewhere preselect one — see `sheetMode`. */
export type SendMode = "contact" | "code" | "link" | "ticket";

/** Fire a store action from a click handler and drop its rejection.
 *
 *  Every action re-throws so that callers which *do* await can react — the send
 *  sheet keeps itself open on a refusal, for one. A menu item has nothing to
 *  react with: the failure has already been recorded and raised as a toast by
 *  the time the promise settles, so the only thing left for the rejection to do
 *  is become an unhandled one. `void p` is not enough for that; the catch is. */
export function fire(p: Promise<unknown>): void {
  void p.catch(() => {});
}

/** A pairing exchange in flight. It is not request/reply — it waits on a person
 *  at another machine — so the UI tracks it as a little state machine fed by
 *  `pairing_*` events rather than by an awaited promise. */
export interface PairingState {
  session: string | null;
  kind: PairKind;
  /** The code to read out (hosting) or that was typed in (joining). */
  code: string;
  phase: "starting" | "waiting" | "done" | "failed";
  message: string;
  /** Set when a device join replaced this daemon's identity: it has to restart
   *  before anything else it reports can be believed. */
  needsRestart: boolean;
}

const THEME_KEY = "arvolo.theme";
const ORDER_KEY = "arvolo.order";

/** Pairing events that arrived before their session handle did.
 *
 *  Kept outside the store because they are a transport detail, not state a
 *  component should ever see: the daemon spawns the session before writing the
 *  reply that names it, so the outcome can genuinely beat the handle. */
const earlyPairingEvents: EngineEvent[] = [];

/** Apply a theme choice to <html>. "system" removes the attribute entirely so the
 *  `prefers-color-scheme` media query in theme.css takes over again — setting it
 *  to some third value would leave both branches unmatched. */
export function applyTheme(choice: ThemeChoice) {
  const root = document.documentElement;
  if (choice === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", choice);
}

function readTheme(): ThemeChoice {
  try {
    const v = localStorage.getItem(THEME_KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    // Private-mode webviews can throw on localStorage access. A theme is not
    // worth failing to boot over.
  }
  return "system";
}

function now(): number {
  return Date.now();
}

/** The order the user dragged the board into, last time they dragged it: row key
 *  → rank. Kept here rather than in the daemon on purpose. The daemon knows what
 *  the transfers are; the order you like to look at them in is a property of this
 *  window, the same kind of thing as the theme and the language, and `arvolo
 *  status` has its own ordering that has no reason to inherit this one. The cost
 *  is that it does not follow you to another machine — that would mean putting it
 *  in the daemon and through identity sync, which is a lot of machinery for the
 *  order of a list.
 *
 *  Keys are daemon-local ids (`t7`, `o3`). Point the GUI at a *different* daemon
 *  and the ids will collide with somebody else's numbering: the worst that
 *  happens is an arbitrary order, which is what you would have had anyway. */
function readOrder(): Record<string, number> {
  try {
    const raw = localStorage.getItem(ORDER_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: Record<string, number> = {};
    for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof v === "number" && Number.isFinite(v)) out[k] = v;
    }
    return out;
  } catch {
    // A refusing localStorage, or something else's key at ours. Neither is worth
    // failing to boot over: an unremembered order is the state we shipped with.
    return {};
  }
}

const savedRanks = readOrder();

/** Strictly increasing list position. `Date.now()` is not usable here: two rows
 *  created in the same millisecond would tie, leaving their order ambiguous and
 *  making a reorder shuffle two equal ranks — a move that changes nothing.
 *
 *  It starts above every remembered rank so that a row nobody has placed by hand
 *  still arrives at the top of the board, remembered order or not. */
let rankSeq = Math.max(0, ...Object.values(savedRanks));
function nextRank(): number {
  return ++rankSeq;
}

/** The rank a row should carry: the one it already has, else the one it was left
 *  at in a previous run, else a fresh one at the top. */
function rankFor(key: string, prev?: UITransfer): number {
  return prev?.rank ?? savedRanks[key] ?? nextRank();
}

/** Drop the rows that will never change again, keeping everything that still has
 *  a future — the daemon's own rule, which is why a deposit awaiting pickup stays
 *  (it reads as done and isn't: it can still be withdrawn, or picked up).
 *
 *  Shared by the button and by the `finished_cleared` event, so a clear done from
 *  the CLI leaves this window showing exactly what a clear done here would. */
function dropFinished(s: { transfers: Record<string, UITransfer> }): {
  transfers: Record<string, UITransfer>;
} {
  const kept: Record<string, UITransfer> = {};
  for (const [k, tx] of Object.entries(s.transfers)) {
    const finished =
      tx.status === "completed" ||
      tx.status === "failed" ||
      tx.status === "cancelled";
    if (!finished) kept[k] = tx;
  }
  return { transfers: kept };
}

/** Remember the order of the rows on the board right now.
 *
 *  Written only when the user reorders, which is also what prunes it: rows that
 *  have since left the board are simply not in the map that gets written. */
function saveOrder(transfers: Record<string, UITransfer>) {
  try {
    const out: Record<string, number> = {};
    for (const t of Object.values(transfers)) out[t.key] = t.rank;
    localStorage.setItem(ORDER_KEY, JSON.stringify(out));
  } catch {
    // Same as reading it: the board still works, it just forgets.
  }
}

/** Last progress sample per transfer id, for throughput estimation. Kept outside
 *  the store: it changes on every event and must not trigger renders itself. */
const progSamples = new Map<number, { t: number; bytes: number }>();

/** Exponentially-smoothed bytes/sec from consecutive progress events. */
function sampleRate(id: number, bytes: number, prevRate?: number): number | undefined {
  const at = now();
  const last = progSamples.get(id);
  progSamples.set(id, { t: at, bytes });
  if (!last || at <= last.t || bytes < last.bytes) return prevRate;
  const inst = ((bytes - last.bytes) * 1000) / (at - last.t);
  return prevRate ? 0.7 * prevRate + 0.3 * inst : inst;
}

/** What a row is doing, from the daemon's status *and* what kind of row it is.
 *
 *  The engine calls a background serve "active" because it is running, which is
 *  true and unhelpful: nothing is in flight, the file is just available. Rendered
 *  as a transfer, a served ticket sits at 100% for ever — indistinguishable from
 *  one that stalled — and the seeding a finished download turns into shows up as a
 *  0% outgoing send of a file the user never sent. Both are `sharing`.
 *
 *  While someone is actually pulling, it *is* a transfer, and stays "active" so
 *  the progress and rate mean what they say. */
function statusOf(dto: TransferDto): { status: UIStatus; reason?: string } {
  const base = toUIStatus(dto.status);
  if (base.status === "active" && dto.sharing && dto.download_peers === 0)
    return { status: "sharing" };
  return base;
}

/** Split a daemon status string into a UI status + optional reason. */
function toUIStatus(raw: string): { status: UIStatus; reason?: string } {
  if (raw === "active") return { status: "active" };
  if (raw === "completed") return { status: "completed" };
  if (raw === "deposited") return { status: "deposited" };
  if (raw === "cancelled") return { status: "cancelled" };
  const [head, ...rest] = raw.split(":");
  const reason = rest.join(":").trim() || undefined;
  if (head === "waiting") return { status: "stalled", reason };
  if (head === "paused") return { status: "paused", reason };
  if (head === "failed") return { status: "failed", reason };
  return { status: "active" };
}

function methodFor(dto: TransferDto): Method {
  if (dto.status.startsWith("deposited")) return "cloud";
  if (dto.download_peers > 0 || dto.swarm_peers > 0) return "p2p";
  return "p2p";
}

interface State {
  connected: boolean;
  status: StatusDto | null;
  guiVersion: string;
  contacts: ContactDto[];
  contactsById: Record<string, ContactDto>;
  transfers: Record<string, UITransfer>;
  /** Why the last snapshot could not be read, if it could not. Silence here is how
   *  the board came to show "Connected · 0 sends" while the daemon held two live
   *  sends: a swallowed error is indistinguishable from an empty list. */
  loadError: string | null;
  /** Why the user's last action failed, if it did. Every action is fired from an
   *  `onClick` that cannot await it, so a rejected promise had nowhere to go: the
   *  button simply did nothing, silently, and the user was left guessing. */
  actionError: string | null;
  dismissActionError: () => void;

  /** What is still on a relay and can be taken back. Unlike the board's rows these
   *  are **not** event-driven: no engine event exists for a deposit, and the relay
   *  never reports a download back, so this list is only ever as fresh as the last
   *  fetch. It is fetched when the panel opens, and the panel is the only place it
   *  is shown — precisely so it cannot sit on screen going quietly stale. */
  deposits: DepositDto[];
  depositsLoading: boolean;
  depositsError: string | null;
  /** Ids currently being withdrawn, so a row can show the click landed and cannot
   *  be double-submitted (the relay round-trip is not instant). */
  revoking: string[];

  /** The history log, fetched when its panel opens (like the deposits: there is
   *  no push event for it, so open-is-the-refresh keeps it honest). */
  history: HistoryDto[];
  historyLoading: boolean;
  historyError: string | null;

  /** Settings, fetched when the settings screen opens. Like the deposits and the
   *  history there is no push event for it, so open-is-the-refresh. */
  config: ConfigDto | null;
  configLoading: boolean;
  configError: string | null;

  /** Who is reachable right now, by public id. `null` in a slot means the relay
   *  could not be asked — never "offline". Fetched when the people screen opens
   *  and on demand, never as part of the address book itself: the book is read
   *  from disk and is instant, this is a relay round trip per contact. */
  presence: Record<string, boolean | null>;
  presenceLoading: boolean;
  loadPresence: () => Promise<void>;

  /** Multi-device summary, same fetch-on-open contract as `config`. */
  sync: SyncDto | null;
  syncLoading: boolean;
  syncError: string | null;

  /** The pairing exchange currently on screen, if any. */
  pairing: PairingState | null;

  // UI state
  route: Route;
  theme: ThemeChoice;
  search: string;
  pauseAll: boolean;
  paletteOpen: boolean;
  sheetPaths: string[] | null; // send sheet open when non-null
  /** Recipient the send sheet should open on, when it was opened *from* someone —
   *  a person card, or "Invia a X" in the palette. Without this, choosing a
   *  person and then having to choose them again is the app forgetting what the
   *  user just told it. */
  sheetTo: string | null;
  /** Which way of sending the sheet should open on, when the section it was
   *  opened from already implies one — "Crea un link" under Link e depositi can
   *  only mean a link. Left null where the section implies nothing (the rail's
   *  Invia, a drop on the window), and the sheet starts on a contact. */
  sheetMode: SendMode | null;
  incomingOfferId: string | null; // incoming offer dialog
  /** Transfer id whose share panel is open, if any. A share has no progress to
   *  watch, so its numbers live behind a panel rather than crowding the row. */
  shareOpen: number | null;
  receiveOpen: boolean; // paste-a-ticket sheet
  /** The person whose detail sheet is open, by contact name. */
  personOpen: string | null;

  // lifecycle
  init: () => Promise<() => void>;
  reload: () => Promise<void>;
  refreshContacts: () => Promise<void>;

  // helpers
  peerLabel: (id: string | null, fallbackName?: string) => string;
  isVerified: (id: string | null) => boolean;

  // event application
  applyEvent: (ev: EngineEvent) => void;

  // ui setters
  setSearch: (q: string) => void;
  /** Navigate. Routes whose data has no push event refetch on arrival. */
  go: (r: Route) => void;
  setTheme: (choice: ThemeChoice) => void;
  setPaletteOpen: (v: boolean) => void;
  openSheet: (paths: string[], to?: string, mode?: SendMode) => void;
  closeSheet: () => void;
  openIncoming: (offerId: string) => void;
  closeIncoming: () => void;
  openShare: (id: number) => void;
  closeShare: () => void;
  openPerson: (name: string | null) => void;
  /** Fetch what is still on a relay. There is no event to keep it live, so a
   *  stale list must never be what greets the user: `go("deposits")` refetches,
   *  and so does anything that *adds* a row — see `link` and `deposit`. */
  loadDeposits: () => Promise<void>;
  /** Withdraw a deposit from the relay and forget it. Irreversible: the link stops
   *  working for everyone who has it. */
  revokeDeposit: (id: string) => Promise<void>;

  openReceive: () => void;
  closeReceive: () => void;
  loadHistory: () => Promise<void>;
  /** Forget the whole daemon-side history log. */
  clearHistory: () => Promise<void>;

  loadConfig: () => Promise<void>;
  /** Write settings back. The daemon answers with the state that resulted, so the
   *  screen shows what was actually saved rather than what was typed. */
  saveConfig: (patch: ConfigPatch) => Promise<void>;
  loadSync: () => Promise<void>;
  /** Run one address-book sync round now. */
  syncNow: () => Promise<void>;
  /** Drop advertised-name records for contacts that no longer exist. */
  pruneNames: () => Promise<number>;

  /** Begin a pairing exchange and park its state for the sheet to render. */
  startPairing: (kind: PairKind, code?: string, name?: string) => Promise<void>;
  /** Abandon the running exchange (also what closing the sheet must do). */
  cancelPairing: () => Promise<void>;
  clearPairing: () => void;

  // actions (forward to the daemon, then let events reconcile)
  send: (to: string, paths: string[], note: string) => Promise<number>;
  /** `send --deposit`: skip the live attempt entirely. Returns the `arvm…`
   *  ticket, which is the sender's copy for hand-delivery. */
  deposit: (
    to: string,
    paths: string[],
    note: string,
    ttl: number | null,
    max: number | null,
    password: string | null
  ) => Promise<{ id: number; ticket: string }>;
  ticket: (paths: string[]) => Promise<{ id: number; ticket: string }>;
  /** Host a short pairing code in the daemon (keep = serve every receiver). */
  code: (paths: string[], keep: boolean) => Promise<{ id: number; code: string }>;
  link: (path: string, ttl: number | null, max: number | null) => Promise<string>;
  /** Receive from a pasted arvc… ticket, pairing code or arvm… offline ticket. */
  receive: (ticket: string, out: string | null, password: string | null) => Promise<number>;
  /** Accept a parked offer. `password` is only needed when the offer points at a
   *  mailbox deposit sealed with one — which nothing reveals until the fetch
   *  fails, so the UI asks after the first refusal. */
  accept: (
    offerId: string,
    out: string | null,
    password?: string | null
  ) => Promise<void>;
  reject: (offerId: string) => Promise<void>;
  pause: (id: number) => Promise<void>;
  resume: (id: number) => Promise<void>;
  cancel: (id: number) => Promise<void>;
  /** Per-row "Elimina": drop a finished transfer from daemon + local list. */
  removeRow: (key: string) => Promise<void>;
  /** Mark a saved contact verified, then refresh contacts + row badges. */
  markVerified: (name: string) => Promise<void>;
  markUnverified: (name: string) => Promise<void>;
  /** Trust auto-download. The daemon refuses an unverified contact unless forced. */
  markTrusted: (who: string, force: boolean) => Promise<void>;
  markUntrusted: (who: string) => Promise<void>;
  blockContact: (who: string) => Promise<void>;
  unblockContact: (who: string) => Promise<void>;
  acceptName: (who: string) => Promise<void>;
  addContact: (name: string, id: string) => Promise<void>;
  removeContact: (name: string) => Promise<void>;
  renameContact: (old: string, newName: string) => Promise<void>;
  /** Set (or clear) the display name advertised inside offers. */
  setMyName: (name: string) => Promise<void>;
  /** Stop the stale daemon; the event pump respawns a fresh one. */
  restartDaemon: () => Promise<void>;
  /** Reorder a list of rows: the keys, top to bottom, take the ranks those same
   *  rows already held. Rows outside the list keep their place. */
  reorderItems: (keys: string[]) => void;
  togglePauseAll: () => Promise<void>;
  /** Drop every finished row, daemon-side first — a local-only clear would just
   *  see them all come back with the next snapshot. */
  clearFinished: () => Promise<void>;
}

export const useStore = create<State>((set, get) => {
  /** Run a daemon action, surfacing any refusal. The UI fires these from click
   *  handlers that cannot await, so without this a rejection is swallowed by the
   *  event loop and the button looks broken rather than blocked. Re-throws so
   *  callers that *do* await (and want to react) still can. */
  const act = async <T,>(what: string, fn: () => Promise<T>): Promise<T> => {
    try {
      const r = await fn();
      set({ actionError: null });
      return r;
    } catch (e) {
      set({ actionError: `${what}: ${String(e)}` });
      throw e;
    }
  };

  /** Re-read the deposits after something has just added one.
   *
   *  The list is the only place a link exists once the panel that produced it is
   *  closed — it is where you go to copy the URL again, hand it to someone else,
   *  or take it back. Nothing pushes to it (see `loadDeposits`), so a link made
   *  while it was on screen would simply not be there, and a list that is missing
   *  the very link you just made reads as "it wasn't kept".
   *
   *  Deliberately not awaited by its callers: the fetch asks every relay about
   *  every deposit and can take seconds, while the caller's own job — handing the
   *  user their URL — is already done. `loadDeposits` swallows its own failure
   *  into `depositsError`, so nothing here can reject. */
  const refreshDeposits = () => {
    void get().loadDeposits();
  };

  const dtoToUI = (d: TransferDto, prev?: UITransfer): UITransfer => {
    const { status, reason } = statusOf(d);
    const dir = d.direction === "send" ? "out" : "in";
    return {
      key: `t${d.id}`,
      id: d.id,
      dir,
      name: d.name,
      size: d.total_size,
      transferred: d.transferred,
      status,
      reason,
      peer: get().peerLabel(d.peer, prev?.peer),
      peerId: d.peer ?? prev?.peerId,
      encrypted: true,
      verified: get().isVerified(d.peer),
      method: methodFor(d),
      swarmPeers: d.swarm_peers,
      downloadPeers: d.download_peers,
      files: prev?.files ?? 1,
      // The engine's answer wins: it is the one that survives a restart. Falling
      // back to what we already had keeps a path learned from a live event when
      // talking to a daemon too old to report it.
      path: d.path ?? prev?.path,
      // The engine's own clock, not ours: a row we first see today may well be
      // yesterday's. `created` is 0 only from a daemon that predates the field —
      // then, and only then, fall back to when we noticed it.
      firstSeen: d.created > 0 ? d.created * 1000 : (prev?.firstSeen ?? now()),
      rank: rankFor(`t${d.id}`, prev),
      rate: prev?.rate,
      code: d.code ?? prev?.code,
      offerStatus: d.offer_status ?? undefined,
      copiesServed: d.copies_served ?? 0,
      bytesServed: d.bytes_served ?? 0,
      lastPickup: d.last_pickup ?? 0,
      fromDownload: d.from_download ?? 0,
    };
  };

  const offerToUI = (o: OfferDto, prev?: UITransfer): UITransfer => ({
    key: `o${o.id}`,
    id: 0,
    offerId: o.id,
    dir: "in",
    name: o.name,
    size: o.size,
    transferred: 0,
    status: "incoming",
    peer: get().peerLabel(o.from, o.sender_name || undefined),
    peerId: o.from,
    note: o.note || undefined,
    senderName: o.sender_name || undefined,
    encrypted: true,
    verified: get().isVerified(o.from),
    method: "cloud",
    swarmPeers: 0,
    downloadPeers: 0,
    files: 1,
    firstSeen: prev?.firstSeen ?? now(),
    rank: rankFor(`o${o.id}`, prev),
    copiesServed: 0,
    bytesServed: 0,
    lastPickup: 0,
    fromDownload: 0,
  });

  /** Merge a partial change into an existing transfer row (creating a stub if the
   *  row is unknown — e.g. an event arrived before its snapshot). */
  const patch = (id: number, fn: (tx: UITransfer) => UITransfer) =>
    set((s) => {
      const key = `t${id}`;
      const existing =
        s.transfers[key] ??
        ({
          key,
          id,
          dir: "out",
          name: "…",
          size: 0,
          transferred: 0,
          status: "active",
          encrypted: true,
          verified: false,
          method: "p2p",
          swarmPeers: 0,
          downloadPeers: 0,
          files: 1,
          firstSeen: now(),
          rank: rankFor(key),
        } as UITransfer);
      return { transfers: { ...s.transfers, [key]: fn(existing) } };
    });

  return {
    connected: false,
    status: null,
    guiVersion: "",
    loadError: null,
    actionError: null,
    contacts: [],
    contactsById: {},
    transfers: {},
    route: "transfers",
    theme: readTheme(),
    search: "",
    pauseAll: false,
    paletteOpen: false,
    sheetPaths: null,
    sheetTo: null,
    sheetMode: null,
    incomingOfferId: null,
    shareOpen: null,
    receiveOpen: false,
    personOpen: null,
    presence: {},
    presenceLoading: false,
    config: null,
    configLoading: false,
    configError: null,
    sync: null,
    syncLoading: false,
    syncError: null,
    pairing: null,
    deposits: [],
    depositsLoading: false,
    depositsError: null,
    revoking: [],
    history: [],
    historyLoading: false,
    historyError: null,

    peerLabel: (id, fallbackName) => {
      if (!id) return fallbackName || t("store.unknownPeer");
      const c = get().contactsById[id];
      if (c) return c.name;
      if (fallbackName) return fallbackName;
      return shortId(id);
    },
    isVerified: (id) => (id ? !!get().contactsById[id]?.verified : false),

    init: async () => {
      // Before anything paints: the stored choice has to beat the media query on
      // the very first frame, or a dark-theme user sees a white flash on launch.
      applyTheme(get().theme);
      api
        .guiVersion()
        .then((v) => set({ guiVersion: v }))
        .catch(() => {});
      const unlistenEv = await onEngineEvent((ev) => get().applyEvent(ev));
      const unlistenConn = await onConnected((c) => {
        set({ connected: c });
        // A daemon that came up explains itself by existing: drop any stale
        // reason so the banner does not outlive the problem it described.
        if (c) {
          set({ loadError: null });
          get().reload();
        }
      });
      // The daemon has no terminal when the GUI spawns it, so without this its
      // reason for not starting — an identity file it refuses, a relay it cannot
      // reach — reaches nobody, and the window shows only "disconnesso".
      const unlistenDaemonErr = await onDaemonError((reason) => {
        if (!get().connected) set({ loadError: reason });
      });

      // Seed the snapshot, retrying until it lands. The backend pump emits
      // `engine://connected` only on a state *change*, so a webview that loads
      // (or reloads) while the pump is already subscribed never receives one:
      // if its first snapshot failed — the daemon was still starting, say — the
      // board would sit empty forever with no event to wake it. Retrying here is
      // the only self-heal, and it costs nothing once connected (the loop exits).
      let stopped = false;
      const seed = async () => {
        while (!stopped) {
          await get().reload();
          if (get().connected) return;
          await new Promise((r) => setTimeout(r, 2000));
        }
      };
      void seed();

      return () => {
        stopped = true;
        unlistenEv();
        unlistenConn();
        unlistenDaemonErr();
      };
    },

    reload: async () => {
      // Nothing here may fail quietly. A swallowed error looks exactly like an empty
      // daemon — and an empty board next to a green "Connesso" is a lie the user
      // cannot see through. If we cannot read the list, say so and keep what we had.
      let st: StatusDto | null = null;
      try {
        const [status, contacts, transfers, pending] = await Promise.all([
          api.status(),
          api.listContacts(),
          api.listTransfers(),
          api.listPending(),
        ]);
        st = status;

        const contactsById: Record<string, ContactDto> = {};
        for (const c of contacts) contactsById[c.id] = c;
        // Land the address book *before* deriving rows: each row resolves its peer
        // name and verified badge through it, so building them first would stamp
        // them from the previous book (a freshly verified contact stayed unverified).
        set({ contacts, contactsById });

        // Rebuild the rows, preserving firstSeen/rank for rows we already track.
        const prev = get().transfers;
        const map: Record<string, UITransfer> = {};
        for (const d of transfers) map[`t${d.id}`] = dtoToUI(d, prev[`t${d.id}`]);
        for (const o of pending) map[`o${o.id}`] = offerToUI(o, prev[`o${o.id}`]);

        set({ status: st, transfers: map, connected: true, loadError: null });
      } catch (e) {
        set({
          connected: false,
          loadError: t("store.loadTransfers", String(e)),
        });
      }
    },

    // Contacts only — the rows are event-driven and must not be rebuilt from a
    // snapshot just because the address book moved.
    refreshContacts: async () => {
      const contacts = await api.listContacts().catch(() => null);
      if (!contacts) return;
      const contactsById: Record<string, ContactDto> = {};
      for (const c of contacts) contactsById[c.id] = c;
      set({ contacts, contactsById });
    },

    applyEvent: (ev) => {
      switch (ev.type) {
        case "offer_received":
          set((s) => {
            // Supersede any older offer from the same sender for the same file —
            // exactly as the daemon does. A sender that is still retrying re-posts
            // its offer and retracts the previous one, so the id churns: keeping the
            // old row leaves a button wired to an id the daemon has already dropped
            // ("no such pending offer"), which is precisely how Accetta came to do
            // nothing at all.
            const kept: Record<string, UITransfer> = {};
            for (const [k, tx] of Object.entries(s.transfers)) {
              const stale =
                tx.status === "incoming" &&
                tx.peerId === ev.from &&
                tx.name === ev.name &&
                tx.offerId !== ev.id;
              if (!stale) kept[k] = tx;
            }
            return {
              transfers: {
                ...kept,
                [`o${ev.id}`]: offerToUI(
                  {
                    id: ev.id,
                    from: ev.from,
                    name: ev.name,
                    size: ev.size,
                    note: ev.note,
                    sender_name: ev.sender_name,
                  },
                  s.transfers[`o${ev.id}`]
                ),
              },
            };
          });
          break;
        case "started":
          patch(ev.id, (tx) => ({
            ...tx,
            dir: ev.direction === "send" ? "out" : "in",
            name: ev.name,
            size: ev.total_size,
            status: "active",
          }));
          break;
        case "progress":
          patch(ev.id, (tx) => ({
            ...tx,
            transferred: ev.transferred,
            size: ev.total_size || tx.size,
            rate: sampleRate(ev.id, ev.transferred, tx.rate),
            // Bytes are moving, so the row is live — flip it back from "paused"
            // or "stalled" (the daemon's "waiting") on resume. Only a *terminal* status
            // is left alone: a late straggler event must not un-finish a done row.
            status:
              tx.status === "completed" ||
              tx.status === "cancelled" ||
              tx.status === "failed"
                ? tx.status
                : "active",
          }));
          break;
        case "completed":
          patch(ev.id, (tx) => ({
            ...tx,
            status: "completed",
            transferred: tx.size || tx.transferred,
            path: ev.path ?? tx.path,
          }));
          break;
        case "deposited":
          patch(ev.id, (tx) => ({ ...tx, status: "deposited" }));
          break;
        case "waiting":
          patch(ev.id, (tx) => ({ ...tx, status: "stalled", reason: ev.reason }));
          break;
        case "paused":
          patch(ev.id, (tx) => ({ ...tx, status: "paused", reason: ev.reason }));
          break;
        case "failed":
          patch(ev.id, (tx) => ({ ...tx, status: "failed", reason: ev.error }));
          break;
        case "cancelled":
          patch(ev.id, (tx) => ({ ...tx, status: "cancelled" }));
          break;
        case "code_ready":
          patch(ev.id, (tx) => ({ ...tx, code: ev.code }));
          break;
        case "code_paired":
          // Someone holds the ticket now; the code may retire (one-shot) or stay
          // (keep) — the daemon says which with `code_closed`, so nothing to do.
          break;
        case "code_closed":
          patch(ev.id, (tx) => ({ ...tx, code: undefined }));
          break;
        case "contacts_changed":
          // Fired by the daemon whoever wrote the book — typically an
          // `arvolo contacts …` run in another process.
          void get().refreshContacts();
          break;
        case "finished_cleared":
          // Somebody else cleared the list — `arvolo status clear`, or another
          // window. Without this the board keeps drawing rows the daemon has
          // already forgotten, and goes on offering to clear them.
          set(dropFinished);
          break;

        // Pairing runs as a session, not a request/reply: these three events are
        // the only way its progress reaches the UI. Each is ignored unless it
        // names the session currently on screen, so a stale outcome from a sheet
        // the user already closed cannot reopen it or overwrite a newer attempt.
        case "pairing_code":
        case "pairing_done":
        case "pairing_failed": {
          const p = get().pairing;
          if (p && p.session === null) {
            // The handle has not come back yet — hold it, don't drop it.
            earlyPairingEvents.push(ev);
            break;
          }
          if (!p || p.session !== ev.session) break;
          if (ev.type === "pairing_code") {
            set({ pairing: { ...p, code: ev.code, phase: "waiting" } });
          } else if (ev.type === "pairing_done") {
            set({
              pairing: {
                ...p,
                phase: "done",
                message: ev.summary,
                needsRestart: ev.needs_restart,
              },
            });
            void get().refreshContacts();
          } else {
            set({
              pairing: {
                ...p,
                phase: "failed",
                // A cancellation is the user's own doing; saying so keeps the
                // sheet from reporting their click back to them as an error.
                message: ev.cancelled ? t("pair.cancelled") : ev.error,
              },
            });
          }
          break;
        }
      }
    },

    setSearch: (q) => set({ search: q }),

    // Arriving *is* the refresh for the three screens with no push event behind
    // them. Doing it here rather than in each view keeps a stale panel from ever
    // being what greets the user, however they navigated to it — rail, command
    // palette or keyboard shortcut.
    go: (r) => {
      // Errors are cleared on the way *out* as well as refetched on the way in:
      // a failure from a previous visit must not be what greets the user when
      // they come back, before the fresh fetch has had a chance to succeed.
      set({
        route: r,
        paletteOpen: false,
        personOpen: null,
        depositsError: null,
        historyError: null,
        configError: null,
        syncError: null,
      });
      if (r === "deposits") void get().loadDeposits();
      if (r === "history") void get().loadHistory();
      if (r === "devices") void get().loadSync();
      if (r === "settings") void get().loadConfig();
    },
    setTheme: (choice) => {
      applyTheme(choice);
      try {
        localStorage.setItem(THEME_KEY, choice);
      } catch {
        // See `readTheme` — a webview that refuses storage still gets the theme,
        // it just will not remember it next launch.
      }
      set({ theme: choice });
    },
    setPaletteOpen: (v) => set({ paletteOpen: v }),
    openSheet: (paths, to, mode) =>
      set({
        sheetPaths: paths,
        sheetTo: to ?? null,
        sheetMode: mode ?? null,
        incomingOfferId: null,
      }),
    closeSheet: () =>
      set({ sheetPaths: null, sheetTo: null, sheetMode: null }),
    openIncoming: (offerId) => set({ incomingOfferId: offerId }),
    closeIncoming: () => set({ incomingOfferId: null }),
    openShare: (id) => set({ shareOpen: id }),
    closeShare: () => set({ shareOpen: null }),
    openPerson: (name) => set({ personOpen: name }),
    openReceive: () => set({ receiveOpen: true }),
    closeReceive: () => set({ receiveOpen: false }),

    loadHistory: async () => {
      set({ historyLoading: true });
      try {
        const history = await api.listHistory();
        set({ history, historyError: null, historyLoading: false });
      } catch (e) {
        // Keep what we last had and say why it may be stale — an empty panel
        // under a green "Connected" would read as "nothing ever happened".
        set({
          historyLoading: false,
          historyError: t("store.loadHistory", String(e)),
        });
      }
    },
    clearHistory: async () => {
      await act(t("store.errClearHistory"), () => api.clearHistory());
      set({ history: [] });
    },

    loadDeposits: async () => {
      set({ depositsLoading: true });
      try {
        // The daemon asks each relay for the real state while building this, so it
        // is slower than the other snapshots — hence the explicit loading flag.
        const deposits = await api.listDeposits();
        set({ deposits, depositsError: null, depositsLoading: false });
      } catch (e) {
        // Keep whatever we last had, and say why it may be wrong. An empty panel
        // under a green "Connected" would read as "you have no links" — the same lie
        // the board's `loadError` exists to prevent.
        set({
          depositsLoading: false,
          depositsError: t("store.loadDeposits", String(e)),
        });
      }
    },

    revokeDeposit: async (id) => {
      if (get().revoking.includes(id)) return;
      set((s) => ({ revoking: [...s.revoking, id] }));
      try {
        // Drop the row only once the daemon confirms the relay let go. Removing it
        // optimistically would show a link as gone while it still serves the file —
        // the worst possible direction to be wrong in for a revoke.
        await act(t("store.errRevokeLink"), () => api.revokeDeposit(id));
        set((s) => ({ deposits: s.deposits.filter((d) => d.id !== id) }));
      } finally {
        set((s) => ({ revoking: s.revoking.filter((r) => r !== id) }));
      }
    },

    loadPresence: async () => {
      const ids = get().contacts.map((c) => c.id);
      if (!ids.length) return;
      // Now that this is polled, the callers overlap: returning to the window
      // fires both `focus` and `visibilitychange`, and the timer keeps ticking
      // underneath. One answer is being fetched — asking twice more would only
      // make the relay repeat itself.
      if (get().presenceLoading) return;
      set({ presenceLoading: true });
      try {
        const rows = await api.presence(ids);
        const presence: Record<string, boolean | null> = {};
        for (const r of rows) presence[r.id] = r.online;
        set({ presence, presenceLoading: false });
      } catch {
        // Keep whatever the last successful probe found, but stop claiming it is
        // current: an unreachable daemon must not make everyone read as away.
        set({ presence: {}, presenceLoading: false });
      }
    },

    loadConfig: async () => {
      set({ configLoading: true });
      try {
        const config = await api.getConfig();
        set({ config, configError: null, configLoading: false });
      } catch (e) {
        set({
          configLoading: false,
          configError: t("store.loadConfig", String(e)),
        });
      }
    },
    saveConfig: async (patch) => {
      // The daemon replies with the state its write produced, so what lands in
      // the store is what is actually on disk — not the optimistic echo of a
      // field, which would quietly disagree the moment a key was refused or
      // normalized (a bare relay host becomes a full https URL, for one).
      const config = await act(t("store.errSaveConfig"), () =>
        api.setConfig(patch)
      );
      set({ config, configError: null });
      // The display name is advertised inside every offer and is also shown in
      // the rail, which reads it off `status`.
      if (patch.display_name !== undefined) {
        const status = await api.status().catch(() => null);
        if (status) set({ status });
      }
    },

    loadSync: async () => {
      set({ syncLoading: true });
      try {
        const sync = await api.syncStatus();
        set({ sync, syncError: null, syncLoading: false });
      } catch (e) {
        set({
          syncLoading: false,
          syncError: t("store.loadSync", String(e)),
        });
      }
    },
    syncNow: async () => {
      set({ syncLoading: true });
      try {
        const sync = await api.syncNow();
        set({ sync, syncLoading: false, syncError: null });
        // The round's own failure comes back inside the summary rather than as a
        // rejection — the rest of the state is still worth showing.
        if (sync.last_error) {
          toast.bad(t("store.syncFailed"), sync.last_error);
        } else {
          toast.ok(
            t("store.syncOk"),
            sync.last_merged
              ? t("store.syncMerged", sync.last_merged)
              : t("store.syncNone")
          );
          await get().refreshContacts();
        }
      } catch (e) {
        set({ syncLoading: false });
        toast.bad(t("store.syncFailed"), String(e));
      }
    },
    pruneNames: async () => {
      const n = await act(t("store.errPruneNames"), () => api.pruneNames());
      await get().refreshContacts();
      return n;
    },

    startPairing: async (kind, code, name) => {
      // Starting a second exchange must retire the first. The hosting side has
      // no deadline of its own — it waits until cancelled — so an orphaned
      // session would keep its rendezvous slot, and in the device case would
      // keep offering this machine's identity secret, with the handle needed to
      // stop it no longer reachable from anywhere in the UI.
      const running = get().pairing?.session;
      if (running) await api.cancelPairing(running).catch(() => {});

      set({
        pairing: {
          session: null,
          kind,
          code: code ?? "",
          phase: "starting",
          message: "",
          needsRestart: false,
        },
      });
      // Joining needs a code, and the code is what the user is about to type.
      // Opening the sheet is the whole action here; contacting the daemon now
      // would spawn a session that fails immediately ("a pairing code is
      // required"), stamp a session id, and replace the input the user was
      // meant to type into with a spinner.
      if ((kind === "contact_join" || kind === "device_join") && !code) return;
      try {
        const session = await api.startPairing(
          kind,
          null,
          code ?? null,
          name ?? null
        );
        set((s) =>
          s.pairing
            ? { pairing: { ...s.pairing, session, phase: "waiting" } }
            : {}
        );
        // The daemon spawns the session and *then* writes its reply, so a
        // fast-failing exchange (no relay configured, address book too large)
        // can emit its outcome before this line ran — with the handle still
        // null, the event handlers would have dropped it and the sheet would
        // wait on a spinner for ever.
        const early = earlyPairingEvents.splice(0);
        for (const ev of early) get().applyEvent(ev);
      } catch (e) {
        set((s) =>
          s.pairing
            ? {
                pairing: { ...s.pairing, phase: "failed", message: String(e) },
              }
            : {}
        );
      }
    },
    cancelPairing: async () => {
      const session = get().pairing?.session;
      set({ pairing: null });
      // Best-effort: the session finishing and the sheet closing race by nature,
      // and the user's intent is satisfied either way. Surfacing "no such
      // session" for closing a panel would be noise.
      if (session) await api.cancelPairing(session).catch(() => {});
    },
    clearPairing: () => set({ pairing: null }),

    send: async (to, paths, note) => {
      const id = await act(t("store.errSend", to), () =>
        api.sendTo(to, paths, note)
      );
      set({ sheetPaths: null, sheetTo: null, sheetMode: null });
      return id;
    },
    deposit: async (to, paths, note, ttl, max, password) => {
      // Unlike `send`, this does NOT close the send sheet. A deposit hands back
      // an `arvm…` ticket — the sender's own copy, for when the inbox route is
      // not wanted or is not working — and closing the panel on success would
      // destroy it the instant it was produced. The panel closes when the user
      // says so, exactly as it does for a code, a link or a ticket.
      const r = await act(t("store.errDeposit", to), () =>
        api.depositTo(to, paths, note, ttl, max, password)
      );
      refreshDeposits();
      return r;
    },
    ticket: async (paths) =>
      act(t("store.errTicket"), () => api.serveTicket(paths, null)),
    code: async (paths, keep) =>
      act(t("store.errCode"), () => api.serveCode(paths, null, keep)),
    link: async (path, ttl, max) => {
      const url = await act(t("store.errLink"), () =>
        api.createLink(path, ttl, max)
      );
      refreshDeposits();
      return url;
    },
    receive: async (ticket, out, password) => {
      const id = await act(t("store.errReceive"), () =>
        api.recv(ticket, out, password)
      );
      set({ receiveOpen: false });
      return id;
    },

    accept: async (offerId, out, password) => {
      await act(t("store.errAccept"), () =>
        api.acceptOffer(offerId, out, password ?? null)
      );
      set((s) => {
        const { [`o${offerId}`]: _drop, ...rest } = s.transfers;
        return { transfers: rest, incomingOfferId: null };
      });
    },
    reject: async (offerId) => {
      await act(t("store.errReject"), () => api.rejectOffer(offerId));
      set((s) => {
        const { [`o${offerId}`]: _drop, ...rest } = s.transfers;
        return { transfers: rest, incomingOfferId: null };
      });
    },
    pause: async (id) => {
      await act(t("store.errPause"), () => api.pause(id));
    },
    resume: async (id) => {
      await act(t("store.errResume"), () => api.resume(id));
    },
    cancel: async (id) => {
      // Show the click landed. The daemon only flips the row once it has actually
      // torn the transfer down — for a deposit that means withdrawing the blob from
      // the relay first — so without this the button looks dead for as long as that
      // takes. The engine's `cancelled` event overwrites this with the real status.
      const key = `t${id}`;
      const before = get().transfers[key]?.status;
      const setStatus = (status: UIStatus) =>
        set((s) =>
          s.transfers[key]
            ? { transfers: { ...s.transfers, [key]: { ...s.transfers[key], status } } }
            : {}
        );
      setStatus("cancelling");
      try {
        await act(t("store.errCancel"), () => api.cancel(id));
      } catch (e) {
        // The daemon refused: put the row back as it was rather than leave it
        // stuck pretending to cancel.
        if (before) setStatus(before);
        throw e;
      }
    },
    removeRow: async (key) => {
      const tx = get().transfers[key];
      if (!tx) return;
      // Only drop the row locally once the daemon confirms it dropped it too —
      // swallowing a refusal would show an empty list while the transfer lives on.
      if (tx.id > 0) await act(t("store.errRemove"), () => api.remove(tx.id));
      set((s) => {
        const { [key]: _drop, ...rest } = s.transfers;
        return { transfers: rest };
      });
    },
    markVerified: async (name) => {
      await act(t("store.errVerify", name), () => api.markVerified(name));
      // Refresh contacts and re-stamp the verified badge on every row.
      await get().reload();
    },
    markUnverified: async (name) => {
      await act(t("store.errUnverify", name), () =>
        api.markUnverified(name)
      );
      await get().reload();
    },
    markTrusted: async (who, force) => {
      await act(t("store.errTrust", who), () => api.markTrusted(who, force));
      await get().refreshContacts();
    },
    markUntrusted: async (who) => {
      await act(t("store.errUntrust", who), () =>
        api.markUntrusted(who)
      );
      await get().refreshContacts();
    },
    blockContact: async (who) => {
      await act(t("store.errBlock", who), () => api.blockContact(who));
      await get().refreshContacts();
    },
    unblockContact: async (who) => {
      await act(t("store.errUnblock", who), () => api.unblockContact(who));
      await get().refreshContacts();
    },
    acceptName: async (who) => {
      await act(t("store.errAcceptName", who), () =>
        api.acceptName(who)
      );
      await get().refreshContacts();
    },
    addContact: async (name, id) => {
      await act(t("store.errAddContact", name), () => api.addContact(name, id));
      await get().refreshContacts();
    },
    removeContact: async (name) => {
      await act(t("store.errRemoveContact", name), () => api.removeContact(name));
      await get().refreshContacts();
    },
    renameContact: async (old, newName) => {
      await act(t("store.errRenameContact", old), () =>
        api.renameContact(old, newName)
      );
      await get().refreshContacts();
    },
    setMyName: async (name) => {
      await act(t("store.errSetMyName"), () => api.setMyName(name));
      // The name lives in StatusDto — refetch so the header shows the new one.
      const status = await api.status().catch(() => null);
      if (status) set({ status });
    },
    restartDaemon: async () => {
      await act(t("store.errRestartDaemon"), () => api.restartDaemon());
      // The event pump notices the drop and respawns; the connected heartbeat
      // and the seed-retry loop take it from there.
    },
    reorderItems: (keys) =>
      set((s) => {
        const rows = keys.map((k) => s.transfers[k]).filter(Boolean);
        if (rows.length < 2) return {};
        // The list keeps the slots it already occupies and only the occupants
        // change places. That is what lets a section be reordered on its own:
        // rows in the other sections — and in the other column — hold ranks
        // outside this set and never move relative to it.
        const slots = rows.map((r) => r.rank).sort((a, b) => b - a);
        if (rows.every((r, i) => r.rank === slots[i])) return {};
        const transfers = { ...s.transfers };
        rows.forEach((r, i) => {
          transfers[r.key] = { ...r, rank: slots[i] };
        });
        // Persisted here and nowhere else: a reorder is the only moment the order
        // is something the *user* said rather than the order rows happened to
        // arrive in, and it is the only one worth carrying to the next run.
        saveOrder(transfers);
        return { transfers };
      }),
    togglePauseAll: async () => {
      const rows = Object.values(get().transfers);
      if (!get().pauseAll) {
        await Promise.all(
          rows
            .filter((tx) => tx.status === "active")
            .map((tx) => api.pause(tx.id).catch(() => {}))
        );
        set({ pauseAll: true });
      } else {
        await Promise.all(
          rows
            .filter((tx) => tx.status === "paused")
            .map((tx) => api.resume(tx.id).catch(() => {}))
        );
        set({ pauseAll: false });
      }
    },
    dismissActionError: () => set({ actionError: null }),
    clearFinished: async () => {
      // Daemon first: a local-only clear would just see every row come back with
      // the next snapshot. Its ClearFinished applies the same definition of
      // "finished" (a deposit awaiting pickup is NOT finished — still cancellable).
      await act(t("store.errClearFinished"), () => api.clearFinished());
      set(dropFinished);
    },
  };
});
