// The single source of truth for the board: a Zustand store seeded from a daemon
// snapshot and then mutated purely by pushed engine events (no polling).

import { create } from "zustand";
import { api, onConnected, onEngineEvent } from "./ipc";
import { shortId } from "./format";
import type {
  ContactDto,
  DepositDto,
  EngineEvent,
  HistoryDto,
  Method,
  OfferDto,
  StatusDto,
  TransferDto,
  UIStatus,
  UITransfer,
} from "./types";

function now(): number {
  return Date.now();
}

/** Strictly increasing list position. `Date.now()` is not usable here: two rows
 *  created in the same millisecond would tie, leaving their order ambiguous and
 *  making "Sposta su/giù" swap two equal ranks — a move that changes nothing. */
let rankSeq = 0;
function nextRank(): number {
  return ++rankSeq;
}

/** Last progress sample per transfer id, for throughput estimation. Kept outside
 *  the store: it changes on every event and must not trigger renders itself. */
const progSamples = new Map<number, { t: number; bytes: number }>();

/** Exponentially-smoothed bytes/sec from consecutive progress events. */
function sampleRate(id: number, bytes: number, prevRate?: number): number | undefined {
  const t = now();
  const last = progSamples.get(id);
  progSamples.set(id, { t, bytes });
  if (!last || t <= last.t || bytes < last.bytes) return prevRate;
  const inst = ((bytes - last.bytes) * 1000) / (t - last.t);
  return prevRate ? 0.7 * prevRate + 0.3 * inst : inst;
}

