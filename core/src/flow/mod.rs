//! Transfer flows: the full send/recv orchestration, composed from the crate's
//! primitives ([`crate::chunked`], [`crate::crypto`], [`crate::backfill`]).
//!
//! The CLI and any UI (desktop/browser/mobile) drive transfers through here,
//! reporting progress via a callback and cancelling via a [`CancellationToken`](tokio_util::sync::CancellationToken)
//! — so orchestration lives once, in the core, not in each front-end.

mod archive;
mod ctrl;
mod offline;
mod recv;
mod schedule;
mod send;
mod sidecar;
mod storage;

pub use archive::pack_tar;
pub use offline::{
    claim_info, claim_status, deposit_offline, fetch_offline, revoke_offline, ClaimInfo,
    ClaimStatus, DepositError, Deposited,
};
// Shared with the link path, which deposits through its own request but has to read
// the same granted-TTL answer off it.
pub(crate) use offline::granted_ttl;
pub use recv::{
    default_out, discard_incomplete, recv_chunked, safe_download_name, ChunkSource, RecvEvent,
    RecvOutcome,
};
pub use send::{prepare_send, resume_send, SendEvent, SendSession};
pub use sidecar::read_ticket;

pub(crate) use recv::{archive_stage_path, seeding_enabled, spawn_swarm_coordinator};

/// AAD binding the sealed content key to its purpose (`--to` sends).
pub(super) const CHUNK_KEY_AAD: &[u8] = b"arvolo/chunk-key/v1";

/// AAD binding the sealed content key to the offline-mailbox purpose (distinct
/// from the P2P [`CHUNK_KEY_AAD`] so a key sealed for one flow can't be replayed
/// into the other).
pub(super) const MAILBOX_KEY_AAD: &[u8] = b"arvolo/mailbox-key/v1";
