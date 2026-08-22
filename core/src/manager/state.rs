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
    pub(super) source: crate::source::SendSource,
    pub(super) name: String,
    pub(super) archive: bool,
    /// Optional sender note to attach to the offer.
    pub(super) note: String,
    /// Flipped by [`TransferManager::pause`] so the running loop, once its token is
    /// cancelled, knows to pause (keep the transfer) rather than cancel (drop it).
    pub(super) pause_flag: Arc<std::sync::atomic::AtomicBool>,
    /// The content key, transport seed and chunk digests this send has already paid
    /// for — here rather than in the delivery task, because the task is what a pause
    /// destroys.
    ///
    /// Re-running the pass does not just cost the minute it takes: it mints a fresh
    /// key and a fresh seed, so the recipient is looking at a different send, under a
    /// node id nobody is serving. What they had in flight dies, and what they get
    /// instead is another offer to approve by hand.
    ///
    /// The key therefore sits in memory for as long as a paused send is kept. It
    /// already did for a running one, and `SendRecord` keeps a plain-key ticket on
    /// disk for anonymous shares — but it is worth knowing rather than discovering.
    pub(super) prep: crate::flow::PrepSlot,
}

pub(super) struct Inner {
    pub(super) next_id: AtomicU64,
    pub(super) transfers: Mutex<HashMap<u64, Transfer>>,
    pub(super) cancels: Mutex<HashMap<u64, CancellationToken>>,
    /// Per-download abort intent (see `flow::RecvCancel`): defaults true, and
    /// `pause` flips it to false *before* cancelling so the fetch slips away
    /// like a drop (tail kept warm) instead of telling the sender to stop.
    pub(super) recv_aborts: Mutex<HashMap<u64, Arc<std::sync::atomic::AtomicBool>>>,
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
    /// This device's local id, stamped on every offer we post
    /// ([`Offer::origin`](crate::presence::Offer::origin)) and handed to the inbox
    /// so it can drop our own offers back out of our own poll. `None` for a client
    /// that never set one — the engine works, it just cannot tell its own offers
    /// from another of its devices'. Set at startup via
    /// [`TransferManager::set_device_id`], like the display name.
    pub(super) device_id: Mutex<Option<crate::sync::DeviceId>>,
}

impl Inner {
    pub(super) fn emit(&self, ev: ManagerEvent) {
        // No subscribers is fine — the state map is the source of truth.
        let _ = self.events.send(ev);
    }

    /// Set a **terminal-or-paused** status: also drops the cancel token, since the
    /// running task is ending (finished, deposited, or paused — a resume installs a
    /// fresh token).
    ///
    /// Passing a *live* status here is always a bug, and a quiet one: the row keeps
    /// running with no way left to stop it. That is not hypothetical — one call site
    /// marked a send `Active` through this function and made every live P2P send
    /// uncancellable from the moment it started serving. Hence the assertion: the
    /// mistake is invisible in production and trips immediately in a test.
    pub(super) fn set_status(&self, id: u64, status: TransferStatus) {
        debug_assert!(
            !matches!(
                status,
                TransferStatus::Active | TransferStatus::Preparing | TransferStatus::Waiting(_)
            ),
            "set_status drops the cancel token — a live status needs set_status_live"
        );
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

    /// Remember where a completed receive was saved, so the row can still offer
    /// to open it long after the event that announced it.
    pub(super) fn set_path(&self, id: u64, path: std::path::PathBuf) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.path = Some(path);
        }
    }

    /// Record how far a deposited send's inbox offer has got, as last reported.
    pub(super) fn set_offer_status(&self, id: u64, status: &str) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.offer_status = status.to_string();
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

    /// Install a share's counters (on registering a resumed share, from its
    /// sidecar record) so a restart doesn't report a week-old share as untouched.
    pub(super) fn set_share_stats(&self, id: u64, rec: &super::records::ShareRecord) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.copies_served = rec.copies_served;
            t.bytes_served = rec.bytes_served;
            t.last_pickup = rec.last_pickup;
            t.from_download = rec.from_download;
            t.share_started = rec.started;
        }
    }

    /// Read a share's counters back out, to persist them.
    pub(super) fn share_stats(&self, id: u64) -> super::records::ShareRecord {
        let transfers = self.transfers.lock().unwrap();
        let Some(t) = transfers.get(&id) else {
            return Default::default();
        };
        super::records::ShareRecord {
            copies_served: t.copies_served,
            bytes_served: t.bytes_served,
            last_pickup: t.last_pickup,
            from_download: t.from_download,
            started: t.share_started,
        }
    }

    /// Add `bytes` to what this share has uploaded.
    pub(super) fn add_bytes_served(&self, id: u64, bytes: u64) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.bytes_served = t.bytes_served.saturating_add(bytes);
        }
    }

    /// One receiver got the whole file: count the copy and stamp the moment.
    pub(super) fn record_pickup(&self, id: u64, at: u64) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.copies_served = t.copies_served.saturating_add(1);
            t.last_pickup = at;
        }
    }

    /// Write a share's counters to its sidecar.
    ///
    /// Called on a completed pickup, not on every progress tick: a share can serve
    /// a large file to several peers at once, and a write per chunk would spend the
    /// disk on a number nobody reads that often. What that costs is the bytes
    /// served since the last pickup, if the daemon dies without one — the copies
    /// count and the timestamps, which are what the view is actually built on, are
    /// written exactly when they change.
    pub(super) fn persist_share_stats(&self, id: u64) {
        if let Some(dir) = &self.state_dir {
            super::records::persist_share(dir, id, &self.share_stats(id));
        }
    }

    /// Attach (or clear) the live pairing code a send is reachable by, so a UI can
    /// show it without having witnessed the command that created it — including
    /// after a daemon restart, where the code comes back but the terminal that
    /// printed it is long gone.
    pub(super) fn set_code(&self, id: u64, code: Option<String>) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.code = code;
        }
    }

    /// Rebuild an owned identity for a spawned task (avoids borrowing `self.me`
    /// across the task's awaits). Cheap: it's a 32-byte key.
    pub(super) fn identity(&self) -> Result<Identity> {
        Identity::from_secret_bytes(&self.me.secret_bytes())
    }
}
