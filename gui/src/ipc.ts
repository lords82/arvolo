// Typed wrappers over the Tauri command bridge + the engine event channel.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { normalizeEvent, type WireEvent } from "./events";
import type {
  ContactDto,
  DepositDto,
  EngineEvent,
  HistoryDto,
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
  serveCode: (paths: string[], relay: string | null, keep: boolean) =>
    invoke<{ id: number; code: string }>("serve_code", { paths, relay, keep }),
  createLink: (path: string, ttl: number | null, max: number | null) =>
    invoke<string>("create_link", { path, ttl, max }),
  recv: (ticket: string, out: string | null, password: string | null) =>
    invoke<number>("recv", { ticket, out, password }),
  acceptOffer: (offerId: string, out: string | null) =>
    invoke<number>("accept_offer", { offerId, out }),
  rejectOffer: (offerId: string) =>
    invoke<void>("reject_offer", { offerId }),
  pause: (id: number) => invoke<void>("pause", { id }),
  resume: (id: number) => invoke<void>("resume", { id }),
  cancel: (id: number) => invoke<void>("cancel", { id }),
  remove: (id: number) => invoke<void>("remove", { id }),
  clearFinished: () => invoke<number>("clear_finished"),
  markVerified: (name: string) => invoke<void>("mark_verified", { name }),
  markUnverified: (name: string) => invoke<void>("mark_unverified", { name }),
  markTrusted: (who: string, force: boolean) =>
    invoke<void>("mark_trusted", { who, force }),
  markUntrusted: (who: string) => invoke<void>("mark_untrusted", { who }),
  blockContact: (who: string) => invoke<void>("block_contact", { who }),
  unblockContact: (who: string) => invoke<void>("unblock_contact", { who }),
  acceptName: (who: string) => invoke<void>("accept_name", { who }),
  addContact: (name: string, id: string) =>
    invoke<void>("add_contact", { name, id }),
  removeContact: (name: string) => invoke<void>("remove_contact", { name }),
  renameContact: (old: string, newName: string) =>
    invoke<void>("rename_contact", { old, new: newName }),
  listHistory: () => invoke<HistoryDto[]>("list_history"),
  clearHistory: () => invoke<number>("clear_history"),
  setMyName: (name: string) => invoke<void>("set_my_name", { name }),
  restartDaemon: () => invoke<void>("restart_daemon"),
  listDeposits: () => invoke<DepositDto[]>("list_deposits"),
  revokeDeposit: (id: string) => invoke<void>("revoke_deposit", { id }),
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
