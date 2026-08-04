//! Transfer manager: a persistent, multi-transfer engine for a UI client.
//!
//! The CLI's one-shot `send`/`recv` handle exactly one payload and exit. A
//! desktop client instead stays open and runs **many** transfers at once — some
//! outgoing, some incoming — while listening for [`crate::presence`] offers. This
//! module wraps the existing [`crate::flow`] primitives in a long-lived owner:
//!
//! * one `tokio` task per transfer (send + receive run concurrently),
//! * a per-transfer [`CancellationToken`] so any one can be stopped,
//! * a [`broadcast`] event stream ([`ManagerEvent`]) a front-end subscribes to,
//! * an inbox subscription that surfaces incoming offers and lets the UI
//!   [`accept`](TransferManager::accept_offer) them — the ticket stays hidden and
//!   the download is transparent.
//!
//! It is intentionally front-end agnostic: the CLI `listen`/`push` commands drive
//! it today; a Tauri/egui GUI would drive the same API tomorrow.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::code;
use crate::crypto::{Identity, PublicId};
use crate::flow::{self, RecvEvent};
use crate::presence::{self, InboxSubscription};
use crate::transfer::RelayChoice;

mod records;
mod state;
mod work;

use records::*;
use state::*;
use work::*;

/// Direction of a transfer, from this client's point of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// We are sending to a peer.
    Send,
    /// We are receiving from a peer.
    Recv,
}

/// Lifecycle state of a transfer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferStatus {
    /// In progress (or, for a send, serving and awaiting the receiver).
    Active,
    /// Finished successfully (for a send: the recipient received it).
    Completed,
    /// An offline send: handed to the relay mailbox, delivery not yet confirmed.
    Deposited,
    /// A send that could not be handed off yet (the relay refused it, was
    /// unreachable, or errored): the daemon is holding it and will keep trying —
    /// delivering live P2P as soon as the recipient is online, and re-trying the
    /// relay on a slow interval. The string is a short human reason.
    Waiting(String),
    /// A send that is **paused** — not actively trying — awaiting a user decision
    /// (`resume` or `cancel`). Reached when the user pauses it, or automatically
    /// when it couldn't be delivered for a long time. Durable: it is restored as
    /// paused after a daemon restart. The string is a short human reason.
    Paused(String),
    /// Stopped before finishing (e.g. the user cancelled).
    Cancelled,
    /// Ended with an error.
    Failed(String),
}

/// A snapshot of one transfer, for a UI list.
#[derive(Clone, Debug)]
pub struct Transfer {
    pub id: u64,
    pub direction: Direction,
    /// The peer: recipient (send) or authenticated sender (receive), if known.
    pub peer: Option<PublicId>,
    pub name: String,
    pub total_size: u64,
    pub transferred: u64,
    pub status: TransferStatus,
    /// Swarm metrics for a receive: peers currently known, and pieces pulled from
    /// peers (both 0 for a non-swarm transfer).
    pub swarm_peers: usize,
    pub pieces_from_peers: u64,
    /// For a send: distinct peers currently downloading from us right now (0 for
    /// a receive, or a send nobody is pulling).
    pub download_peers: usize,
    /// Unix seconds when this transfer began. A UI groups history by day, and only
    /// the engine knows the real time: a client that stamps rows when it first sees
    /// them files every transfer under "today" the moment it restarts.
    pub created: u64,
    /// The short pairing code this send is currently reachable by, if the daemon
    /// is hosting one. Cleared when the code retires; the send itself carries on,
    /// since the ticket is a separate capability.
    pub code: Option<String>,
}

/// What an offline send actually left on the relay, carried by
/// [`ManagerEvent::Deposited`].
///
/// It rides *with* the event on purpose. A subscriber told only "id N was
/// deposited" would have to read the engine's own record back to learn how to
/// withdraw it — which races the write that follows the emit, and finds nothing at
/// all on a manager without a `state_dir`. Handing over the capability at the
/// moment it comes into being avoids both.
///
/// This is in-process only. It holds `revoke_token`, a sender-only secret, which
/// is why the IPC mirror (`arvolo_ipc`'s `EventDto`) keeps nothing from here but
/// the id: subscribers to a daemon socket must not be handed the withdrawal
/// capability for someone else's deposit.
#[derive(Clone, Debug)]
pub struct DepositInfo {
    pub relay: String,
    pub claim: String,
    /// Sender-only secret authorizing removal of the blob from the relay.
    pub revoke_token: String,
    pub name: String,
    pub size: u64,
    /// Unix seconds when the relay drops the blob on its own. Absolute, not a TTL:
    /// a restored deposit keeps its original deadline rather than winning a fresh one.
    pub expires: u64,
    /// The download cap it was deposited under.
    pub max: u32,
    /// The recipient it is sealed to.
    pub recipient: Option<PublicId>,
    /// The offer left in the recipient's inbox pointing at this blob, and the token
    /// that retracts it — the other half of a withdrawal, alongside `revoke_token`.
    ///
    /// The engine keeps its own copy and retracts them itself when it cancels, so
    /// handing them over is redundant *while a daemon is running*. It isn't when one
    /// isn't: a deposit made by the daemon can be withdrawn later from a bare CLI,
    /// with the engine gone, and without these that withdrawal could only kill the
    /// blob — leaving the recipient an arrival that can never be fetched. A receipt
    /// is only worth keeping if it is sufficient on its own.
    pub offer_id: String,
    pub poster_token: String,
}

/// Events emitted as transfers and offers progress. Cloneable for [`broadcast`].
#[derive(Clone, Debug)]
pub enum ManagerEvent {
    /// An incoming offer arrived (show the accept/reject popup). `id` is the
    /// handle to pass to [`accept_offer`](TransferManager::accept_offer) /
    /// [`reject_offer`](TransferManager::reject_offer).
    OfferReceived {
        id: String,
        from: PublicId,
        name: String,
        size: u64,
        /// An optional sender's note attached to the transfer (empty if none).
        note: String,
        /// The sender's self-advertised display name (empty if none). A petname
        /// claim carried inside the sealed offer — never an authenticated identity.
        sender_name: String,
    },
    /// A transfer started.
    Started {
        id: u64,
        direction: Direction,
        name: String,
        total_size: u64,
    },
    /// Progress update (cumulative bytes moved).
    Progress {
        id: u64,
        transferred: u64,
        total_size: u64,
    },
    /// A transfer finished successfully; `path` is set for a completed receive.
    Completed { id: u64, path: Option<PathBuf> },
    /// An offline send was deposited to the mailbox (awaiting the recipient). It
    /// may later transition to [`Completed`](ManagerEvent::Completed) once the
    /// recipient fetches it (within the confirmation window). `info` describes what
    /// was left on the relay, so a front-end can record a withdrawable receipt.
    Deposited { id: u64, info: DepositInfo },
    /// A send is being held for later delivery because the relay couldn't take it
    /// (refused / unreachable / errored). The daemon keeps trying: live P2P when
    /// the recipient appears, and the relay again on a slow interval. `reason` is
    /// a short human explanation.
    Waiting { id: u64, reason: String },
    /// A send was paused (by the user, or automatically after a long failure to
    /// deliver). It stays put until `resume`d or `cancel`led. `reason` explains why.
    Paused { id: u64, reason: String },
    /// A transfer failed.
    Failed { id: u64, error: String },
    /// A transfer was cancelled.
    Cancelled { id: u64 },
    /// A short pairing code is live and listening for receivers. Raised on the
    /// first claim and again after a restart, so a front-end can show the code
    /// without having to have seen the command that created it.
    CodeReady { id: u64, code: String },
    /// A receiver proved it knows the code and has been handed the ticket.
    CodePaired { id: u64, done: u32 },
    /// The code stopped working (used up, guessed at too often, cancelled, or the
    /// relay let the slot go). The send itself carries on — the ticket is a
    /// separate capability, and whoever already has it keeps downloading.
    CodeClosed { id: u64, reason: String },
    /// The local address book changed (a contact added/removed/renamed, a verified
    /// or trusted mark set or cleared). Payload-free on purpose: the book lives on
    /// disk, outside the engine, so this is a "refetch your contacts" nudge for
    /// attached front-ends — not a description of the change. The daemon raises it
    /// when it notices the book files move under it, whoever wrote them.
    ContactsChanged,
}

