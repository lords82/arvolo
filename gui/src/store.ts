// The single source of truth for the board: a Zustand store seeded from a daemon
// snapshot and then mutated purely by pushed engine events (no polling).

import { create } from "zustand";
import { api, onConnected, onEngineEvent } from "./ipc";
import { shortId } from "./format";
import type {
  ContactDto,
  EngineEvent,
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

  // UI state
  search: string;
  pauseAll: boolean;
  openMenuKey: string | null;
  sheetPaths: string[] | null; // send sheet open when non-null
  incomingOfferId: string | null; // incoming modal

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

  // actions (forward to the daemon, then let events reconcile)
  send: (to: string, paths: string[], note: string) => Promise<number>;
  ticket: (paths: string[]) => Promise<{ id: number; ticket: string }>;
  link: (path: string) => Promise<string>;
  accept: (offerId: string, out: string | null) => Promise<void>;
  reject: (offerId: string) => Promise<void>;
  pause: (id: number) => Promise<void>;
  resume: (id: number) => Promise<void>;
  cancel: (id: number) => Promise<void>;
  /** Per-row "Elimina": drop a finished transfer from daemon + local list. */
  removeRow: (key: string) => Promise<void>;
  /** Mark a saved contact verified, then refresh contacts + row badges. */
  markVerified: (name: string) => Promise<void>;
  /** "Sposta su/giù": swap the row's rank with its neighbour (same direction). */
  moveItem: (key: string, delta: 1 | -1) => void;
  togglePauseAll: () => Promise<void>;
  clearFinished: () => void;
}

export const useStore = create<State>((set, get) => {
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
      firstSeen: prev?.firstSeen ?? now(),
      rank: prev?.rank ?? nextRank(),
      rate: prev?.rate,
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
    contacts: [],
    contactsById: {},
    transfers: {},
    search: "",
    pauseAll: false,
    openMenuKey: null,
    sheetPaths: null,
    incomingOfferId: null,

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
          set((s) => ({
            transfers: {
              ...s.transfers,
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
          }));
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
            status: t.status === "in corso" ? "in corso" : t.status,
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

    send: async (to, paths, note) => {
      const id = await api.sendTo(to, paths, note);
      set({ sheetPaths: null });
      return id;
    },
    ticket: async (paths) => {
      const r = await api.serveTicket(paths, null);
      return r;
    },
    link: async (path) => api.createLink(path, null, null),

    accept: async (offerId, out) => {
      await api.acceptOffer(offerId, out);
      set((s) => {
        const { [`o${offerId}`]: _drop, ...rest } = s.transfers;
        return { transfers: rest, incomingOfferId: null };
      });
    },
    reject: async (offerId) => {
      await api.rejectOffer(offerId);
      set((s) => {
        const { [`o${offerId}`]: _drop, ...rest } = s.transfers;
        return { transfers: rest, incomingOfferId: null };
      });
    },
    pause: async (id) => {
      await api.pause(id);
      set({ openMenuKey: null });
    },
    resume: async (id) => {
      await api.resume(id);
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
        await api.cancel(id);
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
      if (t.id > 0) await api.remove(t.id);
      set((s) => {
        const { [key]: _drop, ...rest } = s.transfers;
        return { transfers: rest, openMenuKey: null };
      });
    },
    markVerified: async (name) => {
      await api.markVerified(name);
      set({ openMenuKey: null });
      // Refresh contacts and re-stamp the verified badge on every row.
      await get().reload();
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
    clearFinished: () =>
      set((s) => {
        const kept: Record<string, UITransfer> = {};
        for (const [k, t] of Object.entries(s.transfers)) {
          // A deposit awaiting pickup is NOT finished: the recipient has not taken
          // it, and the row is still cancellable (cancelling withdraws the file from
          // the relay). The daemon keeps it for the same reason, so dropping it here
          // would only make it reappear on the next snapshot.
          const finished =
            t.status === "completato" ||
            t.status === "fallito" ||
            t.status === "annullato";
          if (!finished) kept[k] = t;
        }
        return { transfers: kept, openMenuKey: null };
      }),
  };
});
