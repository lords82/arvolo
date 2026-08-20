// Wire types — 1:1 with `arvolo_ipc::protocol` DTOs, and the derived UI model.

export type Direction = "out" | "in";

/** UI lifecycle state. These are internal identifiers, never rendered: the words
 *  the user reads come from `statusMeta` in format.ts, in their own language. */
export type UIStatus =
  | "incoming" // a parked offer awaiting accept/reject
  | "active" // actively transferring
  | "sharing" // made available and waiting to be fetched — not progress
  | "paused" // manually paused
  | "stalled" // auto-held, retrying when possible (daemon "waiting")
  | "deposited" // handed to the relay mailbox
  | "completed"
  | "failed"
  | "cancelling" // cancel sent, waiting for the daemon to confirm it
  | "cancelled";

// ---- daemon DTOs (mirror the Rust wire types) ----------------------------

export interface TransferDto {
  id: number;
  // The 8-hex handle the CLI shows and takes (`arvolo cancel <handle>`).
  // Restart-stable, unlike `id`. Optional: absent from an older daemon.
  handle?: string;
  direction: "send" | "recv";
  peer: string | null;
  name: string;
  total_size: number;
  transferred: number;
  status: string; // "active" | "completed" | "deposited" | "cancelled" | "failed: .." | "waiting: .." | "paused: .."
  swarm_peers: number;
  pieces_from_peers: number;
  download_peers: number;
  /** Unix seconds when the transfer began, per the engine. 0 from a daemon that
   *  predates the field. */
  created: number;
  /** The live pairing code this send answers to, while one is hosted. */
  code?: string | null;
  /** Where a completed receive was saved. Present on the snapshot too, not only
   *  on the event that announced it — a window that starts after a download
   *  finished must still be able to open it. */
  path?: string | null;
  /** How far the inbox offer for a deposited send has got. Same vocabulary as a
   *  deposit's `offer_status`; null when there is nothing to say. */
  offer_status?: string | null;
  copies_served?: number;
  bytes_served?: number;
  last_pickup?: number;
  from_download?: number;
  /** This row is a file being *made available* — a served ticket/code, or the
   *  seeding a finished download turns into — rather than a transfer under way.
   *  None of them ends on its own, so none is progress towards anything: pair it
   *  with `download_peers` to tell "available" from "being pulled right now". */
  sharing?: boolean;
}

export interface OfferDto {
  id: string;
  from: string;
  name: string;
  size: number;
  note: string;
  sender_name: string;
}

export interface ContactDto {
  name: string;
  id: string;
  fingerprint: string;
  verified: boolean;
  /** Auto-download without a prompt (contacts trust). */
  trusted: boolean;
  /** Offers dropped on arrival (contacts block). */
  blocked: boolean;
  /** The advertised display name already approved ("" when none). */
  display_name: string;
  /** An advertised name awaiting approval ("" when none). */
  pending_name: string;
}

/** One finished transfer from the daemon's history log (read-only). */
export interface HistoryDto {
  id: string;
  direction: "send" | "recv";
  peer: string | null;
  name: string;
  total_size: number;
  transferred: number;
  /** "completed" | "cancelled" | "failed: …" | "deposited". */
  status: string;
  /** Unix seconds. */
  created: number;
}

/** Something left on a relay that can still be taken back: a public download link,
 *  or a sealed deposit waiting for its recipient. No revoke token — that secret
 *  stays in the daemon; the id is all a UI needs to ask for a withdrawal. */
