import type { PickedItem, RecentPhoto } from "./types";
// Typed wrappers over the Tauri command bridge + the engine event channel.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { normalizeEvent, type WireEvent } from "./events";
import type {
  ConfigDto,
  ConfigPatch,
  ContactDto,
  DepositDto,
  EngineEvent,
  HistoryDto,
  OfferDto,
  PairKind,
  PresenceDto,
  StatusDto,
  SyncDto,
  DeviceDto,
  TransferDto,
  UpdateInfo,
  UpdateProgress,
} from "./types";

/** An accept that did not happen, carrying the daemon's word for why.
 *
 *  `code` is one of the engine's refusal codes (`needsPassword`, `p2pOff`,
 *  `noLongerWaiting`) and empty for anything else — including a daemon too old to
 *  send one, where the only thing left to read is the sentence. */
export type AcceptError = Error & { code: string };

export function asAcceptError(e: unknown): AcceptError {
  const raw = e as { code?: unknown; message?: unknown } | null;
  const message =
    typeof raw?.message === "string" ? raw.message : String(e ?? "");
  const code = typeof raw?.code === "string" ? raw.code : "";
  return Object.assign(new Error(message), { code });
}

/** An action the daemon refused, carrying its word for why and, for a file the
 *  relay will not hold, the relay's cap in bytes.
 *
 *  `code` is empty for a plain failure — and for a daemon too old to send one,
 *  where the only thing left to read is the sentence. `limitBytes` is `null`
 *  unless the code is `tooLarge` and the relay named its limit. */
export type Refusal = Error & { code: string; limitBytes: number | null };

export function asRefusal(e: unknown): Refusal {
  const raw = e as
    | { code?: unknown; message?: unknown; limit_bytes?: unknown }
    | null;
  const message =
    typeof raw?.message === "string" ? raw.message : String(e ?? "");
  const code = typeof raw?.code === "string" ? raw.code : "";
  const limitBytes =
    typeof raw?.limit_bytes === "number" && raw.limit_bytes > 0
      ? raw.limit_bytes
      : null;
  return Object.assign(new Error(message), { code, limitBytes });
}

