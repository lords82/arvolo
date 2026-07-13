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
}

export interface StatusDto {
  version: string;
  public_id: string;
  fingerprint: string;
  relay: string | null;
  transfers: number;
  pending: number;
  download_dir: string;
}

/** Mirrors `EventDto` (serde snake_case, internally tagged by `type`). */
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
  | { type: "cancelled"; id: number };

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
  /** Client-side arrival time (ms) — daemon carries no timestamp; used for the
   *  Oggi / Precedenti grouping only. */
  firstSeen: number;
  /** Manual list position (defaults to firstSeen; "Sposta su/giù" swaps it). */
  rank: number;
  /** Smoothed throughput in bytes/sec, derived from progress events. */
  rate?: number;
}