/// A persistent, multi-transfer engine bound to one local identity.
#[derive(Clone)]
pub struct TransferManager {
    inner: Arc<Inner>,
}

impl TransferManager {
    /// Create a manager for `me`, using `relay` for presence/offers (required for
    /// [`send_to`](Self::send_to) and [`spawn_inbox`](Self::spawn_inbox)) and
    /// saving accepted downloads under `download_dir` by default.
    pub fn new(me: Identity, relay: Option<String>, download_dir: PathBuf) -> Self {
        Self::with_state_dir(me, relay, download_dir, None)
    }

    /// Like [`new`](Self::new) but with a `state_dir` for persisting resumable
    /// downloads (the daemon passes one; ephemeral clients pass `None`).
    pub fn with_state_dir(
        me: Identity,
        relay: Option<String>,
        download_dir: PathBuf,
        state_dir: Option<PathBuf>,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        let inbox = relay
            .as_ref()
            .map(|r| Arc::new(InboxSubscription::new(r.clone(), &me)));
        if let Some(d) = &state_dir {
            let _ = std::fs::create_dir_all(d);
        }
        Self {
            inner: Arc::new(Inner {
                next_id: AtomicU64::new(1),
                transfers: Mutex::new(HashMap::new()),
                cancels: Mutex::new(HashMap::new()),
                held: Mutex::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                events,
                me,
                relay,
                client: reqwest::Client::new(),
                download_dir,
                state_dir,
                inbox,
                display_name: Mutex::new(String::new()),
            }),
        }
    }

    /// Set the local user's display name, advertised inside every outgoing offer.
    /// Call once at startup (from the client's config); empty means no name is
    /// advertised. It is a petname claim, not an authenticated identity — see
    /// [`Offer::sender_name`](crate::presence::Offer::sender_name).
    pub fn set_display_name(&self, name: String) {
        *self.inner.display_name.lock().unwrap() = name;
    }

