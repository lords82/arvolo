//! Shared server state, capacity/TTL constants, and their env overrides.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arvolo_core::backfill::BlobNode;

use crate::limits::{RzLimiter, WriteLimiter};
use crate::mailbox::Mailbox;

/// Shared HTTP state: the zero-knowledge mailbox plus the blob-store node that
/// backs seed-to-relay backfill.
#[derive(Clone)]
pub struct AppState {
    pub mailbox: Arc<Mailbox>,
    pub blobs: Arc<BlobNode>,
    /// Per-process secret keying the inbox session-token MAC. Random at startup:
    /// a restart just forces clients to re-run the (cheap) auth handshake.
    pub auth_secret: Arc<[u8; 32]>,
    /// Whether this relay offers public **browser download links** (`--link`).
    /// When an administrator disables it, the `/dl` page is not served and
    /// link deposits (HPKE-less blobs) are refused. Defaults to enabled.
    pub links_enabled: bool,
    /// Swarm tracker: `swarm_id → (peer node_addr → entry)`, in-memory with a TTL.
    /// Purely a rendezvous for peers of one shared `arvc…` ticket to find each
    /// other; the relay learns node addresses + bitfields, never the key/plaintext.
    pub swarm: Arc<Mutex<HashMap<String, HashMap<String, SwarmPeer>>>>,
    /// Per-IP rate-limiter state for the rendezvous routes (nameplate-sweep and
    /// pairing-griefing guard; see the rendezvous rate-limiting section).
    pub rz_limiter: Arc<RzLimiter>,
    /// Per-IP rate-limiter state for the unauthenticated *write* routes (deposit,
    /// seed, inbox-post, swarm-announce, presence). Bounds cheap disk/peer-list
    /// abuse from a single source; see the write rate-limiting section.
    pub write_limiter: Arc<WriteLimiter>,
}

/// One tracked peer of a swarm (in-memory tracker row).
#[derive(Clone)]
pub struct SwarmPeer {
    pub node_addr: String,
    pub bitfield: Vec<u8>,
    pub expires_at: u64,
}

/// How long a swarm peer stays listed without re-announcing.
pub const SWARM_PEER_TTL_SECS: u64 = 60;
/// Max peers tracked (and returned) per swarm — bounds memory and response size.
pub const MAX_SWARM_PEERS: usize = 100;
/// Max distinct swarms tracked at once (disk/memory guard; new swarms rejected
/// beyond this until entries expire).
pub const MAX_SWARMS: usize = 10_000;
/// Sanity cap on a swarm's piece count (bounds an announced bitfield).
pub const MAX_SWARM_CHUNKS: u32 = 1_000_000;

