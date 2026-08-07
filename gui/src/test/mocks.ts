// A stand-in for the Tauri bridge, so the store can be driven without a daemon.
//
// The store is the GUI's whole brain: every row the user sees is derived here from
// a daemon snapshot plus pushed engine events. Faking the bridge lets us assert on
// that derivation directly — which is where the real bugs lived (a board that
// stayed empty, rows that claimed a transfer was gone when the daemon still had it).

import { vi } from "vitest";
import { normalizeEvent, type WireEvent } from "../events";
import type {
  ConfigDto,
  ConfigPatch,
  ContactDto,
  DepositDto,
  EngineEvent,
  HistoryDto,
  OfferDto,
  PairKind,
  StatusDto,
  SyncDto,
  TransferDto,
} from "../types";

/** Calls the store made, so a test can assert the daemon was actually told. */
export interface Recorder {
  cancel: number[];
  remove: number[];
  pause: number[];
  resume: number[];
  accept: [string, string | null, string | null][];
  reject: string[];
  markVerified: string[];
  sendTo: [string, string[], string][];
  revokeDeposit: string[];
  /** Counts fetches, so a test can prove the panel refreshes rather than trusting
   *  a list it fetched once and never again. */
  listDeposits: number;
  recv: [string, string | null, string | null][];
  serveCode: [string[], boolean][];
  createLink: [number | null, number | null][];
  addContact: [string, string][];
  removeContact: string[];
  renameContact: [string, string][];
  markTrusted: [string, boolean][];
  markUntrusted: string[];
  block: string[];
  unblock: string[];
  acceptName: string[];
  setMyName: string[];
  clearFinished: number;
  clearHistory: number;
  listHistory: number;
  depositTo: [string, string[], number | null, number | null, string | null][];
  setConfig: ConfigPatch[];
  pruneNames: number;
  syncNow: number;
  startPairing: [PairKind, string | null, string | null][];
  cancelPairing: string[];
}

export interface Harness {
  recorder: Recorder;
  /** Push an engine event **in its wire shape**, exactly as the Rust pump sends it.
   *  Tests speak the real contract: a mock that invented `{ type, ... }` is how the
   *  externally tagged wire format went unnoticed while every event was dropped. */
  emit: (wire: WireEvent) => void;
  /** Flip the connected heartbeat. */
  setConnected: (c: boolean) => void;
  /** What `reload()` will find on the daemon. */
  snapshot: {
    status: StatusDto | null;
    transfers: TransferDto[];
    pending: OfferDto[];
    contacts: ContactDto[];
    deposits: DepositDto[];
    history: HistoryDto[];
    config: ConfigDto;
    sync: SyncDto;
    /** Who the relay reports as online, by id. Absent = "could not ask". */
    presence: Record<string, boolean | null>;
  };
  /** Make the next call to these commands reject. */
  fail: Set<string>;
}

// Built fresh each time rather than written out twice: the initial value and the
// per-test reset must not be able to drift apart. A field present in one and
// missing from the other leaks state between tests, which is the kind of bug that
// shows up as an unrelated test failing later.
function freshRecorder(): Recorder {
  return {
    cancel: [],
    remove: [],
    pause: [],
    resume: [],
    accept: [],
    reject: [],
    markVerified: [],
    sendTo: [],
    revokeDeposit: [],
    listDeposits: 0,
    recv: [],
    serveCode: [],
    createLink: [],
    addContact: [],
    removeContact: [],
    renameContact: [],
    markTrusted: [],
    markUntrusted: [],
    block: [],
    unblock: [],
    acceptName: [],
    setMyName: [],
    clearFinished: 0,
    clearHistory: 0,
    listHistory: 0,
    depositTo: [],
    setConfig: [],
    pruneNames: 0,
    syncNow: 0,
    startPairing: [],
    cancelPairing: [],
  };
}

function freshSnapshot(): Harness["snapshot"] {
  return {
    status: {
      version: "0.9.2",
      public_id: "if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha",
      fingerprint: "able-otter-nine",
      relay: "https://relay.test",
      transfers: 0,
      pending: 0,
      download_dir: "/Users/ls/Arvolo",
      display_name: "",
    },
    transfers: [],
    pending: [],
    contacts: [],
    deposits: [],
    history: [],
    config: {
      relay: "https://relay.test",
      relay_configured: "relay.test",
      relay_source: "config",
      download_dir: "/Users/ls/Arvolo",
      download_dir_configured: "",
      download_dir_from_env: false,
      display_name: "",
      sync: true,
      seed: null,
      swarm: "",
      concurrency: null,
      config_path: "/Users/ls/.config/arvolo/config.toml",
      identity_path: "/Users/ls/.config/arvolo/identity.key",
    },
    presence: {},
    sync: {
      fingerprint: "able-otter-nine",
      public_id: "if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha",
      contacts: 0,
      enabled: true,
      last_sync: 0,
      last_merged: 0,
      last_error: "",
    },
  };
}