    /// Subscribe to the manager's event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<ManagerEvent> {
        self.inner.events.subscribe()
    }

    /// Raise [`ContactsChanged`](ManagerEvent::ContactsChanged) on the event stream.
    /// The address book is the client's, not the engine's, so the client tells us
    /// when it moved; we only carry the notice to every subscriber.
    pub fn notify_contacts_changed(&self) {
        self.inner.emit(ManagerEvent::ContactsChanged);
    }

    /// This client's public id.
    pub fn public_id(&self) -> PublicId {
        self.inner.me.public()
    }

    /// A snapshot of all known transfers (any status).
    pub fn list(&self) -> Vec<Transfer> {
        let mut out: Vec<Transfer> = self
            .inner
            .transfers
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        // Newest first: the id is a monotonic counter, so higher = more recent.
        out.sort_by_key(|t| std::cmp::Reverse(t.id));
        out
    }

    /// A snapshot of one transfer by id, if still tracked.
    pub fn get(&self, id: u64) -> Option<Transfer> {
        self.inner.transfers.lock().unwrap().get(&id).cloned()
    }

    /// Drop every finished (completed/failed/cancelled) transfer from the list and
    /// return how many went — the bulk twin of [`remove`](Self::remove).
    ///
    /// "Finished" is defined by what it *keeps*: anything that still has a future.
    /// A **Deposited** send is the one that catches people out — it looks done, and
    /// isn't: its blob is sitting on a relay waiting to be collected, and the pickup
    /// poll can still turn it into Completed. Waiting and Paused likewise. Only rows
    /// that will never change again are dropped.
    ///
    /// This forgets the row, not the deed: the history log is written separately and
    /// is untouched here.
    pub fn clear_finished(&self) -> usize {
        let mut transfers = self.inner.transfers.lock().unwrap();
        let before = transfers.len();
        transfers.retain(|_, t| {
            matches!(
                t.status,
                TransferStatus::Active
                    | TransferStatus::Deposited
                    | TransferStatus::Waiting(_)
                    | TransferStatus::Paused(_)
            )
        });
        before - transfers.len()
    }

    /// Drop one **finished** (completed/failed/cancelled) transfer from the list —
    /// the per-row "remove from history" a UI offers next to
    /// [`clear_finished`](Self::clear_finished). Refuses (returns `false`) for
    /// anything still in flight — including a Deposited send awaiting its pickup
    /// confirmation — so a UI can't orphan a live task.
    pub fn remove(&self, id: u64) -> bool {
        let mut transfers = self.inner.transfers.lock().unwrap();
        let terminal = transfers
            .get(&id)
            .map(|t| {
                matches!(
                    t.status,
                    TransferStatus::Completed
                        | TransferStatus::Failed(_)
                        | TransferStatus::Cancelled
                )
            })
            .unwrap_or(false);
        if terminal {
            transfers.remove(&id);
        }
        terminal
    }

    fn register(
        &self,
        direction: Direction,
        peer: Option<PublicId>,
        name: String,
        total_size: u64,
    ) -> (u64, CancellationToken) {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        {
            let mut transfers = self.inner.transfers.lock().unwrap();
            transfers.insert(
                id,
                Transfer {
                    id,
                    created: unix_now(),
                    direction,
                    peer,
                    name: name.clone(),
                    total_size,
                    transferred: 0,
                    status: TransferStatus::Active,
                    swarm_peers: 0,
                    pieces_from_peers: 0,
                    download_peers: 0,
                    code: None,
                },
            );
            // Keep the map from growing without bound over a long session: once
            // there are many finished entries, drop the oldest (lowest ids).
            prune_finished(&mut transfers, MAX_FINISHED_RETAINED);
        }
        self.inner
            .cancels
            .lock()
            .unwrap()
            .insert(id, cancel.clone());
        self.inner.emit(ManagerEvent::Started {
            id,
            direction,
            name,
            total_size,
        });
        (id, cancel)
    }

    /// Cancel a transfer by id (no-op if it already finished).
    pub fn cancel(&self, id: u64) {
        let status = self
            .inner
            .transfers
            .lock()
            .unwrap()
            .get(&id)
            .map(|t| t.status.clone());
        match status {
            // A paused send has no running loop to cancel — end it directly.
            Some(TransferStatus::Paused(_)) => {
                finish(&self.inner, id, true, Ok(None));
                return;
            }
            // An awaiting-pickup deposit has no running loop either, and the file
            // is sitting on the relay: withdraw it there before ending the row,
            // or "cancel" would only hide it while the recipient could still fetch.
            Some(TransferStatus::Deposited) => {
                cancel_deposited(self.inner.clone(), id);
                return;
            }
            _ => {}
        }
        if let Some(c) = self.inner.cancels.lock().unwrap().get(&id) {
            c.cancel();
        }
    }

    // ---- presence ---------------------------------------------------------

    /// Is `contact` currently online (a live presence beacon on the relay)? Any
    /// error or missing relay reads as offline.
    pub async fn is_online(&self, contact: &PublicId) -> bool {
        let Some(relay) = &self.inner.relay else {
            return false;
        };
        presence::check_online(&self.inner.client, relay, contact)
            .await
            .unwrap_or(false)
    }

    // ---- sending ----------------------------------------------------------

    /// Send `payload` to `recipient`, robustly. Registers the transfer and returns
    /// its id immediately; delivery runs in the background (see `deliver_to`):
    /// live P2P whenever the recipient is online, else the relay mailbox. If the
    /// relay can't take it (too large / unreachable / error) the send is *held*
    /// (`Waiting`) and keeps retrying rather than failing.
    pub async fn send_to(
        &self,
        recipient: &PublicId,
        payload: PathBuf,
        name: String,
        archive: bool,
        note: String,
    ) -> Result<u64> {
        let relay = self.inner.relay.clone().context(
            "a relay is required to send to a contact (set ARVOLO_RELAY or config `relay`)",
        )?;
        let size = std::fs::metadata(&payload).map(|m| m.len()).unwrap_or(0);
        let (id, cancel) =
            self.register(Direction::Send, Some(recipient.clone()), name.clone(), size);
        let pause_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.inner.held.lock().unwrap().insert(
            id,
            Held {
                recipient: recipient.clone(),
                payload: payload.clone(),
                name: name.clone(),
                archive,
                note: note.clone(),
                pause_flag: pause_flag.clone(),
            },
        );
        persist_held_record(&self.inner, id, false, "");
        tokio::spawn(deliver_to(
            self.inner.clone(),
            id,
            cancel,
            relay,
            recipient.clone(),
            payload,
            name,
            archive,
            note,
            pause_flag,
        ));
        Ok(id)
    }

    /// Pause an in-progress transfer (Active or Waiting): stop working on it and hold
    /// it as `Paused` until `resume`d or `cancel`led. Returns whether it was paused.
    ///
    /// Works for a live **`send --to`** (held-state send) and for a resumable
    /// **download** (a chunked receive with a resume record). Both keep their partial
    /// state on disk. No-op for anything else — an anonymous ticket serve, a one-shot
    /// mailbox fetch, an awaiting-pickup deposit.
    pub fn pause(&self, id: u64) -> bool {
        let (can, is_recv) = self
            .inner
            .transfers
            .lock()
            .unwrap()
            .get(&id)
            .map(|t| {
                (
                    matches!(
                        t.status,
                        TransferStatus::Active | TransferStatus::Waiting(_)
                    ),
                    t.direction == Direction::Recv,
                )
            })
            .unwrap_or((false, false));
        if !can {
            return false;
        }

        // A held send: flag it and wake its delivery loop, which parks it as Paused.
        if let Some(h) = self.inner.held.lock().unwrap().get(&id).cloned() {
            h.pause_flag.store(true, Ordering::Relaxed);
            if let Some(c) = self.inner.cancels.lock().unwrap().get(&id) {
                c.cancel();
            }
            return true;
        }

        // A resumable download: mark it paused on disk *before* cancelling the fetch,
        // so the task settling from the cancel sees the marker and parks it as Paused
        // (keeping the partial + record) instead of ending it as Cancelled. The marker
        // also survives a restart, so `resume_incomplete` leaves it paused.
        if is_recv {
            if let Some(dir) = &self.inner.state_dir {
                if download_record_path(dir, id).exists() {
                    mark_paused(dir, id);
                    if let Some(c) = self.inner.cancels.lock().unwrap().get(&id) {
                        c.cancel();
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Resume a `Paused` transfer: a held **send** starts delivering again; a paused
    /// **download** restarts its fetch under the same id, continuing from the partial
    /// file on disk. Returns whether it resumed.
    pub fn resume(&self, id: u64) -> bool {
        let paused = self
            .inner
            .transfers
            .lock()
            .unwrap()
            .get(&id)
            .map(|t| matches!(t.status, TransferStatus::Paused(_)))
            .unwrap_or(false);
        if !paused {
            return false;
        }

        // A held send: re-arm its cancel token and re-spawn the delivery loop.
        let held = self.inner.held.lock().unwrap().get(&id).cloned();
        if let (Some(h), Some(relay)) = (held, self.inner.relay.clone()) {
            h.pause_flag.store(false, Ordering::Relaxed);
            let cancel = CancellationToken::new();
            self.inner
                .cancels
                .lock()
                .unwrap()
                .insert(id, cancel.clone());
            self.inner.set_status_live(id, TransferStatus::Active);
            self.emit_progress_now(id);
            persist_held_record(&self.inner, id, false, "");
            tokio::spawn(deliver_to(
                self.inner.clone(),
                id,
                cancel,
                relay,
                h.recipient,
                h.payload,
                h.name,
                h.archive,
                h.note,
                h.pause_flag,
            ));
            return true;
        }

        // A paused download: clear the marker and re-spawn the fetch under the same id
        // with a fresh cancel token — recv_chunked resumes from the partial file.
        if let Some(dir) = self.inner.state_dir.clone() {
            if let Some(rec) = load_download(&dir, id) {
                clear_paused(&dir, id);
                let cancel = CancellationToken::new();
                self.inner
                    .cancels
                    .lock()
                    .unwrap()
                    .insert(id, cancel.clone());
                self.inner.set_status_live(id, TransferStatus::Active);
                self.emit_progress_now(id);
                self.spawn_receive(id, rec.ticket, PathBuf::from(rec.out_path), None, cancel);
                return true;
            }
        }
        false
    }

    /// Emit a `Progress` event with the row's current bytes. `set_status_live` changes
    /// status silently (no event), so on **resume** a UI would keep showing the row as
    /// paused until the first fetched chunk. This nudges it back to active at once —
    /// a progress event means the transfer is moving.
    fn emit_progress_now(&self, id: u64) {
        let snapshot = self
            .inner
            .transfers
            .lock()
            .unwrap()
            .get(&id)
            .map(|t| (t.transferred, t.total_size));
        if let Some((transferred, total_size)) = snapshot {
            self.inner.emit(ManagerEvent::Progress {
                id,
                transferred,
                total_size,
            });
        }
    }

    /// Re-register a `send --to` restored from disk after a daemon restart: paused
    /// ones come back `Paused` (awaiting the user); active ones resume delivering.
    /// Restore an awaiting-pickup mailbox deposit after a restart: the row comes
    /// back as Deposited and the pickup-confirmation poll resumes. A record whose
    /// relay-side TTL has already lapsed is dropped silently (the blob is gone).
    fn restore_deposited(&self, rec: DepositedRecord) {
        if unix_now() >= rec.expires {
            // Delete it too: skipping alone left the file to be re-read and
            // re-skipped on every start from here to eternity.
            if let Some(dir) = &self.inner.state_dir {
                remove_deposited(dir, rec.id);
            }
            return;
        }
        let peer = PublicId::from_bytes(&rec.recipient).ok();
        let (id, _cancel) = self.register(Direction::Send, peer, rec.name.clone(), rec.size);
        // No running task to cancel for a deposited send.
        self.inner.cancels.lock().unwrap().remove(&id);
        // spawn_offline_confirm re-sets status/progress, re-persists under the
        // fresh id, and resumes the pickup-confirmation poll. The revoke/retract
        // tokens ride along so a restored deposit stays cancellable.
        let relay = rec.relay.clone();
        spawn_offline_confirm(
            &self.inner,
            id,
            relay,
            DepositOutcome {
                size: rec.size,
                claim: rec.claim,
                revoke_token: rec.revoke_token,
                offer_id: rec.offer_id,
                poster_token: rec.poster_token,
            },
            rec.expires,
        );
    }

    fn restore_sendto(&self, rec: SendToRecord) {
        let Ok(recipient) = PublicId::from_bytes(&rec.recipient) else {
            return;
        };
        let Some(relay) = self.inner.relay.clone() else {
            return; // can't deliver without a relay; drop the stale record
        };
        let payload = PathBuf::from(&rec.payload);
        let size = std::fs::metadata(&payload).map(|m| m.len()).unwrap_or(0);
        let (id, cancel) = self.register(
            Direction::Send,
            Some(recipient.clone()),
            rec.name.clone(),
            size,
        );
        let pause_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.inner.held.lock().unwrap().insert(
            id,
            Held {
                recipient: recipient.clone(),
                payload: payload.clone(),
                name: rec.name.clone(),
                archive: rec.archive,
                note: rec.note.clone(),
                pause_flag: pause_flag.clone(),
            },
        );
        if rec.paused {
            // Restore paused: don't run; drop the fresh token, mark Paused.
            self.inner.cancels.lock().unwrap().remove(&id);
            persist_held_record(&self.inner, id, true, &rec.reason);
            self.inner
                .set_status(id, TransferStatus::Paused(rec.reason.clone()));
            self.inner.emit(ManagerEvent::Paused {
                id,
                reason: rec.reason,
            });
        } else {
            persist_held_record(&self.inner, id, false, "");
            tokio::spawn(deliver_to(
                self.inner.clone(),
                id,
                cancel,
                relay,
                recipient,
                payload,
                rec.name,
                rec.archive,
                rec.note,
                pause_flag,
            ));
        }
    }

    /// Re-register a download the user had paused, after a restart: it comes back as
    /// `Paused` under a fresh id (with its record + marker rewritten to match), and
    /// does **not** start fetching until [`resume`](Self::resume)d. Mirrors the
    /// paused-`send --to` restore in [`restore_sendto`](Self::restore_sendto).
    fn restore_paused_download(&self, rec: DownloadRecord) {
        let (id, _cancel) = self.register(Direction::Recv, None, rec.name.clone(), rec.size);
        // No running task to cancel for a paused download.
        self.inner.cancels.lock().unwrap().remove(&id);
        if let Some(dir) = &self.inner.state_dir {
            persist_download(
                dir,
                &DownloadRecord {
                    id,
                    ticket: rec.ticket,
                    out_path: rec.out_path,
                    name: rec.name.clone(),
                    size: rec.size,
                },
            );
            mark_paused(dir, id);
        }
        self.inner
            .set_status(id, TransferStatus::Paused("in pausa".to_string()));
        self.inner.emit(ManagerEvent::Paused {
            id,
            reason: "in pausa".to_string(),
        });
    }

    /// Serve an **anonymous** P2P ticket (no `--to`) in the background: returns the
    /// `arvc…` ticket and a transfer id, then keeps serving to whoever fetches it —
    /// the ticket is a reusable capability, so serving continues until
    /// [`cancel`](Self::cancel)led. Progress is tracked on the [`Transfer`] so a UI
    /// can show "downloading / % / delivered". Unlike [`send_to`](Self::send_to)
    /// there's no recipient, no offer, and no mailbox fallback.
    pub async fn serve_ticket(
        &self,
        payload: PathBuf,
        name: String,
        archive: bool,
        seed_relay: Option<String>,
    ) -> Result<(u64, String)> {
        let session = flow::prepare_send(
            &payload,
            &name,
            archive,
            None,
            seed_relay,
            RelayChoice::from_env(),
        )
        .await
        .context("prepare send")?;
        let ticket = session.ticket.clone();
        let total = session.total_size;
        let node_seed = session.node_seed();
        let (id, cancel) = self.register(Direction::Send, None, name, total);

        // Persist so the daemon can resume serving this ticket after a restart —
        // the same content key (carried in the ticket) + node seed reproduce the
        // same chunk hashes and node id, so the ticket already handed out stays
        // valid and receivers reconnect.
        if let Some(dir) = &self.inner.state_dir {
            persist_send(
                dir,
                &SendRecord {
                    id,
                    path: payload.to_string_lossy().into_owned(),
                    node_seed: node_seed.to_vec(),
                    ticket: ticket.clone(),
                    owned_stage: None,
                },
            );
        }

        tokio::spawn(serve_session(self.inner.clone(), session, id, cancel));
        Ok((id, ticket))
    }

    /// Serve an anonymous ticket **and** host a short pairing code for it in the
    /// background, so `arvolo code` can hand off to the daemon the way `arvolo
    /// ticket` already does. Returns the transfer id and the code to show.
    ///
    /// `max_sessions` is `Some(1)` for the default one-shot code and `None` for
    /// `--keep`. Needs a relay that speaks rendezvous v2: a v1 slot cannot outlive
    /// the process that claimed it, which is the whole point here.
    pub async fn serve_code(
        &self,
        payload: PathBuf,
        name: String,
        archive: bool,
        relay: String,
        embed: bool,
        max_sessions: Option<u32>,
    ) -> Result<(u64, String)> {
        anyhow::ensure!(
            code::relay_rz_version(&relay).await == code::RzVersion::V2,
            "relay {relay} is too old to host a background code (needs rendezvous v2) — \
             run `arvolo code --foreground`, which serves it in this terminal instead"
        );
        // Claim before serving: a code that can't be minted should leave no
        // half-started transfer behind.
        let (shown, host) = code::claim_code(&relay, embed)
            .await
            .context("claim a pairing code")?;

        let (id, ticket) = match self
            .serve_ticket(payload, name, archive, Some(relay.clone()))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let _ = host.close().await;
                return Err(e);
            }
        };

        let opts = code::HostOpts {
            max_sessions,
            ..code::HostOpts::default()
        };
        let rec = CodeRecord {
            id,
            slot: host.slot.clone(),
            secret: host.secret.clone(),
            relay: host.relay.clone(),
            owner_token: host.owner_token.to_vec(),
            payload: ticket.into_bytes(),
            shown: shown.clone(),
            max_sessions,
            max_failures: opts.max_failures,
            sessions_done: 0,
            failures: 0,
        };
        if let Some(dir) = &self.inner.state_dir {
            persist_code(dir, &rec);
        }
        // A child of the transfer's own token: `arvolo cancel <id>` retires the
        // code with the send, while the daemon merely exiting leaves the slot
        // alive on the relay for the next start to reattach to.
        let cancel = self
            .inner
            .cancels
            .lock()
            .unwrap()
            .get(&id)
            .map(|c| c.child_token())
            .unwrap_or_default();
        tokio::spawn(code_host_task(
            self.inner.clone(),
            rec,
            host,
            opts,
            code::HostState::default(),
            cancel,
        ));
        Ok((id, shown))
    }

    /// Bring a persisted code back after a restart, alongside the send it belongs
    /// to. The slot usually outlives the daemon (its lease is an hour, renewed on
    /// every listen), so the common case is simply reattaching; a slot that did
    /// lapse is re-claimed under the same nameplate so a code already written down
    /// keeps working.
    fn resume_code(&self, rec: CodeRecord, id: u64) {
        let Ok(owner_token) = <[u8; 32]>::try_from(rec.owner_token.as_slice()) else {
            tracing::warn!("dropping a code record with a malformed owner token");
            return;
        };
        let host = code::CodeHost {
            slot: rec.slot.clone(),
            secret: rec.secret.clone(),
            relay: rec.relay.clone(),
            owner_token,
        };
        let opts = code::HostOpts {
            max_sessions: rec.max_sessions,
            max_failures: rec.max_failures,
            ..code::HostOpts::default()
        };
        // Carried over verbatim — a restart that reset the failure count would
        // hand an attacker its guess budget back.
        let state = code::HostState {
            sessions_done: rec.sessions_done,
            failures: rec.failures,
        };
        let rec = CodeRecord { id, ..rec };
        let cancel = self
            .inner
            .cancels
            .lock()
            .unwrap()
            .get(&id)
            .map(|c| c.child_token())
            .unwrap_or_default();
        let inner = self.inner.clone();
        tokio::spawn(async move {
            match host.reattach().await {
                Ok(code::Reattach::Ok) => {}
                Ok(code::Reattach::Expired) => match host.reclaim().await {
                    Ok(code::Reattach::Ok) => {}
                    _ => {
                        tracing::warn!("pairing code {} could not be reclaimed", rec.shown);
                        if let Some(dir) = &inner.state_dir {
                            remove_code(dir, id);
                        }
                        inner.emit(ManagerEvent::CodeClosed {
                            id,
                            reason: "the rendezvous slot was taken over".to_string(),
                        });
                        return;
                    }
                },
                Ok(code::Reattach::Taken) | Err(_) => {
                    tracing::warn!("pairing code {} is no longer ours", rec.shown);
                    if let Some(dir) = &inner.state_dir {
                        remove_code(dir, id);
                    }
                    inner.emit(ManagerEvent::CodeClosed {
                        id,
                        reason: "the rendezvous slot was taken over".to_string(),
                    });
                    return;
                }
            }
            code_host_task(inner, rec, host, opts, state, cancel).await;
        });
    }

    /// Re-serve a persisted send after a daemon restart: rebind the same node id
    /// and content key (from the saved ticket) so the original ticket reconnects.
    /// Only anonymous (`Plain`-key) tickets are resumable here; `--to` sends aren't.
    /// Keep seeding a fully-downloaded file into the swarm. Modeled as a normal
    /// resumable send of the complete file with a fresh node identity, so it is
    /// persisted as a `SendRecord` and auto-resumes on daemon restart like any
    /// other sender. No-op for a sealed (`--to`) ticket.
    ///
    /// `owned` marks a file the daemon created purely to seed (a staged archive
    /// tar) — it is deleted when the session ends. For a single-file seed `path`
    /// is the user's own download, so `owned` is false and it's never deleted.
    fn seed_file(&self, path: PathBuf, ticket: String, owned: bool) {
        let path_str = path.to_string_lossy().into_owned();
        let rec = SendRecord {
            id: 0, // ignored: resume_serve registers a fresh id
            path: path_str.clone(),
            node_seed: crate::node::random_node_seed().to_vec(),
            ticket,
            owned_stage: owned.then_some(path_str),
        };
        if let Err(e) = self.resume_serve(rec) {
            tracing::warn!("seed-after-complete: not seeding {}: {e:#}", path.display());
        }
    }

    /// Returns the **new** transfer id: a resumed send is registered afresh, and
    /// anything keyed to the old id (a hosted pairing code) has to follow it.
    fn resume_serve(&self, rec: SendRecord) -> Result<u64> {
        let expected = crate::chunked::ChunkTicket::decode(&rec.ticket).context("decode ticket")?;
        let key: [u8; crate::crypto::CHUNK_KEY_LEN] = match &expected.key {
            crate::chunked::KeyDelivery::Plain(k) => {
                k.as_slice().try_into().context("bad content key length")?
            }
            _ => anyhow::bail!("cannot resume a sealed (--to) send"),
        };
        let node_seed: [u8; 32] = rec
            .node_seed
            .as_slice()
            .try_into()
            .context("bad node seed")?;
        let path = PathBuf::from(&rec.path);
        let (id, cancel) = self.register(
            Direction::Send,
            None,
            expected.name.clone(),
            expected.total_size,
        );
        if let Some(dir) = &self.inner.state_dir {
            persist_send(
                dir,
                &SendRecord {
                    id,
                    path: rec.path.clone(),
                    node_seed: rec.node_seed.clone(),
                    ticket: rec.ticket.clone(),
                    owned_stage: rec.owned_stage.clone(),
                },
            );
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            match flow::resume_send(
                &path,
                key,
                Some(node_seed),
                &expected,
                RelayChoice::from_env(),
            )
            .await
            {
                Ok(session) => serve_session(inner, session, id, cancel).await,
                Err(e) => finish(&inner, id, false, Err(e)),
            }
        });
        Ok(id)
    }

    // ---- receiving (offers) ----------------------------------------------

    /// Start a background task that long-polls this client's inbox and surfaces
    /// each incoming offer as a [`ManagerEvent::OfferReceived`]. Returns a token;
    /// cancel it to stop listening. Requires a relay.
    pub fn spawn_inbox(&self) -> Result<CancellationToken> {
        let sub = self.inner.inbox.clone().context(
            "a relay is required to receive offers (set ARVOLO_RELAY or config `relay`)",
        )?;
        let relay = self.inner.relay.clone().context("a relay is required")?;
        let cancel = CancellationToken::new();

        // Inbox long-poll: surface incoming offers.
        {
            let inner = self.inner.clone();
            let stop = cancel.clone();
            tokio::spawn(async move {
                sub.run(stop, move |offer| {
                    inner
                        .pending
                        .lock()
                        .unwrap()
                        .insert(offer.id.clone(), offer.clone());
                    inner.emit(ManagerEvent::OfferReceived {
                        id: offer.id.clone(),
                        from: offer.sender.clone(),
                        name: offer.offer.name.clone(),
                        size: offer.offer.size,
                        note: offer.offer.note.clone(),
                        sender_name: offer.offer.sender_name.clone(),
                    });
                })
                .await;
            });
        }

        // Presence beacon: a listening client is online, so refresh its beacon so
        // contacts can see it and push live instead of falling back to the mailbox.
        {
            let inner = self.inner.clone();
            let stop = cancel.clone();
            tokio::spawn(async move {
                let me = match inner.identity() {
                    Ok(m) => m,
                    Err(_) => return,
                };
                loop {
                    if stop.is_cancelled() {
                        return;
                    }
                    let _ = presence::publish_beacon(&inner.client, &relay, &me).await;
                    tokio::select! {
                        _ = stop.cancelled() => return,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(BEACON_REFRESH_SECS)) => {}
                    }
                }
            });
        }

        Ok(cancel)
    }

    /// Accept a pending offer by its handle: fetch it into `out` (or the default
    /// download dir, using the offer's name) transparently and ack it. Returns the
    /// receive transfer id.
    pub async fn accept_offer(&self, offer_id: &str, out: Option<PathBuf>) -> Result<u64> {
        let offer = self
            .inner
            .pending
            .lock()
            .unwrap()
            .remove(offer_id)
            .context("no such pending offer")?;

        // The offer name is sender-supplied and untrusted (even a "trusted"
        // contact is auto-downloaded via this path): reduce it to a single safe
        // path component so it can't traverse out of the download dir, and fall
        // back to a ticket-derived name if nothing usable remains. A `Some(out)`
        // that is a directory is resolved to a file *inside* it by `recv_chunked`
        // (the common chokepoint for both this daemon path and the CLI `recv`).
        let out_path = out.unwrap_or_else(|| {
            let base = crate::flow::safe_download_name(&offer.offer.name).unwrap_or_else(|| {
                crate::flow::default_out(&offer.offer.ticket)
                    .display()
                    .to_string()
            });
            self.inner.download_dir.join(base)
        });
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }

        // Ack the offer now that we've taken ownership of it (best-effort).
        if let Some(sub) = &self.inner.inbox {
            let _ = sub.ack(offer_id).await;
        }

        Ok(self.start_download(
            offer.offer.ticket.clone(),
            out_path,
            Some(offer.sender.clone()),
            offer.offer.name.clone(),
            offer.offer.size,
        ))
    }

    /// Start (or resume) a background download of `ticket` into `out_path`. A
    /// chunked (live/swarm) download is recorded to `state_dir` so a daemon restart
    /// can [`resume`](Self::resume_incomplete) it; the record is removed when the
    /// download finishes. `recv_chunked` itself resumes from the partial file on
    /// disk, so restarting the same (ticket, out_path) continues where it left off.
    pub fn start_download(
        &self,
        ticket: String,
        out_path: PathBuf,
        peer: Option<PublicId>,
        name: String,
        size: u64,
    ) -> u64 {
        let (id, cancel) = self.register(Direction::Recv, peer.clone(), name.clone(), size);

        // Persist a resume record for chunked downloads (offline one-shots aren't
        // worth resuming — they just re-fetch from the mailbox).
        if crate::chunked::ChunkTicket::looks_like(&ticket) {
            if let Some(dir) = &self.inner.state_dir {
                persist_download(
                    dir,
                    &DownloadRecord {
                        id,
                        ticket: ticket.clone(),
                        out_path: out_path.to_string_lossy().into_owned(),
                        name: name.clone(),
                        size,
                    },
                );
            }
        }

        self.spawn_receive(id, ticket, out_path, peer, cancel);
        id
    }

    /// Spawn the background fetch for receive `id`. Split out of [`start_download`] so
    /// [`resume`](Self::resume) can restart a **paused** download under its existing id
    /// with a fresh cancel token — without re-registering the row or rewriting its
    /// record. `recv_chunked` picks up from the partial file on disk, so a resumed
    /// fetch continues where the pause left off.
    fn spawn_receive(
        &self,
        id: u64,
        ticket: String,
        out_path: PathBuf,
        peer: Option<PublicId>,
        cancel: CancellationToken,
    ) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let me = match inner.identity() {
                Ok(id) => id,
                Err(e) => {
                    finish(&inner, id, false, Err(e));
                    return;
                }
            };
            let cancelled = cancel.clone();

            // An offline (`arvm`) offer is a one-shot mailbox fetch — no live
            // sender to stream from, so no per-chunk progress, and nothing to pause.
            if !crate::chunked::ChunkTicket::looks_like(&ticket) {
                if let Some(p) = &peer {
                    inner.set_peer(id, p.clone());
                }
                let result = flow::fetch_offline(&ticket, Some(out_path.clone()), &me, None).await;
                match result {
                    Ok((path, n)) => {
                        // No per-chunk progress for a one-shot mailbox fetch — record
                        // the fetched size so the history isn't "0 B".
                        inner.set_progress(id, n as u64);
                        finish(&inner, id, cancelled.is_cancelled(), Ok(Some(path)));
                    }
                    Err(e) => finish(&inner, id, false, Err(e)),
                }
                return;
            }

            let transferred = Arc::new(AtomicU64::new(0));
            let inner_cb = inner.clone();
            let t_cb = transferred.clone();
            let result = flow::recv_chunked(
                &ticket,
                Some(out_path.clone()),
                Some(&me),
                RelayChoice::from_env(),
                cancel,
                move |ev| match ev {
                    RecvEvent::Sender { id: Some(bytes) } => {
                        if let Ok(pk) = PublicId::from_bytes(&bytes) {
                            inner_cb.set_peer(id, pk);
                        }
                    }
                    RecvEvent::Sender { id: None } => {}
                    RecvEvent::Started {
                        resumed_bytes,
                        total_size,
                        ..
                    } => {
                        t_cb.store(resumed_bytes, Ordering::Relaxed);
                        inner_cb.set_progress(id, resumed_bytes);
                        inner_cb.emit(ManagerEvent::Progress {
                            id,
                            transferred: resumed_bytes,
                            total_size,
                        });
                    }
                    RecvEvent::Chunk { bytes, total, .. } => {
                        let done = t_cb.fetch_add(bytes, Ordering::Relaxed) + bytes;
                        inner_cb.set_progress(id, done);
                        // `total` is chunk count, not size — carry the record's size.
                        let total_size = inner_cb
                            .transfers
                            .lock()
                            .unwrap()
                            .get(&id)
                            .map(|t| t.total_size)
                            .unwrap_or(done);
                        let _ = total;
                        inner_cb.emit(ManagerEvent::Progress {
                            id,
                            transferred: done,
                            total_size,
                        });
                    }
                    RecvEvent::Swarm {
                        peers,
                        pieces_from_peers,
                    } => inner_cb.set_swarm(id, peers, pieces_from_peers),
                    RecvEvent::Saved { .. }
                    | RecvEvent::Control { .. }
                    | RecvEvent::Warning { .. }
                    | RecvEvent::Paused { .. } => {}
                },
            )
            .await;
            // Seed-after-complete: on a clean finish, keep serving the payload to
            // the swarm (opt-in via ARVOLO_SEED). Persisted, so it survives a
            // restart. For an archive `saved` is the unpacked directory, so we seed
            // the staged tar (a file we own and delete when the session ends); for a
            // single file we seed the download itself (owned by the user).
            if let Ok(outcome) = &result {
                // Only a *fully completed* download is seeded — a cancelled or
                // disk-full-paused one is partial (`is_complete()` excludes both).
                if outcome.is_complete() && flow::seeding_enabled() {
                    let mgr = TransferManager {
                        inner: inner.clone(),
                    };
                    match crate::chunked::ChunkTicket::decode(&ticket) {
                        Ok(t) if t.archive => {
                            let tar = flow::archive_stage_path(&t.chunks);
                            mgr.seed_file(tar, ticket.clone(), true);
                        }
                        _ => mgr.seed_file(outcome.path().to_path_buf(), ticket.clone(), false),
                    }
                }
            }
            // A pause is not a terminal state: the partial + record stay on disk so it
            // can resume, exactly like the disk-full case. Two ways in:
            //  - a **user pause** arrives as a Cancelled outcome (pause() cancelled the
            //    token) with the paused marker already set (pause() wrote it first);
            //  - a **disk-full stop** is a Paused outcome — we set the marker now so a
            //    restart keeps it paused rather than silently resuming into a full disk.
            // Everything else (done / real cancel / failure) is terminal and drops the
            // record via `finish`.
            let paused_marked = inner
                .state_dir
                .as_ref()
                .is_some_and(|dir| is_paused_marked(dir, id));
            let paused_reason =
                receive_pause_reason(&result, cancelled.is_cancelled(), paused_marked);
            if let Some(reason) = paused_reason {
                if let Some(dir) = &inner.state_dir {
                    mark_paused(dir, id);
                }
                inner.set_status(id, TransferStatus::Paused(reason.clone()));
                inner.emit(ManagerEvent::Paused { id, reason });
            } else {
                let stopped_incomplete = matches!(&result, Ok(o) if !o.is_complete());
                // A real cancel (user discarded it, no pause marker) is terminal:
                // `finish` drops the resume record, so the partial can never resume.
                // Delete the litter it would otherwise leave — the partial file, its
                // `.arvhave` sidecar, and the `.arvpart.N` chunk stages. Not on a
                // *failure* (Err): that partial can still resume if the user re-accepts.
                let discard = cancelled.is_cancelled() && stopped_incomplete;
                finish(
                    &inner,
                    id,
                    cancelled.is_cancelled() || stopped_incomplete,
                    result.map(|o| Some(o.into_path())),
                );
                if discard {
                    flow::discard_incomplete(&ticket, &out_path);
                }
            }
        });
    }

    /// Re-start every persisted, not-yet-finished chunked download (called once at
    /// daemon startup). Each resumes from its partial file on disk — no re-accept.
    /// Returns how many were resumed.
    pub fn resume_incomplete(&self) -> usize {
        let Some(dir) = self.inner.state_dir.clone() else {
            return 0;
        };
        let downloads = load_downloads(&dir);
        let sends = load_sends(&dir);
        let sendtos = load_sendtos(&dir);
        let depositeds = load_depositeds(&dir);
        // Indexed by the id they were written under; `resume_serve` allocates a
        // fresh one, so each is re-keyed as its send comes back.
        let mut codes: HashMap<u64, CodeRecord> = load_codes(&dir)
            .into_iter()
            .map(|rec| {
                remove_code(&dir, rec.id);
                (rec.id, rec)
            })
            .collect();
        let n = downloads.len() + sends.len() + sendtos.len() + depositeds.len();
        for rec in depositeds {
            // Re-registers under a fresh id; drop the stale record either way
            // (restore_deposited persists a fresh one when still in-window).
            remove_deposited(&dir, rec.id);
            self.restore_deposited(rec);
        }
        for rec in sendtos {
            // Re-registers under a fresh id; drop the stale record either way.
            remove_sendto(&dir, rec.id);
            self.restore_sendto(rec);
        }
        for rec in downloads {
            // A download the user paused comes back Paused (awaiting them); an active
            // one resumes fetching. Read the marker before dropping the stale record —
            // `remove_download` clears the marker too.
            let was_paused = is_paused_marked(&dir, rec.id);
            remove_download(&dir, rec.id);
            if was_paused {
                self.restore_paused_download(rec);
            } else {
                self.start_download(
                    rec.ticket,
                    PathBuf::from(rec.out_path),
                    None,
                    rec.name,
                    rec.size,
                );
            }
        }
        for rec in sends {
            let old_id = rec.id;
            remove_send(&dir, rec.id);
            // Re-serve the same ticket (same key + node seed). Best-effort: a send
            // whose file changed/vanished just isn't resumed.
            match self.resume_serve(rec) {
                // A send that hosted a pairing code brings it back too, re-keyed
                // onto the fresh id the resume registered.
                Ok(new_id) => {
                    if let Some(code) = codes.remove(&old_id) {
                        self.resume_code(code, new_id);
                    }
                }
                Err(e) => tracing::warn!("could not resume a send: {e:#}"),
            }
        }
        // Any code whose send didn't come back has nothing left to hand out.
        for rec in codes.into_values() {
            tracing::warn!(
                "dropping pairing code {}: its send did not resume",
                rec.shown
            );
        }
        n
    }

    /// Reject a pending offer: drop it and ack it so it stops coming back.
    pub async fn reject_offer(&self, offer_id: &str) {
        self.inner.pending.lock().unwrap().remove(offer_id);
        if let Some(sub) = &self.inner.inbox {
            let _ = sub.ack(offer_id).await;
        }
    }
}

