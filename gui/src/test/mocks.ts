// A stand-in for the Tauri bridge, so the store can be driven without a daemon.
//
// The store is the GUI's whole brain: every row the user sees is derived here from
// a daemon snapshot plus pushed engine events. Faking the bridge lets us assert on
// that derivation directly — which is where the real bugs lived (a board that
// stayed empty, rows that claimed a transfer was gone when the daemon still had it).

import { vi } from "vitest";
import { normalizeEvent, type WireEvent } from "../events";
import type {
  ContactDto,
  EngineEvent,
  OfferDto,
  StatusDto,
  TransferDto,
} from "../types";

/** Calls the store made, so a test can assert the daemon was actually told. */
export interface Recorder {
  cancel: number[];
  remove: number[];
  pause: number[];
  resume: number[];
  accept: [string, string | null][];
  reject: string[];
  markVerified: string[];
  sendTo: [string, string[], string][];
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
  };
  /** Make the next call to these commands reject. */
  fail: Set<string>;
}

export const harness: Harness = {
  recorder: {
    cancel: [],
    remove: [],
    pause: [],
    resume: [],
    accept: [],
    reject: [],
    markVerified: [],
    sendTo: [],
  },
  emit: () => {},
  setConnected: () => {},
  snapshot: {
    status: {
      version: "0.9.2",
      public_id: "if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha",
      fingerprint: "able-otter-nine",
      relay: "https://relay.test",
      transfers: 0,
      pending: 0,
      download_dir: "/Users/ls/Arvolo",
    },
    transfers: [],
    pending: [],
    contacts: [],
  },
  fail: new Set(),
};

export function resetHarness() {
  harness.recorder = {
    cancel: [],
    remove: [],
    pause: [],
    resume: [],
    accept: [],
    reject: [],
    markVerified: [],
    sendTo: [],
  };
  harness.snapshot = {
    status: {
      version: "0.9.2",
      public_id: "if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha",
      fingerprint: "able-otter-nine",
      relay: "https://relay.test",
      transfers: 0,
      pending: 0,
      download_dir: "/Users/ls/Arvolo",
    },
    transfers: [],
    pending: [],
    contacts: [],
  };
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
      createLink: () => guard("createLink", "https://relay.test/dl/abc#key"),
      acceptOffer: (offerId: string, out: string | null) => {
        harness.recorder.accept.push([offerId, out]);
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
      ...over,
    };
  },
};

export const vitestNoop = vi.fn;