export const harness: Harness = {
  recorder: freshRecorder(),
  emit: () => {},
  setConnected: () => {},
  snapshot: freshSnapshot(),
  fail: new Set(),
};

export function resetHarness() {
  harness.recorder = freshRecorder();
  harness.snapshot = freshSnapshot();
  harness.fail = new Set();
}

function guard<T>(name: string, value: T): Promise<T> {
  if (harness.fail.has(name)) {
    return Promise.reject(new Error(`daemon refused: ${name}`));
  }
  return Promise.resolve(value);
}

/** Install the mock. Must be called from a `vi.mock` factory for "../ipc". */
export function makeIpcMock() {
  return {
    api: {
      status: () => guard("status", harness.snapshot.status),
      listTransfers: () => guard("listTransfers", harness.snapshot.transfers),
      listPending: () => guard("listPending", harness.snapshot.pending),
      listContacts: () => guard("listContacts", harness.snapshot.contacts),
      guiVersion: () => guard("guiVersion", "0.9.2"),
      sendTo: (to: string, paths: string[], note: string) => {
        harness.recorder.sendTo.push([to, paths, note]);
        return guard("sendTo", 1);
      },
      serveTicket: () => guard("serveTicket", { id: 1, ticket: "arvc-test" }),
      depositTo: (
        to: string,
        paths: string[],
        _note: string,
        ttl: number | null,
        max: number | null,
        password: string | null
      ) => {
        harness.recorder.depositTo.push([to, paths, ttl, max, password]);
        return guard("depositTo", { id: 2, ticket: "arvm-test" });
      },
      getConfig: () => guard("getConfig", harness.snapshot.config),
      setConfig: (patch: ConfigPatch) => {
        harness.recorder.setConfig.push(patch);
        return guard("setConfig", harness.snapshot.config);
      },
      presence: (ids: string[]) =>
        guard(
          "presence",
          ids.map((id) => ({ id, online: harness.snapshot.presence[id] ?? null }))
        ),
      pruneNames: () => {
        harness.recorder.pruneNames++;
        return guard("pruneNames", 0);
      },
      syncStatus: () => guard("syncStatus", harness.snapshot.sync),
      syncNow: () => {
        harness.recorder.syncNow++;
        return guard("syncNow", harness.snapshot.sync);
      },
      startPairing: (
        kind: PairKind,
        relay: string | null,
        code: string | null,
        _name: string | null
      ) => {
        harness.recorder.startPairing.push([kind, relay, code]);
        return guard("startPairing", "pair-1");
      },
      cancelPairing: (session: string) => {
        harness.recorder.cancelPairing.push(session);
        return guard("cancelPairing", undefined);
      },
      readTextFile: () => guard("readTextFile", "[]"),
      writeTextFile: () => guard("writeTextFile", undefined),
      serveCode: (paths: string[], _relay: string | null, keep: boolean) => {
        harness.recorder.serveCode.push([paths, keep]);
        return guard("serveCode", { id: 1, code: "4821-crater-mango" });
      },
      createLink: (_path: string, ttl: number | null, max: number | null) => {
        harness.recorder.createLink.push([ttl, max]);
        return guard("createLink", "https://relay.test/dl/abc#key");
      },
      recv: (ticket: string, out: string | null, password: string | null) => {
        harness.recorder.recv.push([ticket, out, password]);
        return guard("recv", 9);
      },
      acceptOffer: (
        offerId: string,
        out: string | null,
        password?: string | null
      ) => {
        harness.recorder.accept.push([offerId, out, password ?? null]);
        // A deposit sealed with a password refuses the plain accept and *keeps*
        // the offer, so the UI can ask and try again. Mirrors the engine's
        // pre-flight check.
        if (harness.fail.has("acceptOffer:password") && !password) {
          return Promise.reject(
            new Error("this deposit is password-protected — supply the password")
          );
        }
        return guard("acceptOffer", 7);
      },
      rejectOffer: (offerId: string) => {
        harness.recorder.reject.push(offerId);
        return guard("rejectOffer", undefined);
      },
      pause: (id: number) => {
        harness.recorder.pause.push(id);
        return guard("pause", undefined);
      },
      resume: (id: number) => {
        harness.recorder.resume.push(id);
        return guard("resume", undefined);
      },
      cancel: (id: number) => {
        harness.recorder.cancel.push(id);
        return guard("cancel", undefined);
      },
      remove: (id: number) => {
        harness.recorder.remove.push(id);
        return guard("remove", undefined);
      },
      markVerified: (name: string) => {
        harness.recorder.markVerified.push(name);
        return guard("markVerified", undefined);
      },
      markUnverified: (name: string) => guard("markUnverified", void name),
      markTrusted: (who: string, force: boolean) => {
        harness.recorder.markTrusted.push([who, force]);
        return guard("markTrusted", undefined);
      },
      markUntrusted: (who: string) => {
        harness.recorder.markUntrusted.push(who);
        return guard("markUntrusted", undefined);
      },
      blockContact: (who: string) => {
        harness.recorder.block.push(who);
        return guard("blockContact", undefined);
      },
      unblockContact: (who: string) => {
        harness.recorder.unblock.push(who);
        return guard("unblockContact", undefined);
      },
      acceptName: (who: string) => {
        harness.recorder.acceptName.push(who);
        return guard("acceptName", undefined);
      },
      addContact: (name: string, id: string) => {
        harness.recorder.addContact.push([name, id]);
        return guard("addContact", undefined);
      },
      removeContact: (name: string) => {
        harness.recorder.removeContact.push(name);
        return guard("removeContact", undefined);
      },
      renameContact: (old: string, newName: string) => {
        harness.recorder.renameContact.push([old, newName]);
        return guard("renameContact", undefined);
      },
      listHistory: () => {
        harness.recorder.listHistory++;
        return guard("listHistory", harness.snapshot.history);
      },
      clearHistory: () => {
        harness.recorder.clearHistory++;
        return guard("clearHistory", 0);
      },
      clearFinished: () => {
        harness.recorder.clearFinished++;
        return guard("clearFinished", 0);
      },
      setMyName: (name: string) => {
        harness.recorder.setMyName.push(name);
        return guard("setMyName", undefined);
      },
      restartDaemon: () => guard("restartDaemon", undefined),
      listDeposits: () => {
        harness.recorder.listDeposits++;
        return guard("listDeposits", harness.snapshot.deposits);
      },
      revokeDeposit: (id: string) => {
        harness.recorder.revokeDeposit.push(id);
        return guard("revokeDeposit", undefined);
      },
    },
    onEngineEvent: (cb: (ev: EngineEvent) => void) => {
      // Mirror the real subscriber exactly: wire in, normalizer, app model out.
      harness.emit = (wire: WireEvent) => {
        const ev = normalizeEvent(wire);
        if (ev) cb(ev);
      };
      return Promise.resolve(() => {});
    },
    onConnected: (cb: (c: boolean) => void) => {
      harness.setConnected = cb;
      return Promise.resolve(() => {});
    },
  };
}

