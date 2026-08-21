//! arvolo relay / mailbox: zero-knowledge store-and-forward.
//!
//! Holds **opaque ciphertext** blobs (the relay never sees plaintext or keys)
//! addressed by a random claim token, each with a TTL after which it is reaped.
//! This is the offline-delivery path: the sender deposits the encrypted blob
//! while the recipient is away; the recipient claims it later; it expires and is
//! deleted on its own.
//!
//! Storage: metadata in **SQLite**, ciphertext as **files on disk** (`blob_dir`).
//! Survives restarts. Milestone 2 scope: a single relay, full-blob deposit, TTL,
//! max-downloads (burn-after-read). Federation, multi-recipient refcount GC, and
//! partial backfill are post-MVP (see docs/ROADMAP-FUTURE.md).

mod http;
mod limits;
mod mailbox;
mod state;
mod util;

pub use http::router;
pub use limits::{
    RzIpWindow, RzLimiter, WriteIpWindow, WriteLimiter, DEFAULT_RZ_POSTS_PER_MIN,
    DEFAULT_RZ_SLOTS_PER_MIN, DEFAULT_WRITES_PER_MIN,
};
pub use mailbox::{Claimed, Deposit, FetchPlan, InboxStatus, Mailbox, MailboxError};
pub use state::{
    links_disabled_from_env, max_blob_bytes, max_session_relay_bytes, max_total_blob_bytes,
    AppState, InboxWaiterGuard, InboxWaiters, SwarmPeer, DEFAULT_MAX_BLOB_BYTES,
    DEFAULT_MAX_ENTRIES, DEFAULT_MAX_INBOX_ROWS, DEFAULT_MAX_PRESENCE_ROWS, DEFAULT_MAX_RZ_ROWS,
    DEFAULT_MAX_SEEDED_ROWS, DEFAULT_MAX_TOTAL_BLOB_BYTES, DEFAULT_MAX_TTL_SECS,
    INBOX_MAX_TTL_SECS, INBOX_TTL_SECS, MAX_DOWNLOADS_CAP, MAX_INBOX_PER_SLOT,
    MAX_INBOX_VALUE_BYTES, MAX_RZ_VALUE_BYTES, MAX_SEED_CHUNKS_PER_REQ, MAX_SWARMS,
    MAX_SWARM_CHUNKS, MAX_SWARM_PEERS, PRESENCE_TTL_SECS, SWARM_PEER_TTL_SECS,
};
pub use util::now_unix;