/// Decide whether a finished receive should **park as Paused** (keeping its partial
/// output + resume record on disk) rather than reach a terminal state. Returns the
/// pause reason, or `None` for a terminal outcome (completed / real cancel / failure).
///
/// Pure so the branch can be pinned without a live transfer. Two ways to pause:
/// - a **disk-full** stop is a [`RecvOutcome::Paused`] — always resumable;
/// - a **user pause** arrives as [`RecvOutcome::Cancelled`] (pause cancels the fetch
///   token) *and* the paused marker is set (pause wrote it before cancelling). Without
///   the marker a Cancelled outcome is a real user cancel, which is terminal.
fn receive_pause_reason(
    result: &Result<flow::RecvOutcome>,
    cancel_fired: bool,
    paused_marked: bool,
) -> Option<String> {
    match result {
        Ok(flow::RecvOutcome::Paused { reason, .. }) => Some(reason.clone()),
        Ok(o) if !o.is_complete() && cancel_fired && paused_marked => Some("in pausa".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod clear_finished_tests {
    use super::*;

    /// The whole risk of a bulk clear is what it *keeps*. A Deposited send is the
    /// trap: it reads as finished — bytes all moved, nothing running — but its blob
    /// is on a relay awaiting pickup and the poll can still turn it Completed.
    /// Dropping it would lose the row *and* the pending delivery from the list.
    #[tokio::test]
    async fn clearing_keeps_everything_that_still_has_a_future() {
        let dir = tempfile::tempdir().unwrap();
        let m = TransferManager::with_state_dir(
            Identity::generate(),
            None,
            dir.path().to_path_buf(),
            Some(dir.path().to_path_buf()),
        );

        // One row per status, so the predicate is pinned from both sides.
        let mk = |status: TransferStatus| {
            let (id, _c) = m.register(Direction::Send, None, "f.bin".into(), 1);
            m.inner.cancels.lock().unwrap().remove(&id);
            m.inner.set_status(id, status);
            id
        };
        let completed = mk(TransferStatus::Completed);
        let cancelled = mk(TransferStatus::Cancelled);
        let failed = mk(TransferStatus::Failed("nope".into()));
        let active = mk(TransferStatus::Active);
        let deposited = mk(TransferStatus::Deposited);
        let waiting = mk(TransferStatus::Waiting("relay down".into()));
        let paused = mk(TransferStatus::Paused("by hand".into()));

        assert_eq!(m.clear_finished(), 3, "completed + cancelled + failed");

        for (id, what) in [
            (active, "active"),
            (deposited, "deposited — awaiting pickup, not done"),
            (waiting, "waiting"),
            (paused, "paused"),
        ] {
            assert!(m.get(id).is_some(), "{what} must survive a clear");
        }
        for (id, what) in [
            (completed, "completed"),
            (cancelled, "cancelled"),
            (failed, "failed"),
        ] {
            assert!(m.get(id).is_none(), "{what} must be cleared");
        }

        // Idempotent: nothing left to take.
        assert_eq!(m.clear_finished(), 0);
    }
}

#[cfg(test)]
mod pause_download_tests {
    use super::*;
    use crate::flow::RecvOutcome;
    use std::path::PathBuf;

    /// The heart of the fix: a stopped download is parked as Paused (kept + resumable)
    /// only when it should be — a disk-full stop always, a cancel only when the user
    /// pause marker is set. A real user cancel (no marker) and every terminal outcome
    /// stay terminal. Getting this wrong either loses a paused download or leaves a
    /// cancelled one un-droppable.
    #[test]
    fn a_download_parks_as_paused_exactly_when_it_should() {
        let p = PathBuf::from("/tmp/x");
        // disk-full: Paused outcome → always resumable, marker irrelevant.
        assert_eq!(
            receive_pause_reason(
                &Ok(RecvOutcome::Paused {
                    output: p.clone(),
                    reason: "disk full".into()
                }),
                false,
                false
            ),
            Some("disk full".to_string())
        );
        // user pause: cancel fired AND marker set.
        assert_eq!(
            receive_pause_reason(&Ok(RecvOutcome::Cancelled(p.clone())), true, true),
            Some("in pausa".to_string())
        );
        // real cancel: cancel fired, NO marker → terminal.
        assert_eq!(
            receive_pause_reason(&Ok(RecvOutcome::Cancelled(p.clone())), true, false),
            None
        );
        // completed → terminal, even if a marker somehow lingers.
        assert_eq!(
            receive_pause_reason(&Ok(RecvOutcome::Completed(p.clone())), false, true),
            None
        );
        // error → terminal.
        assert_eq!(
            receive_pause_reason(&Err(anyhow::anyhow!("boom")), true, true),
            None
        );
    }

    /// The paused marker and the resume record live and die together: `remove_download`
    /// (every terminal path) clears the marker too, so a finished download can never
    /// leave a stray "paused" flag a restart would honor.
    #[test]
    fn the_marker_never_outlives_its_record() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        persist_download(
            d,
            &DownloadRecord {
                id: 7,
                ticket: "arvc-x".into(),
                out_path: "/tmp/out".into(),
                name: "f".into(),
                size: 1,
            },
        );
        mark_paused(d, 7);
        assert!(is_paused_marked(d, 7));
        assert!(load_download(d, 7).is_some());

        remove_download(d, 7); // terminal
        assert!(!is_paused_marked(d, 7), "marker must go with the record");
        assert!(load_download(d, 7).is_none());
    }

    /// Persistence across a restart — the behavior the user asked for. A download
    /// paused before a restart comes back **Paused**, not silently re-downloading:
    /// `resume_incomplete` sees the marker and restores it without fetching.
    #[tokio::test]
    async fn a_paused_download_stays_paused_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let m = TransferManager::with_state_dir(
            Identity::generate(),
            None,
            dir.path().to_path_buf(),
            Some(dir.path().to_path_buf()),
        );
        // A paused download's on-disk state: a record + its marker.
        persist_download(
            dir.path(),
            &DownloadRecord {
                id: 3,
                ticket: "arvcSOMETHING".into(),
                out_path: dir.path().join("movie.mp4").to_string_lossy().into_owned(),
                name: "movie.mp4".into(),
                size: 1_000_000,
            },
        );
        mark_paused(dir.path(), 3);

        assert_eq!(m.resume_incomplete(), 1);

        let t = m
            .list()
            .into_iter()
            .find(|t| t.name == "movie.mp4")
            .expect("restored");
        assert!(
            matches!(t.status, TransferStatus::Paused(_)),
            "a paused download must come back Paused, not downloading — got {:?}",
            t.status
        );
        assert_eq!(t.direction, Direction::Recv);
        // Re-registered under a fresh id, with its record + marker rewritten to match,
        // so a later resume can find them.
        assert!(is_paused_marked(dir.path(), t.id));
        assert!(load_download(dir.path(), t.id).is_some());
    }

    /// An *active* (un-paused) download record still auto-resumes on restart — the
    /// pause path must not swallow the normal case.
    #[tokio::test]
    async fn an_unpaused_download_still_auto_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let m = TransferManager::with_state_dir(
            Identity::generate(),
            None,
            dir.path().to_path_buf(),
            Some(dir.path().to_path_buf()),
        );
        persist_download(
            dir.path(),
            &DownloadRecord {
                id: 4,
                // Not a real ticket: the fetch task will fail fast, but the row is
                // registered Active first, which is all this asserts.
                ticket: "arvcBOGUS".into(),
                out_path: dir.path().join("f.bin").to_string_lossy().into_owned(),
                name: "f.bin".into(),
                size: 10,
            },
        );
        // No marker → not paused.
        assert_eq!(m.resume_incomplete(), 1);
        let t = m
            .list()
            .into_iter()
            .find(|t| t.name == "f.bin")
            .expect("restored");
        assert!(
            !matches!(t.status, TransferStatus::Paused(_)),
            "an un-paused download must resume, not come back Paused"
        );
    }
}