export const dto = {
  transfer(over: Partial<TransferDto> = {}): TransferDto {
    return {
      id: 1,
      direction: "send",
      peer: "peer1",
      name: "file.txt",
      total_size: 100,
      transferred: 0,
      status: "active",
      swarm_peers: 0,
      pieces_from_peers: 0,
      download_peers: 0,
      created: Math.floor(Date.now() / 1000),
      ...over,
    };
  },
  offer(over: Partial<OfferDto> = {}): OfferDto {
    return {
      id: "off1",
      from: "peer1",
      name: "incoming.pdf",
      size: 500,
      note: "",
      sender_name: "",
      ...over,
    };
  },
  contact(over: Partial<ContactDto> = {}): ContactDto {
    return {
      name: "proj",
      id: "peer1",
      fingerprint: "able-otter-nine",
      verified: false,
      trusted: false,
      blocked: false,
      display_name: "",
      pending_name: "",
      ...over,
    };
  },
  /** A live public link, as a reachable relay reports it. */
  deposit(over: Partial<DepositDto> = {}): DepositDto {
    return {
      id: "a1b2c3d4",
      kind: "link",
      name: "photo.jpg",
      size: 4242,
      link: "https://relay.test/dl/claim1#key",
      recipient: "",
      created: Math.floor(Date.now() / 1000),
      expires: Math.floor(Date.now() / 1000) + 7 * 86400,
      expired: false,
      max_label: "unlimited",
      present: true,
      downloads: 0,
      max_downloads: null,
      ...over,
    };
  },
};

export const vitestNoop = vi.fn;
