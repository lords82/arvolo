// Wire types — 1:1 with `arvolo_ipc::protocol` DTOs, and the derived UI model.

export type Direction = "out" | "in";
export type Method = "p2p" | "cloud" | "link" | "ticket";

/** UI lifecycle state (Italian labels come from the mock's state machine). */
export type UIStatus =
  | "in arrivo" // a parked offer awaiting accept/reject
  | "in corso" // actively transferring
  | "in attesa" // manually paused
  | "in stallo" // auto-held, retrying when possible (daemon "waiting")
  | "deposited" // handed to the relay mailbox
  | "completato"
  | "fallito"
  | "in annullamento" // cancel sent, waiting for the daemon to confirm it
  | "annullato";

// ---- daemon DTOs (mirror the Rust wire types) ----------------------------

export interface TransferDto {
  id: number;
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
  | { type: "contacts_changed" };

// ---- UI model -------------------------------------------------------------

export interface UITransfer {
  /** Stable list key: `t<id>` for a transfer, `o<offerId>` for a pending offer. */
  key: string;
  /** Numeric daemon transfer id (0 for a not-yet-accepted offer). */
  id: number;
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
  encrypted: boolean;
  verified: boolean;
  method: Method;
  swarmPeers: number;
  downloadPeers: number;
  files: number;
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
}
