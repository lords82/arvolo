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

/// Drop a waiting code the moment the row stops waiting.
///
/// Both status setters call it, so the code cannot outlive the status it belongs
/// to: a transfer that waited, then went active, would otherwise still be
/// carrying "offered" for any client that read the field without checking the
/// status first. Cheap insurance against a stale label — the sort that shows up
/// as a row explaining itself with last week's reason.
fn clear_waiting_code(t: &mut Transfer) {
    if !matches!(t.status, TransferStatus::Waiting(_)) {
        t.waiting_code.clear();
        t.waiting_detail.clear();
    }
}

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
    /// Which of our own devices this is for, when the sender named one. Lives
    /// here rather than being threaded through the delivery functions because it
    /// belongs to the *send*, and both the live attempt and the mailbox fallback
    /// have to stamp the same value on the offer they post.
    pub(super) target: Option<crate::sync::DeviceId>,
    /// Where the recipient listens, when that is not where we do. Kept here for
    /// the same reason as `target`: it belongs to the send, and a pause/resume or
    /// a restart has to route to the same place the first attempt did.
    pub(super) relay: Option<String>,
    /// Never the mailbox: the loop waits for them to be online and goes direct,
    /// or keeps waiting. See `TransferManager::send_to`.
    pub(super) live_only: bool,
    /// Flipped by [`TransferManager::pause`] so the running loop, once its token is
    /// cancelled, knows to pause (keep the transfer) rather than cancel (drop it).
    pub(super) pause_flag: Arc<std::sync::atomic::AtomicBool>,
    /// Flipped by [`TransferManager::deposit_now`]: stop waiting for the recipient
    /// to accept and park the file on the relay at the next turn of the loop.
    ///
    /// Separate from `pause_flag` because it means the opposite thing about the
    /// send — this one is "finish it by the other route", not "stop". It also
    /// deliberately does **not** cancel the send's own token, which would pause it.
    pub(super) deposit_now: Arc<std::sync::atomic::AtomicBool>,
    /// The cancellation token of the live attempt currently serving, when one is.
    ///
    /// Published here so a `deposit_now` can end that attempt without waiting out
    /// the watchdog — the same token the watchdog itself cancels to force the
    /// fallback, so the loop around it already does the right thing. `None`
    /// between attempts.
    pub(super) attempt: Arc<Mutex<Option<CancellationToken>>>,
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
    /// Inbox offers whose ack waits on the download they started: transfer id →
    /// offer id. A mailbox pickup acks only once its bytes are on disk (or the
    /// deposit is known gone), so that an app killed mid-fetch finds the offer
    /// still in its inbox and the deposit still on the relay — see
    /// `Mailbox::fetch_plan_held` in the relay for the other half.
    pub(super) deferred_acks: Mutex<HashMap<u64, String>>,
    /// Delivery state for active/paused `send --to` transfers (for pause/resume).
    pub(super) held: Mutex<HashMap<u64, Held>>,
    /// What each live chunked download is fetching, as
    /// [`ChunkTicket::content_id`](crate::chunked::ChunkTicket::content_id) — so an
    /// offer that arrives about a file already coming down can find it instead of
    /// asking the user again.
    ///
    /// Keyed by transfer id and not by content: the same content can legitimately be
    /// downloaded twice (two output paths), and a map keyed the other way would
    /// silently drop one of them. Entries go in when a chunked download starts (or
    /// is restored paused) and out when it reaches a terminal state — a *completed*
    /// download deliberately stops matching, because the user may since have deleted
    /// the file and "you already have this" is a different feature.
    pub(super) download_content: Mutex<HashMap<u64, String>>,
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
    /// The platform's say in where a finished download comes to rest (see
    /// [`SavePublisher`]). Unset on every desktop, where a download is already
    /// where it belongs; set once at startup by a mobile shell.
    ///
    /// A `OnceLock` rather than a `Mutex` because it is written once before any
    /// transfer runs and read on every one that finishes: there is nothing to
    /// serialize, and a publisher that could change mid-download would mean two
    /// halves of one file going to two different places.
    pub(super) publisher: std::sync::OnceLock<Arc<dyn SavePublisher>>,
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

    /// Record what download `id` is fetching. A ticket that doesn't decode is simply
    /// not indexed: nothing can match it either.
    pub(super) fn note_download_content(&self, id: u64, ticket: &str) {
        if let Ok(t) = crate::chunked::ChunkTicket::decode(ticket) {
            self.download_content
                .lock()
                .unwrap()
                .insert(id, t.content_id());
        }
    }

    /// See [`TransferManager::download_of_same_content`], which is this with a doc
    /// comment. Here because the inbox poll callback holds an `Inner`, not a manager.
    pub(super) fn download_of_same_content(
        &self,
        ticket: &str,
        from: Option<&PublicId>,
    ) -> Option<u64> {
        let want = crate::chunked::ChunkTicket::decode(ticket)
            .ok()?
            .content_id();
        let ids: Vec<u64> = self
            .download_content
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, c)| **c == want)
            .map(|(id, _)| *id)
            .collect();
        let transfers = self.transfers.lock().unwrap();
        let mut best: Option<(u8, u64)> = None;
        for id in ids {
            let Some(t) = transfers.get(&id) else {
                continue;
            };
            if let (Some(from), Some(peer)) = (from, t.peer.as_ref()) {
                if peer.to_bytes() != from.to_bytes() {
                    continue;
                }
            }
            // A live download first, a paused one only if there is no live one:
            // deterministic, and it prefers the row that can act on the news.
            let rank = match t.status {
                TransferStatus::Active | TransferStatus::Preparing | TransferStatus::Waiting(_) => {
                    0
                }
                TransferStatus::Paused(_) => 1,
                _ => continue,
            };
            if best.map(|(r, bid)| (rank, id) < (r, bid)).unwrap_or(true) {
                best = Some((rank, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// The parked offers that carry the same content as `ticket` — the copies a
    /// sender left behind before it kept one offer standing, and the ones a relay
    /// withdrawal cannot reach because they are already in this map. Removed from
    /// `pending` and returned, so the caller can ack them off the relay too.
    pub(super) fn take_offers_for_same_content(&self, ticket: &str) -> Vec<String> {
        let Ok(want) = crate::chunked::ChunkTicket::decode(ticket).map(|t| t.content_id()) else {
            return Vec::new();
        };
        let mut pending = self.pending.lock().unwrap();
        let ghosts: Vec<String> = pending
            .iter()
            .filter(|(_, o)| {
                crate::chunked::ChunkTicket::decode(&o.offer.ticket)
                    .map(|t| t.content_id() == want)
                    .unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ghosts {
            pending.remove(id);
        }
        ghosts
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
            clear_waiting_code(t);
        }
        self.cancels.lock().unwrap().remove(&id);
    }

    /// Set a **live** status (Active / Waiting) *without* touching the cancel token,
    /// so a still-running transfer stays cancellable across status changes.
    pub(super) fn set_status_live(&self, id: u64, status: TransferStatus) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.status = status;
            clear_waiting_code(t);
        }
    }

    /// End a transfer with a reason: the status, the code beside it, and the
    /// event — in that order, so nothing reads the event before the row says
    /// the same thing. The one door every failure goes through.
    pub(super) fn fail(&self, id: u64, reason: &super::FailReason) {
        let text = reason.text();
        self.set_status(id, TransferStatus::Failed(text.clone()));
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.fail_code = reason.code().to_string();
            t.fail_detail = reason.detail().unwrap_or("").to_string();
        }
        self.emit(ManagerEvent::Failed {
            id,
            error: text,
            code: reason.code().to_string(),
        });
    }

    /// Record why a send is waiting, as the code a client translates. Set right
    /// after the status, which is why it does not clear itself.
    pub(super) fn set_waiting_code(&self, id: u64, code: &str, detail: &str) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.waiting_code = code.to_string();
            t.waiting_detail = detail.to_string();
        }
    }

    /// Remember where a completed receive was saved, so the row can still offer
    /// to open it long after the event that announced it.
    pub(super) fn set_path(&self, id: u64, path: std::path::PathBuf) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.path = Some(path);
        }
    }

    /// Remember the platform handle a publisher gave a completed receive, for the
    /// rows whose file cannot be named by path (see [`SavedLocation::Uri`]).
    pub(super) fn set_saved_uri(&self, id: u64, uri: String) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.saved_uri = Some(uri);
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