export const api = {
  status: () => invoke<StatusDto>("status"),
  listTransfers: () => invoke<TransferDto[]>("list_transfers"),
  listPending: () => invoke<OfferDto[]>("list_pending"),
  listContacts: () => invoke<ContactDto[]>("list_contacts"),
  /** Open the native picker; resolves to registered items (empty = cancelled). */
  pickFiles: (directory: boolean, media = false) =>
    invoke<PickedItem[]>("pick_files", { directory, media }),
  /** `device` aims it at one of our own machines; only meaningful when `to` is
   *  the reserved self recipient, and refused by the daemon otherwise. */
  sendTo: (
    to: string,
    items: string[],
    note: string,
    device: string | null = null,
    /** Never the mailbox (13d): the send waits for them and goes direct. */
    liveOnly = false
  ) => invoke<number>("send_to", { to, items, note, device, liveOnly }),
  /** `send --deposit`: skip the live attempt, leave it in their mailbox. Returns
   *  the `arvm…` ticket too, so it can also be handed over by other means. */
  depositTo: (
    to: string,
    items: string[],
    note: string,
    ttl: number | null,
    max: number | null,
    password: string | null,
    device: string | null = null
  ) =>
    invoke<{ id: number; ticket: string }>("deposit_to", {
      to,
      items,
      note,
      ttl,
      max,
      password,
      device,
    }),
  serveTicket: (items: string[], seedRelay: string | null) =>
    invoke<{ id: number; ticket: string }>("serve_ticket", {
      items,
      seedRelay,
    }),
  serveCode: (items: string[], relay: string | null, keep: boolean) =>
    invoke<{ id: number; code: string }>("serve_code", { items, relay, keep }),
  /** Rejects with a `Refusal`: the relay's size cap arrives as `tooLarge` with
   *  `limitBytes`, so the notice can say how large is too large. */
  createLink: (item: string, ttl: number | null, max: number | null) =>
    invoke<string>("create_link", { item, ttl, max }).catch((e) => {
      throw asRefusal(e);
    }),
  recv: (ticket: string, outItem: string | null, password: string | null) =>
    invoke<number>("recv", { ticket, outItem, password }),
  acceptOffer: async (
    offerId: string,
    outItem: string | null,
    password?: string | null
  ) => {
    try {
      return await invoke<number>("accept_offer", {
        offerId,
        outItem,
        password: password ?? null,
      });
    } catch (e) {
      // This one command rejects with `{code, message}` rather than a bare
      // string: a refusal carries the daemon's own word for itself, and the
      // dialog acts on it. Re-thrown as an Error so `message` reads the same as
      // every other failure, with the code riding along for whoever wants it.
      throw asAcceptError(e);
    }
  },
  rejectOffer: (offerId: string) =>
    invoke<void>("reject_offer", { offerId }),
  pause: (id: number) => invoke<void>("pause", { id }),
  resume: (id: number) => invoke<void>("resume", { id }),
  /** Going to sleep, and coming back. See `bridge::suspend_transfers`. */
  suspendTransfers: () => invoke<void>("suspend_transfers"),
  resumeSuspendedTransfers: () => invoke<void>("resume_suspended_transfers"),
  /** The window went away, or came back — see `window_hidden` in the backend. */
  windowHidden: (hidden: boolean) => invoke<void>("window_hidden", { hidden }),
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
  /** Stop waiting for the recipient to accept and park the file on the relay.
   *  Rejects when the send is not waiting — the daemon says so rather than
   *  reporting a success that did nothing. */
  depositNow: (id: number) => invoke<void>("deposit_now", { id }),
  addContact: (name: string, id: string) =>
    invoke<void>("add_contact", { name, id }),
  /** Point an existing contact name at a different key. `addContact` refuses that
   *  collision — re-pointing a name drops its verified and trusted marks — so
   *  this is the only way past it, and the caller must have asked first. Resolves
   *  to the old and new fingerprints when a key really did change. */
  rebindContact: (name: string, id: string) =>
    invoke<[string, string] | null>("rebind_contact", { name, id }),
  removeContact: (name: string) => invoke<void>("remove_contact", { name }),
  renameContact: (old: string, newName: string) =>
    invoke<void>("rename_contact", { old, new: newName }),
  listHistory: () => invoke<HistoryDto[]>("list_history"),
  clearHistory: () => invoke<number>("clear_history"),
  setMyName: (name: string) => invoke<void>("set_my_name", { name }),
  restartDaemon: () => invoke<void>("restart_daemon"),
  listDeposits: () => invoke<DepositDto[]>("list_deposits"),
  revokeDeposit: (id: string) => invoke<void>("revoke_deposit", { id }),
  getConfig: () => invoke<ConfigDto>("get_config"),
  setConfig: (patch: ConfigPatch) => invoke<ConfigDto>("set_config", { patch }),
  /** Who is reachable right now. Separate from `listContacts` because the book
   *  is read from disk and is instant, while this is a relay round trip. */
  presence: (ids: string[]) => invoke<PresenceDto[]>("presence", { ids }),
  syncStatus: () => invoke<SyncDto>("sync_status"),
  syncNow: () => invoke<SyncDto>("sync_now"),
  listDevices: () => invoke<DeviceDto[]>("list_devices"),
  /** Rename *this* machine. The others are renamed on themselves — a device row
   *  may only be written by the device it describes. */
  renameThisDevice: (name: string) =>
    invoke<void>("rename_this_device", { name }),
  forgetDevice: (who: string) => invoke<void>("forget_device", { who }),
  /** Begin a pairing exchange; returns the session handle. The code to read out
   *  and the outcome arrive as `pairing_*` engine events, not as a return value —
   *  pairing waits on a human at another machine. */
  startPairing: (
    kind: PairKind,
    relay: string | null,
    code: string | null,
    name: string | null
  ) => invoke<string>("start_pairing", { kind, relay, code, name }),
  cancelPairing: (session: string) =>
    invoke<void>("cancel_pairing", { session }),
  /** Read/write a single file the user picked in a native dialog — the whole of
   *  the app's filesystem access from the frontend, used by address-book
   *  import/export. */
  /** Native open dialog + read, on the Rust side: the webview never sees a path.
   *  Resolves to the file's text, or null if the user cancelled. */
  importContacts: () => invoke<string | null>("import_contacts"),
  /** Native save dialog + write. Resolves to the chosen file name, or null. */
  exportContacts: (defaultName: string, contents: string) =>
    invoke<string | null>("export_contacts", { defaultName, contents }),
  // Save an arvc… ticket as a shareable .arvolo file (native dialog, Rust-side
  // path). Resolves to the chosen file name, or null if cancelled.
  saveTicket: (defaultName: string, ticket: string) =>
    invoke<string | null>("save_ticket", { defaultName, ticket }),
  guiVersion: () => invoke<string>("gui_version"),
  /** Whether a newer release exists; null when there is nothing on offer.
   *  `pinned` asks one specific release's manifest instead of `latest.json` —
   *  used to look up a version the relay can still talk to. */
  checkUpdate: (pinned: string | null = null) =>
    invoke<UpdateInfo | null>("check_update", { pinned }),
  /** Download, stop the daemon, install, restart. Never resolves on success —
   *  the process is replaced — so callers must not await it expecting a return. */
  installUpdate: (version: string | null = null) =>
    invoke<void>("install_update", { version }),
  // Open a file (or folder) with whatever the system uses for it. Goes through
  // the bridge, not `@tauri-apps/plugin-opener`: the plugin's `openPath` is behind
  // an ACL scope that has to be written out at build time, and the paths here —
  // download folder, a received file, `config.toml` — are only known at runtime.
  // The Rust side checks the path against the daemon before opening it.
  openPath: (path: string) => invoke<void>("open_path", { path }),
  /** Metti una copia di un file ricevuto nella Galleria del telefono, e tieni
   *  l'identificatore che torna: «Togli dalla Galleria» deve poter togliere
   *  esattamente quella copia. Il lato Rust rifiuta quello che non è una foto o
   *  un video, ed è lui l'autorità sull'elenco dei formati. */
  photosAdd: (path: string) => invoke<string>("photos_add", { path }),
  photosRemove: (id: string) => invoke<void>("photos_remove", { id }),
  /** C'è una Galleria su questa piattaforma? Una preferenza che non può valere
   *  non va mostrata. */
  photosAvailable: () => invoke<boolean>("photos_available"),
  /** The last `limit` pictures and videos, newest first — or `null` where the
   *  phone has no library to read (a desktop, a refused permission). */
  photosRecent: (limit: number, offset = 0) =>
    invoke<RecentPhoto[] | null>("photos_recent", { limit, offset }),
  /** Those pictures as files the engine can send. */
  photosExport: (ids: string[]) => invoke<PickedItem[]>("photos_export", { ids }),
  /** Offer a link to the system's own share sheet. iOS only — see `share_link`;
   *  the screen asks `canShareLink()` before drawing the button. */
  shareLink: (url: string) => invoke<void>("share_link", { url }),
  /** A line to the device console, for what a phone cannot otherwise be asked. */
  note: (msg: string) => invoke<void>("note", { msg }),
  // The ticket of a .arvolo the app was LAUNCHED with (double click before the
  // window existed). One-shot: the backend hands it over once.
  takePendingTicket: () => invoke<string | null>("take_pending_ticket"),
  /** What another app shared with Arvolo and nobody has taken yet, registered
   *  as picked files. Empty on a desktop, which has no share sheet. */
  takePendingShared: () => invoke<PickedItem[]>("take_pending_shared"),
  // Keep the native side (tray menu, arrival notifications) in the webview's
  // language — they used to be hardcoded Italian in a four-language app.
  setUiLanguage: (lang: string) => invoke<void>("set_ui_language", { lang }),
};

