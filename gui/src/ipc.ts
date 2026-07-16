// Typed wrappers over the Tauri command bridge + the engine event channel.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { normalizeEvent, type WireEvent } from "./events";
import type {
  ContactDto,
  EngineEvent,
  OfferDto,
  StatusDto,
  TransferDto,
} from "./types";

export const api = {
  status: () => invoke<StatusDto>("status"),
  listTransfers: () => invoke<TransferDto[]>("list_transfers"),
  listPending: () => invoke<OfferDto[]>("list_pending"),
  listContacts: () => invoke<ContactDto[]>("list_contacts"),
  sendTo: (to: string, paths: string[], note: string) =>
    invoke<number>("send_to", { to, paths, note }),
  serveTicket: (paths: string[], seedRelay: string | null) =>
    invoke<{ id: number; ticket: string }>("serve_ticket", {
      paths,
      seedRelay,
    }),
  createLink: (path: string, ttl: number | null, max: number | null) =>
    invoke<string>("create_link", { path, ttl, max }),
  acceptOffer: (offerId: string, out: string | null) =>
    invoke<number>("accept_offer", { offerId, out }),
  rejectOffer: (offerId: string) =>
    invoke<void>("reject_offer", { offerId }),
  pause: (id: number) => invoke<void>("pause", { id }),
  resume: (id: number) => invoke<void>("resume", { id }),
  cancel: (id: number) => invoke<void>("cancel", { id }),
  remove: (id: number) => invoke<void>("remove", { id }),
  markVerified: (name: string) => invoke<void>("mark_verified", { name }),
  guiVersion: () => invoke<string>("gui_version"),
};

/** Subscribe to the pushed engine event stream, flattening each event out of its
 *  wire form (see `events.ts` — the daemon sends serde's externally tagged shape,
 *  not `{ type, ... }`). Unknown events are dropped, not forwarded. */
export function onEngineEvent(cb: (ev: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<WireEvent>("engine://event", (e) => {
    const ev = normalizeEvent(e.payload);
    if (ev) cb(ev);
  });
}

/** Subscribe to the connected/disconnected heartbeat. */
export function onConnected(cb: (connected: boolean) => void): Promise<UnlistenFn> {
  return listen<boolean>("engine://connected", (e) => cb(e.payload));
}