export interface DepositDto {
  id: string;
  /** "link" | "offline" */
  kind: string;
  name: string;
  size: number;
  /** The browser URL, for a link. Empty for a sealed deposit. */
  link: string;
  /** The `arvm…` ticket to hand over, for a sealed deposit. Empty for a link
   *  (its URL is above) and for a deposit made before tickets were kept — those
   *  can still be withdrawn, but never handed over again. */
  ticket: string;
  /** The recipient's base32 id, for a sealed deposit. Empty for a link. */
  recipient: string;
  created: number;
  expires: number;
  expired: boolean;
  max_label: string;
  /** Does the relay still hold it? The local record is only a receipt of the
   *  deposit and never learns that a link was downloaded or a sealed deposit
   *  collected, so the daemon asks the relay. `null` = it could not ask; show
   *  "unknown", never "alive". */
  present: boolean | null;
  /** Times the relay has served it. `null` when unknown (see `present`). */
  downloads: number | null;
  /** The relay's own cap, possibly lower than the one requested. `null` when
   *  unknown (see `present`). */
  max_downloads: number | null;
  /** How far the offer in the recipient's inbox has got: `"pending"` (no client
   *  of theirs has read it), `"arrived"` (it reached one of their devices — a
   *  listing sets this as much as their daemon does, so it is never "they have
   *  the file", nor even "they looked"), `"taken"` (fetched and acked: the only
   *  state that reports a person acting), `"gone"` (retracted, or lapsed unread).
   *
   *  `null` when there is nothing to say: a public link leaves no offer, and a
   *  relay that could not be asked leaves no answer. Like `present`, absence is
   *  "unknown" — never "not taken". */
  offer_status: string | null;
}

export interface StatusDto {
  version: string;
  public_id: string;
  fingerprint: string;
  relay: string | null;
  transfers: number;
  pending: number;
  download_dir: string;
  /** The display name advertised in offers ("" when none is set). */
  display_name: string;
}

/** The settings screen: what is in force, and what `config.toml` says. The two
 *  differ whenever an env var or the built-in default is supplying the value, and
 *  the UI has to be able to say which — an unchangeable value must not be
 *  presented as an editable one. */
export interface ConfigDto {
  relay: string | null;
  relay_configured: string;
  /** "env" | "config" | "builtin" | "none" */
  relay_source: string;
  download_dir: string;
  download_dir_configured: string;
  download_dir_from_env: boolean;
  display_name: string;
  sync: boolean;
  seed: boolean | null;
  swarm: string;
  concurrency: number | null;
  config_path: string;
  identity_path: string;
}

/** One field of a config patch: set it, or clear it back to the default. Mirrors
 *  Rust's `Setting<T>` (externally tagged, so `{ set: v }` or `"clear"`). */
export type Setting<T> = { set: T } | "clear";

/** A settings edit. An absent key is left alone — which is why every field is
 *  optional *and* its value is a `Setting`: "don't touch the relay" and "stop
 *  overriding the relay" are different requests. */
export interface ConfigPatch {
  relay?: Setting<string>;
  download_dir?: Setting<string>;
  display_name?: Setting<string>;
  sync?: Setting<boolean>;
  seed?: Setting<boolean>;
  swarm?: Setting<string>;
  concurrency?: Setting<number>;
}

/** Whether one identity is reachable on the relay right now.
 *
 *  `online` is nullable and that is the point: a network failure is not the same
 *  as being away. `null` means the relay could not be asked, and the UI says
 *  "non lo so" rather than "offline". */
export interface PresenceDto {
  id: string;
  online: boolean | null;
}

/** Multi-device state: one identity across machines, address books kept in step
 *  through an encrypted cell on the relay inbox. */
export interface SyncDto {
  fingerprint: string;
  public_id: string;
  contacts: number;
  enabled: boolean;
  /** Unix seconds of the last successful round; 0 = not since this daemon
   *  started. Deliberately not persisted by the daemon — a stamp from a previous
   *  run says nothing about whether sync works now. */
  last_sync: number;
  last_merged: number;
  last_error: string;
}

/** Which pairing. `contact_*` trades public ids between two people; `device_*`
 *  hands this device's identity secret to another machine. They share a
 *  mechanism and nothing else — see the Rust `PairKind`. */
export type PairKind =
  | "contact_host"
  | "contact_join"
  | "device_host"
  | "device_join";

/** The app-model event: `EventDto` flattened by `normalizeEvent`. This is NOT the
 *  wire shape — see `events.ts` for what the daemon actually sends. */