/** Split a daemon status string into a UI status + optional reason. */
function toUIStatus(raw: string): { status: UIStatus; reason?: string } {
  if (raw === "active") return { status: "in corso" };
  if (raw === "completed") return { status: "completato" };
  if (raw === "deposited") return { status: "deposited" };
  if (raw === "cancelled") return { status: "annullato" };
  const [head, ...rest] = raw.split(":");
  const reason = rest.join(":").trim() || undefined;
  if (head === "waiting") return { status: "in stallo", reason };
  if (head === "paused") return { status: "in attesa", reason };
  if (head === "failed") return { status: "fallito", reason };
  return { status: "in corso" };
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
   *  the board came to show "Connesso · 0 invii" while the daemon held two live
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
  depositsOpen: boolean;
  depositsLoading: boolean;
  depositsError: string | null;
  /** Ids currently being withdrawn, so a row can show the click landed and cannot
   *  be double-submitted (the relay round-trip is not instant). */
  revoking: string[];

  /** The history log, fetched when its panel opens (like the deposits: there is
   *  no push event for it, so open-is-the-refresh keeps it honest). */
  history: HistoryDto[];
  historyOpen: boolean;
  historyLoading: boolean;
  historyError: string | null;

  // UI state
  search: string;
  pauseAll: boolean;
  openMenuKey: string | null;
  sheetPaths: string[] | null; // send sheet open when non-null
  incomingOfferId: string | null; // incoming modal
  receiveOpen: boolean; // paste-a-ticket modal
  contactsOpen: boolean; // address book panel

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
  toggleMenu: (key: string | null) => void;
  openSheet: (paths: string[]) => void;
  closeSheet: () => void;
  openIncoming: (offerId: string) => void;
  closeIncoming: () => void;
  /** Open the deposits panel and fetch it. Opening *is* the refresh: there is no
   *  event to keep it live, so a stale list must never be what greets the user. */
  openDeposits: () => Promise<void>;
  closeDeposits: () => void;
  loadDeposits: () => Promise<void>;
  /** Withdraw a deposit from the relay and forget it. Irreversible: the link stops
   *  working for everyone who has it. */
  revokeDeposit: (id: string) => Promise<void>;

  openReceive: () => void;
  closeReceive: () => void;
  openContacts: () => void;
  closeContacts: () => void;
  openHistory: () => Promise<void>;
  closeHistory: () => void;
  loadHistory: () => Promise<void>;
  /** Forget the whole daemon-side history log. */
  clearHistory: () => Promise<void>;

  // actions (forward to the daemon, then let events reconcile)
  send: (to: string, paths: string[], note: string) => Promise<number>;
  ticket: (paths: string[]) => Promise<{ id: number; ticket: string }>;
  /** Host a short pairing code in the daemon (keep = serve every receiver). */
  code: (paths: string[], keep: boolean) => Promise<{ id: number; code: string }>;
  link: (path: string, ttl: number | null, max: number | null) => Promise<string>;
  /** Receive from a pasted arvc… ticket, pairing code or arvm… offline ticket. */
  receive: (ticket: string, out: string | null, password: string | null) => Promise<number>;
  accept: (offerId: string, out: string | null) => Promise<void>;
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
  /** "Sposta su/giù": swap the row's rank with its neighbour (same direction). */
  moveItem: (key: string, delta: 1 | -1) => void;
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

  const dtoToUI = (d: TransferDto, prev?: UITransfer): UITransfer => {
    const { status, reason } = toUIStatus(d.status);
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
      path: prev?.path,
      // The engine's own clock, not ours: a row we first see today may well be
      // yesterday's. `created` is 0 only from a daemon that predates the field —
      // then, and only then, fall back to when we noticed it.
      firstSeen: d.created > 0 ? d.created * 1000 : (prev?.firstSeen ?? now()),
      rank: prev?.rank ?? nextRank(),
      rate: prev?.rate,
      code: d.code ?? prev?.code,
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
    status: "in arrivo",
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
    rank: prev?.rank ?? nextRank(),
  });

  /** Merge a partial change into an existing transfer row (creating a stub if the
   *  row is unknown — e.g. an event arrived before its snapshot). */
  const patch = (id: number, fn: (t: UITransfer) => UITransfer) =>
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
          status: "in corso",
          encrypted: true,
          verified: false,
          method: "p2p",
          swarmPeers: 0,
          downloadPeers: 0,
          files: 1,
          firstSeen: now(),
          rank: nextRank(),
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
    search: "",
    pauseAll: false,
    openMenuKey: null,
    sheetPaths: null,
    incomingOfferId: null,
    receiveOpen: false,
    contactsOpen: false,
    deposits: [],
    depositsOpen: false,
    depositsLoading: false,
    depositsError: null,
    revoking: [],
    history: [],
    historyOpen: false,
    historyLoading: false,
    historyError: null,

    peerLabel: (id, fallbackName) => {
      if (!id) return fallbackName || "sconosciuto";
      const c = get().contactsById[id];
      if (c) return c.name;
      if (fallbackName) return fallbackName;
      return shortId(id);
    },
    isVerified: (id) => (id ? !!get().contactsById[id]?.verified : false),

    init: async () => {
      api
        .guiVersion()
        .then((v) => set({ guiVersion: v }))
        .catch(() => {});
      const unlistenEv = await onEngineEvent((ev) => get().applyEvent(ev));
      const unlistenConn = await onConnected((c) => {
        set({ connected: c });
        if (c) get().reload();
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
          loadError: `Non riesco a leggere i trasferimenti dal daemon: ${String(e)}`,
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
            for (const [k, t] of Object.entries(s.transfers)) {
              const stale =
                t.status === "in arrivo" &&
                t.peerId === ev.from &&
                t.name === ev.name &&
                t.offerId !== ev.id;
              if (!stale) kept[k] = t;
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
          patch(ev.id, (t) => ({
            ...t,
            dir: ev.direction === "send" ? "out" : "in",
            name: ev.name,
            size: ev.total_size,
            status: "in corso",
          }));
          break;
        case "progress":
          patch(ev.id, (t) => ({
            ...t,
            transferred: ev.transferred,
            size: ev.total_size || t.size,
            rate: sampleRate(ev.id, ev.transferred, t.rate),
            // Bytes are moving, so the row is active — flip it back from "in attesa"
            // (paused) or "in stallo" (waiting) on resume. Only a *terminal* status
            // is left alone: a late straggler event must not un-finish a done row.
            status:
              t.status === "completato" ||
              t.status === "annullato" ||
              t.status === "fallito"
                ? t.status
                : "in corso",
          }));
          break;
        case "completed":
          patch(ev.id, (t) => ({
            ...t,
            status: "completato",
            transferred: t.size || t.transferred,
            path: ev.path ?? t.path,
          }));
          break;
        case "deposited":
          patch(ev.id, (t) => ({ ...t, status: "deposited" }));
          break;
        case "waiting":
          patch(ev.id, (t) => ({ ...t, status: "in stallo", reason: ev.reason }));
          break;
        case "paused":
          patch(ev.id, (t) => ({ ...t, status: "in attesa", reason: ev.reason }));
          break;
        case "failed":
          patch(ev.id, (t) => ({ ...t, status: "fallito", reason: ev.error }));
          break;
        case "cancelled":
          patch(ev.id, (t) => ({ ...t, status: "annullato" }));
          break;
        case "code_ready":
          patch(ev.id, (t) => ({ ...t, code: ev.code }));
          break;
        case "code_paired":
          // Someone holds the ticket now; the code may retire (one-shot) or stay
          // (keep) — the daemon says which with `code_closed`, so nothing to do.
          break;
        case "code_closed":
          patch(ev.id, (t) => ({ ...t, code: undefined }));
          break;
        case "contacts_changed":
          // Fired by the daemon whoever wrote the book — typically an
          // `arvolo contacts …` run in another process.
          void get().refreshContacts();
          break;
      }
    },

    setSearch: (q) => set({ search: q }),
    toggleMenu: (key) =>
      set((s) => ({ openMenuKey: s.openMenuKey === key ? null : key })),
    openSheet: (paths) =>
      set({ sheetPaths: paths, openMenuKey: null, incomingOfferId: null }),
    closeSheet: () => set({ sheetPaths: null }),
    openIncoming: (offerId) =>
      set({ incomingOfferId: offerId, openMenuKey: null }),
    closeIncoming: () => set({ incomingOfferId: null }),
    openReceive: () => set({ receiveOpen: true, openMenuKey: null }),
    closeReceive: () => set({ receiveOpen: false }),
    // The three flags below are the app's *view*: at most one is on, and all off
    // means the board. The setters keep the exclusivity so the sidebar can treat
    // them as one selection — two panels claiming the main pane at once is a
    // state the UI cannot draw.
    openContacts: () =>
      set({
        contactsOpen: true,
        historyOpen: false,
        depositsOpen: false,
        openMenuKey: null,
      }),
    closeContacts: () => set({ contactsOpen: false }),

    openHistory: async () => {
      set({
        historyOpen: true,
        contactsOpen: false,
        depositsOpen: false,
        openMenuKey: null,
      });
      await get().loadHistory();
    },
    closeHistory: () => set({ historyOpen: false, historyError: null }),
    loadHistory: async () => {
      set({ historyLoading: true });
      try {
        const history = await api.listHistory();
        set({ history, historyError: null, historyLoading: false });
      } catch (e) {
        // Keep what we last had and say why it may be stale — an empty panel
        // under a green "Connesso" would read as "nothing ever happened".
        set({
          historyLoading: false,
          historyError: `Non riesco a leggere lo storico dal daemon: ${String(e)}`,
        });
      }
    },
    clearHistory: async () => {
      await act("Impossibile svuotare lo storico", () => api.clearHistory());
      set({ history: [] });
    },

    openDeposits: async () => {
      set({
        depositsOpen: true,
        historyOpen: false,
        contactsOpen: false,
        openMenuKey: null,
      });
      await get().loadDeposits();
    },
    closeDeposits: () => set({ depositsOpen: false, depositsError: null }),

    loadDeposits: async () => {
      set({ depositsLoading: true });
      try {
        // The daemon asks each relay for the real state while building this, so it
        // is slower than the other snapshots — hence the explicit loading flag.
        const deposits = await api.listDeposits();
        set({ deposits, depositsError: null, depositsLoading: false });
      } catch (e) {
        // Keep whatever we last had, and say why it may be wrong. An empty panel
        // under a green "Connesso" would read as "you have no links" — the same lie
        // the board's `loadError` exists to prevent.
        set({
          depositsLoading: false,
          depositsError: `Non riesco a leggere i link dal daemon: ${String(e)}`,
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
        await act("Impossibile revocare il link", () => api.revokeDeposit(id));
        set((s) => ({ deposits: s.deposits.filter((d) => d.id !== id) }));
      } finally {
        set((s) => ({ revoking: s.revoking.filter((r) => r !== id) }));
      }
    },

    send: async (to, paths, note) => {
      const id = await act(`Invio a ${to} non riuscito`, () =>
        api.sendTo(to, paths, note)
      );
      set({ sheetPaths: null });
      return id;
    },
    ticket: async (paths) =>
      act("Creazione del ticket non riuscita", () => api.serveTicket(paths, null)),
    code: async (paths, keep) =>
      act("Creazione del codice non riuscita", () => api.serveCode(paths, null, keep)),
    link: async (path, ttl, max) =>
      act("Creazione del link non riuscita", () => api.createLink(path, ttl, max)),
    receive: async (ticket, out, password) => {
      const id = await act("Ricezione non riuscita", () =>
        api.recv(ticket, out, password)
      );
      set({ receiveOpen: false });
      return id;
    },

    accept: async (offerId, out) => {
      await act("Impossibile accettare il file", () => api.acceptOffer(offerId, out));
      set((s) => {
        const { [`o${offerId}`]: _drop, ...rest } = s.transfers;
        return { transfers: rest, incomingOfferId: null };
      });
    },
    reject: async (offerId) => {
      await act("Impossibile rifiutare il file", () => api.rejectOffer(offerId));
      set((s) => {
        const { [`o${offerId}`]: _drop, ...rest } = s.transfers;
        return { transfers: rest, incomingOfferId: null };
      });
    },
    pause: async (id) => {
      await act("Impossibile mettere in pausa", () => api.pause(id));
      set({ openMenuKey: null });
    },
    resume: async (id) => {
      await act("Impossibile riprendere", () => api.resume(id));
      set({ openMenuKey: null });
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
      setStatus("in annullamento");
      set({ openMenuKey: null });
      try {
        await act("Impossibile annullare", () => api.cancel(id));
      } catch (e) {
        // The daemon refused: put the row back as it was rather than leave it
        // stuck pretending to cancel.
        if (before) setStatus(before);
        throw e;
      }
    },
    removeRow: async (key) => {
      const t = get().transfers[key];
      if (!t) return;
      // Only drop the row locally once the daemon confirms it dropped it too —
      // swallowing a refusal would show an empty list while the transfer lives on.
      if (t.id > 0) await act("Impossibile eliminare", () => api.remove(t.id));
      set((s) => {
        const { [key]: _drop, ...rest } = s.transfers;
        return { transfers: rest, openMenuKey: null };
      });
    },
    markVerified: async (name) => {
      await act(`Impossibile verificare ${name}`, () => api.markVerified(name));
      set({ openMenuKey: null });
      // Refresh contacts and re-stamp the verified badge on every row.
      await get().reload();
    },
    markUnverified: async (name) => {
      await act(`Impossibile togliere la verifica a ${name}`, () =>
        api.markUnverified(name)
      );
      await get().reload();
    },
    markTrusted: async (who, force) => {
      await act(`Impossibile fidarsi di ${who}`, () => api.markTrusted(who, force));
      await get().refreshContacts();
    },
    markUntrusted: async (who) => {
      await act(`Impossibile togliere la fiducia a ${who}`, () =>
        api.markUntrusted(who)
      );
      await get().refreshContacts();
    },
    blockContact: async (who) => {
      await act(`Impossibile bloccare ${who}`, () => api.blockContact(who));
      await get().refreshContacts();
    },
    unblockContact: async (who) => {
      await act(`Impossibile sbloccare ${who}`, () => api.unblockContact(who));
      await get().refreshContacts();
    },
    acceptName: async (who) => {
      await act(`Impossibile approvare il nome di ${who}`, () =>
        api.acceptName(who)
      );
      await get().refreshContacts();
    },
    addContact: async (name, id) => {
      await act(`Impossibile salvare ${name}`, () => api.addContact(name, id));
      await get().refreshContacts();
    },
    removeContact: async (name) => {
      await act(`Impossibile rimuovere ${name}`, () => api.removeContact(name));
      await get().refreshContacts();
    },
    renameContact: async (old, newName) => {
      await act(`Impossibile rinominare ${old}`, () =>
        api.renameContact(old, newName)
      );
      await get().refreshContacts();
    },
    setMyName: async (name) => {
      await act("Impossibile impostare il nome", () => api.setMyName(name));
      // The name lives in StatusDto — refetch so the header shows the new one.
      const status = await api.status().catch(() => null);
      if (status) set({ status });
    },
    restartDaemon: async () => {
      await act("Impossibile riavviare il daemon", () => api.restartDaemon());
      // The event pump notices the drop and respawns; the connected heartbeat
      // and the seed-retry loop take it from there.
    },
    moveItem: (key, delta) =>
      set((s) => {
        const me = s.transfers[key];
        if (!me) return { openMenuKey: null };
        // Neighbours = same direction, ordered exactly as the board renders
        // (rank descending). delta -1 = up (towards higher rank).
        const siblings = Object.values(s.transfers)
          .filter((t) => t.dir === me.dir)
          .sort((a, b) => b.rank - a.rank);
        const i = siblings.findIndex((t) => t.key === key);
        const j = i + delta;
        if (i < 0 || j < 0 || j >= siblings.length) return { openMenuKey: null };
        const other = siblings[j];
        return {
          openMenuKey: null,
          transfers: {
            ...s.transfers,
            [me.key]: { ...me, rank: other.rank },
            [other.key]: { ...other, rank: me.rank },
          },
        };
      }),
    togglePauseAll: async () => {
      const rows = Object.values(get().transfers);
      if (!get().pauseAll) {
        await Promise.all(
          rows
            .filter((t) => t.status === "in corso")
            .map((t) => api.pause(t.id).catch(() => {}))
        );
        set({ pauseAll: true, openMenuKey: null });
      } else {
        await Promise.all(
          rows
            .filter((t) => t.status === "in attesa")
            .map((t) => api.resume(t.id).catch(() => {}))
        );
        set({ pauseAll: false, openMenuKey: null });
      }
    },
    dismissActionError: () => set({ actionError: null }),
    clearFinished: async () => {
      // Daemon first: a local-only clear would just see every row come back with
      // the next snapshot. Its ClearFinished applies the same definition of
      // "finished" (a deposit awaiting pickup is NOT finished — still cancellable).
      await act("Impossibile pulire i completati", () => api.clearFinished());
      set((s) => {
        const kept: Record<string, UITransfer> = {};
        for (const [k, t] of Object.entries(s.transfers)) {
          const finished =
            t.status === "completato" ||
            t.status === "fallito" ||
            t.status === "annullato";
          if (!finished) kept[k] = t;
        }
        return { transfers: kept, openMenuKey: null };
      });
    },
  };
});
