use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::crypto::{Identity, PublicId};
use crate::presence::{InboxSubscription, ReceivedOffer};

use super::*;

/// Live state for an in-progress `send --to` delivery, so it can be paused,
/// resumed, or re-driven. Mirrored to disk (a `SendToRecord`) for durability.
#[derive(Clone)]
pub(super) struct Held {
    pub(super) recipient: PublicId,
    pub(super) payload: PathBuf,
    pub(super) name: String,
    pub(super) archive: bool,
    /// Optional sender note to attach to the offer.
    pub(super) note: String,
    /// Flipped by [`TransferManager::pause`] so the running loop, once its token is
    /// cancelled, knows to pause (keep the transfer) rather than cancel (drop it).
    pub(super) pause_flag: Arc<std::sync::atomic::AtomicBool>,
}

pub(super) struct Inner {
    pub(super) next_id: AtomicU64,
    pub(super) transfers: Mutex<HashMap<u64, Transfer>>,
    pub(super) cancels: Mutex<HashMap<u64, CancellationToken>>,
    /// Delivery state for active/paused `send --to` transfers (for pause/resume).
    pub(super) held: Mutex<HashMap<u64, Held>>,
    pub(super) pending: Mutex<HashMap<String, ReceivedOffer>>,
    pub(super) events: broadcast::Sender<ManagerEvent>,
    pub(super) me: Identity,
    pub(super) relay: Option<String>,
    pub(super) client: reqwest::Client,
    pub(super) download_dir: PathBuf,
    /// Where to persist resumable-download records (present in the daemon). When
    /// set, an accepted chunked download writes a record here on start and removes
    /// it on finish, so [`resume_incomplete`](TransferManager::resume_incomplete)
    /// can restart it after a daemon/machine restart. `None` = no persistence
    /// (ephemeral one-shot clients).
    pub(super) state_dir: Option<PathBuf>,
    /// Shared inbox subscription (present iff a relay is configured). One instance
    /// so its proof-of-possession session token is reused across polls and acks.
    pub(super) inbox: Option<Arc<InboxSubscription>>,
    /// The local user's self-chosen display name, advertised inside every outgoing
    /// offer ([`Offer::sender_name`]). Empty = don't advertise a name. Set once at
    /// startup via [`TransferManager::set_display_name`] before any `send_to`.
    pub(super) display_name: Mutex<String>,
}

impl Inner {
    pub(super) fn emit(&self, ev: ManagerEvent) {
        // No subscribers is fine — the state map is the source of truth.
        let _ = self.events.send(ev);
    }

    /// Set a **terminal-or-paused** status: also drops the cancel token, since the
    /// running task is ending (finished, deposited, or paused — a resume installs a
    /// fresh token).
    pub(super) fn set_status(&self, id: u64, status: TransferStatus) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.status = status;
        }
        self.cancels.lock().unwrap().remove(&id);
    }

    /// Set a **live** status (Active / Waiting) *without* touching the cancel token,
    /// so a still-running transfer stays cancellable across status changes.
    pub(super) fn set_status_live(&self, id: u64, status: TransferStatus) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.status = status;
        }
    }

    pub(super) fn set_progress(&self, id: u64, transferred: u64) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.transferred = transferred;
        }
    }

    pub(super) fn set_peer(&self, id: u64, peer: PublicId) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.peer = Some(peer);
        }
    }

    pub(super) fn set_swarm(&self, id: u64, peers: usize, from_peers: u64) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.swarm_peers = peers;
            t.pieces_from_peers = from_peers;
        }
    }

    pub(super) fn set_download_peers(&self, id: u64, count: usize) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.download_peers = count;
        }
    }

    /// Rebuild an owned identity for a spawned task (avoids borrowing `self.me`
    /// across the task's awaits). Cheap: it's a 32-byte key.
    pub(super) fn identity(&self) -> Result<Identity> {
        Identity::from_secret_bytes(&self.me.secret_bytes())
    }
}