export type EngineEvent =
  | {
      type: "offer_received";
      id: string;
      from: string;
      name: string;
      size: number;
      note: string;
      sender_name: string;
    }
  | {
      type: "started";
      id: number;
      direction: "send" | "recv";
      name: string;
      total_size: number;
    }
  | { type: "progress"; id: number; transferred: number; total_size: number }
  | { type: "completed"; id: number; path: string | null }
  | { type: "deposited"; id: number }
  | { type: "waiting"; id: number; reason: string }
  | { type: "paused"; id: number; reason: string }
  | { type: "failed"; id: number; error: string }
  | { type: "cancelled"; id: number }
  /** A short pairing code is live for this send (fresh, or restored on restart). */
  | { type: "code_ready"; id: number; code: string }
  /** A receiver used the code and now holds the ticket. */
  | { type: "code_paired"; id: number; done: number }
  /** The code stopped working; the send behind it carries on. */
  | { type: "code_closed"; id: number; reason: string }
  /** The address book moved (from this GUI, the CLI, or a sync). Carries nothing:
   *  refetch the contacts. */
  | { type: "contacts_changed" }
  // A waiting offer left the daemon by another client's hand (CLI recv/decline
  // while this window is open) — drop its row instead of showing a ghost.
  | { type: "offer_gone"; id: string }
  /** Finished rows were cleared from the daemon's list by somebody else — an
   *  `arvolo status clear`, or another window. Carries nothing: drop the ones
   *  this window considers finished. */
  | { type: "finished_cleared" }
  /** A hosted pairing session has its code — what the other machine types. */
  | { type: "pairing_code"; session: string; code: string }
  /** A pairing session succeeded. `needs_restart` marks the one case a UI must
   *  act on: a device join replaced the identity the daemon is running as. */
  | {
      type: "pairing_done";
      session: string;
      kind: PairKind;
      summary: string;
      needs_restart: boolean;
    }
  /** A pairing session ended without pairing. `cancelled` separates "the user
   *  closed the sheet" from a real failure worth showing. */
  | {
      type: "pairing_failed";
      session: string;
      kind: PairKind;
      error: string;
      cancelled: boolean;
    };

// ---- UI model -------------------------------------------------------------

export interface UITransfer {
  /** Stable list key: `t<id>` for a transfer, `o<offerId>` for a pending offer. */
  key: string;
  /** Numeric daemon transfer id (0 for a not-yet-accepted offer). */
  id: number;
  /** The 8-hex handle the CLI shows and takes (`arvolo cancel <handle>`).
   *  Empty for offers and against an older daemon. */
  handle?: string;
  /** Offer id, when this row is a parked incoming offer. */
  offerId?: string;
  dir: Direction;
  name: string;
  size: number;
  transferred: number;
  status: UIStatus;
  /** Display label for the peer (contact name, or a shortened id). */
  peer?: string;
  note?: string;
  senderName?: string;
  /** Base32 id of the peer, when known (for verify/name lookups). */
  peerId?: string;
  verified: boolean;
  swarmPeers: number;
  downloadPeers: number;
  /** Short reason for waiting/paused/failed states. */
  reason?: string;
  /** Completed-receive path (for "open folder"). */
  path?: string;
  /** When the transfer began (ms). Comes from the engine's `created`; only a row
   *  the daemon cannot date (an offer, or a pre-`created` daemon) falls back to
   *  the moment we first saw it. Stamping "now" for everything filed yesterday's
   *  transfers under "Oggi" on every restart. */
  firstSeen: number;
  /** Manual list position (defaults to firstSeen; "Sposta su/giù" swaps it). */
  rank: number;
  /** Smoothed throughput in bytes/sec, derived from progress events. */
  rate?: number;
  /** The live pairing code this send answers to, while one is hosted. */
  code?: string;
  /** Share counters, for a `sharing` row (all 0 otherwise). `copiesServed` counts
   *  completed pickups, not people — an anonymous ticket has no identity to count.
   *  `fromDownload` is when the download that started this share finished, and is
   *  0 for a share the user asked for. */
  /** For a deposited send: "pending" | "arrived" | "taken" | "gone", or absent. */
  offerStatus?: string;
  copiesServed: number;
  bytesServed: number;
  lastPickup: number;
  fromDownload: number;
}

/** One picked file as the backend registers it: an opaque id plus display
 *  metadata. Deliberately no path — see `bridge::PickedFiles`. */
export interface PickedItem {
  id: string;
  name: string;
  size: number;
  isDir: boolean;
}