/** A .arvolo handed to the running app (drop on the window, macOS Opened). */
export function onArvoloTicket(cb: (ticket: string) => void): Promise<UnlistenFn> {
  return listen<string>("files://arvolo", (e) => cb(e.payload));
}

/** Subscribe to the pushed engine event stream, flattening each event out of its
 *  wire form (see `events.ts` — the daemon sends serde's externally tagged shape,
 *  not `{ type, ... }`). Unknown events are dropped, not forwarded. */
export function onEngineEvent(cb: (ev: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<WireEvent>("engine://event", (e) => {
    const ev = normalizeEvent(e.payload);
    if (ev) cb(ev);
  });
}

/** Files just dropped on the window, already registered on the Rust side. */
export function onPickedFiles(cb: (items: PickedItem[]) => void): Promise<UnlistenFn> {
  return listen<PickedItem[]>("files://picked", (e) => cb(e.payload));
}

/** The nudge that something was shared from another app — the files are
 *  fetched with `takePendingShared`, never carried by the event. */
export function onSharedArrived(cb: () => void): Promise<UnlistenFn> {
  return listen<void>("shared://arrived", () => cb());
}

/** Subscribe to *why* the daemon would not start, when it would not start. */
export function onDaemonError(cb: (reason: string) => void): Promise<UnlistenFn> {
  return listen<string>("engine://daemon-error", (e) => cb(e.payload));
}

/** Subscribe to the connected/disconnected heartbeat. */
export function onConnected(cb: (connected: boolean) => void): Promise<UnlistenFn> {
  return listen<boolean>("engine://connected", (e) => cb(e.payload));
}

/** Bytes of an update downloaded so far. `total` is absent when the server did
 *  not say how big the file is, which is why the bar has an indeterminate form. */
export function onUpdateProgress(
  cb: (p: UpdateProgress) => void
): Promise<UnlistenFn> {
  return listen<UpdateProgress>("update://progress", (e) => cb(e.payload));
}