/// Whether the relay administrator has disabled public download links, via
/// `ARVOLO_DISABLE_LINKS` (set to `1`/`true`/`yes`/`on`).
pub fn links_disabled_from_env() -> bool {
    matches!(
        std::env::var("ARVOLO_DISABLE_LINKS")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

impl AppState {
    /// Build state with a fresh random inbox-auth secret.
    pub fn new(mailbox: Arc<Mailbox>, blobs: Arc<BlobNode>) -> Self {
        Self {
            links_enabled: true,
            mailbox,
            blobs,
            auth_secret: Arc::new(rand::random()),
            swarm: Arc::new(Mutex::new(HashMap::new())),
            rz_limiter: Arc::new(Mutex::new(HashMap::new())),
            write_limiter: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Default cap on a single deposited blob (2 GiB). The deposit path streams the
/// body straight to disk (never buffering it in memory) and enforces this as it
/// writes, so the bound is on disk footprint, not memory. Finite by default so an
/// unauthenticated deposit can't fill the disk with one huge blob; a private relay
/// can raise or lift it by setting `ARVOLO_MAX_BLOB_BYTES` (`0` = unlimited).
pub const DEFAULT_MAX_BLOB_BYTES: usize = 16 * 1024 * 1024 * 1024;

/// Default cap on the **aggregate** bytes of all stored blobs. `0` = unlimited
/// (the relay warns at startup): the per-blob cap above is a *functional* bound
/// (the largest file an offline send / link can carry), while this is the real
/// disk-fill guard — without it, an attacker can fill the disk with many
/// blob-cap-sized deposits regardless of how small the per-blob cap is. Override
/// with `ARVOLO_MAX_TOTAL_BLOB_BYTES`.
pub const DEFAULT_MAX_TOTAL_BLOB_BYTES: u64 = 0;

/// Fixed request-body limit for the small control-plane routes (rendezvous,
/// inbox, presence, seed, swarm — all a few hundred KiB at most). Comfortably
/// above every per-route check; the streaming `/v1/deposit` route is exempt and
/// enforces [`max_blob_bytes`] itself as it writes.
pub(crate) const CONTROL_PLANE_BODY_LIMIT: usize = 16 * 1024 * 1024; // 16 MiB
/// Default cap on the number of stored mailbox entries (disk-fill guard).
/// Override with `ARVOLO_MAX_ENTRIES`.
pub const DEFAULT_MAX_ENTRIES: i64 = 100_000;
/// Cap on chunks a single seed request may ask the relay to fetch+store.
pub const MAX_SEED_CHUNKS_PER_REQ: usize = 4096;
/// Default cap on the total number of pending seeded chunk rows (each backs one
/// ≤16 MiB chunk file on disk), bounding the unauthenticated seed path's disk
/// footprint. Override with `ARVOLO_MAX_SEEDED_ROWS`.
pub const DEFAULT_MAX_SEEDED_ROWS: i64 = 50_000;
/// Cap on a rendezvous value (SPAKE2 message / encrypted ticket are tiny).
pub const MAX_RZ_VALUE_BYTES: usize = 64 * 1024; // 64 KiB
/// Default cap on the number of live rendezvous rows (disk-fill guard).
pub const DEFAULT_MAX_RZ_ROWS: i64 = 100_000;
/// Cap on a single sealed inbox offer (metadata + an `arvc` ticket whose size
/// grows with chunk count — a big file's ticket is a few hundred KiB).
pub const MAX_INBOX_VALUE_BYTES: usize = 512 * 1024; // 512 KiB
/// Default cap on the total number of live inbox rows (disk-fill guard).
/// Override with `ARVOLO_MAX_INBOX_ROWS`.
pub const DEFAULT_MAX_INBOX_ROWS: i64 = 100_000;
/// Cap on pending offers a single inbox slot may hold, so one victim's inbox
/// can't be flooded (and so a slow reader can't accumulate unbounded rows).
pub const MAX_INBOX_PER_SLOT: i64 = 64;
/// Default TTL (seconds) for a deposited offer when the client doesn't request
/// one: long enough for a client that is briefly away to come back, short enough
/// that an unaccepted offer vanishes.
pub const INBOX_TTL_SECS: u64 = 600;
/// Upper bound on a caller-requested offer TTL (`?ttl=`). An offline offer must
/// survive as long as the mailbox blob it points at (up to the blob TTL cap), so
/// this mirrors the deposit TTL cap rather than the short live-offer default.
pub const INBOX_MAX_TTL_SECS: u64 = 30 * 24 * 3600; // 30 days
/// TTL (seconds) of a presence beacon: a listening client refreshes well within
/// this, so a stale beacon means the client went away. Kept short so a departed
/// client stops showing "online" quickly (the send-side watchdog covers the brief
/// window where a just-quit client still looks online).
pub const PRESENCE_TTL_SECS: u64 = 30;
/// Default cap on the number of live presence beacon rows (disk-fill guard).
/// Override with `ARVOLO_MAX_PRESENCE_ROWS`.
pub const DEFAULT_MAX_PRESENCE_ROWS: i64 = 100_000;
/// Default cap on a blob's TTL (seconds). Prevents effectively-immortal entries
/// permanently occupying storage — and the i64 overflow a near-`u64::MAX` TTL
/// would cause (`now + ttl` wraps negative → the entry never expires). Override
/// with `ARVOLO_MAX_TTL`.
pub const DEFAULT_MAX_TTL_SECS: u64 = 30 * 24 * 3600; // 30 days
/// Upper bound on the max-downloads a single deposit may request.
pub const MAX_DOWNLOADS_CAP: u32 = 10_000;

pub(crate) fn env_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Max deposited-blob size in bytes (env `ARVOLO_MAX_BLOB_BYTES`). Defaults to
/// [`DEFAULT_MAX_BLOB_BYTES`] (16 GiB — this bounds the largest single file an
/// offline send or link can carry, so it is deliberately roomy); `0` lifts the
/// limit for a private relay. Disk-fill protection comes from
/// [`max_total_blob_bytes`], not from this per-blob bound.
pub fn max_blob_bytes() -> usize {
    env_usize("ARVOLO_MAX_BLOB_BYTES", DEFAULT_MAX_BLOB_BYTES)
}

/// Aggregate cap on all stored blob bytes (env `ARVOLO_MAX_TOTAL_BLOB_BYTES`).
/// `0` (the default) means unlimited; a public relay should set it to the disk
/// budget it is willing to lend out.
pub fn max_total_blob_bytes() -> u64 {
    env_usize(
        "ARVOLO_MAX_TOTAL_BLOB_BYTES",
        DEFAULT_MAX_TOTAL_BLOB_BYTES as usize,
    ) as u64
}

pub(crate) fn max_entries() -> i64 {
    env_usize("ARVOLO_MAX_ENTRIES", DEFAULT_MAX_ENTRIES as usize) as i64
}

pub(crate) fn max_rz_rows() -> i64 {
    env_usize("ARVOLO_MAX_RZ_ROWS", DEFAULT_MAX_RZ_ROWS as usize) as i64
}

pub(crate) fn max_inbox_rows() -> i64 {
    env_usize("ARVOLO_MAX_INBOX_ROWS", DEFAULT_MAX_INBOX_ROWS as usize) as i64
}

pub(crate) fn max_presence_rows() -> i64 {
    env_usize(
        "ARVOLO_MAX_PRESENCE_ROWS",
        DEFAULT_MAX_PRESENCE_ROWS as usize,
    ) as i64
}

pub(crate) fn max_ttl_secs() -> u64 {
    env_usize("ARVOLO_MAX_TTL", DEFAULT_MAX_TTL_SECS as usize) as u64
}

pub(crate) fn max_seeded_rows() -> i64 {
    env_usize("ARVOLO_MAX_SEEDED_ROWS", DEFAULT_MAX_SEEDED_ROWS as usize) as i64
}

/// Per-session relay-offload cap in bytes (env `ARVOLO_MAX_SESSION_RELAY_BYTES`).
/// `0` (the default) means **unlimited**: by default the relay meters nothing and
/// carries as much of a transfer as it is asked to. A public/shared relay sets a
/// non-zero value to bound how many bytes any one transfer may lean on it —
/// forcing the remainder onto direct P2P once the cap is reached.
pub fn max_session_relay_bytes() -> u64 {
    env_usize("ARVOLO_MAX_SESSION_RELAY_BYTES", 0) as u64
}