#[cfg(test)]
mod deposit_event_tests {
    use super::*;

    /// The `Deposited` event must hand over the withdrawal capability, not just an
    /// id. A front-end files its own receipt from this — it is how a mailbox send
    /// stays listable and revocable — and it cannot read ours back: the write races
    /// the emit, and a manager without a `state_dir` never writes one at all.
    ///
    /// Driven through `resume_incomplete` because that is also the backfill path: a
    /// deposit made before any of this existed is re-announced on the next start,
    /// which is what spares us a migration.
    #[tokio::test]
    async fn restoring_a_deposit_re_announces_how_to_withdraw_it() {
        let dir = tempfile::tempdir().unwrap();
        let m = TransferManager::with_state_dir(
            Identity::generate(),
            None,
            dir.path().to_path_buf(),
            Some(dir.path().to_path_buf()),
        );
        let mut events = m.subscribe();

        let recipient = Identity::generate().public();
        let expires = unix_now() + 3600;
        persist_deposited(
            dir.path(),
            &DepositedRecord {
                id: 41,
                recipient: recipient.to_bytes().to_vec(),
                name: "budget.xlsx".into(),
                size: 4242,
                relay: "https://relay.example".into(),
                claim: "claim-abc".into(),
                expires,
                revoke_token: "revoke-me".into(),
                offer_id: "offer-1".into(),
                poster_token: "poster-1".into(),
            },
        );

        assert_eq!(m.resume_incomplete(), 1);

        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .expect("an event")
            .expect("not lagged");
        // `register` announces the row first; the deposit news follows.
        let ev = if matches!(ev, ManagerEvent::Started { .. }) {
            tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                .await
                .expect("an event")
                .expect("not lagged")
        } else {
            ev
        };

        let ManagerEvent::Deposited { id, info } = ev else {
            panic!("expected Deposited, got {ev:?}");
        };
        // A fresh id each restart — which is why the receipt is keyed on the claim,
        // not on this.
        assert!(m.get(id).is_some());
        assert_eq!(info.claim, "claim-abc");
        assert_eq!(info.revoke_token, "revoke-me");
        // The other half of a withdrawal: without these a front-end could only kill
        // the blob, leaving the recipient an arrival that can never be fetched.
        assert_eq!(info.offer_id, "offer-1");
        assert_eq!(info.poster_token, "poster-1");
        assert_eq!(info.relay, "https://relay.example");
        assert_eq!(info.name, "budget.xlsx");
        assert_eq!(info.size, 4242);
        assert_eq!(
            info.recipient.as_ref().map(|p| p.to_bytes().to_vec()),
            Some(recipient.to_bytes().to_vec())
        );
        // The relay's original deadline, not a fresh hour from now: a restart loop
        // must not keep a blob alive past the TTL its recipient was promised.
        assert_eq!(info.expires, expires);
    }
}
