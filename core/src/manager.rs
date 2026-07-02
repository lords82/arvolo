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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::crypto::{Identity, PublicId};
use crate::flow::{self, RecvEvent, SendEvent};
use crate::presence::{self, InboxSubscription, Offer, ReceivedOffer};
use crate::transfer::RelayChoice;

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
    /// Finished successfully.
    Completed,
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
    /// A transfer failed.
    Failed { id: u64, error: String },
    /// A transfer was cancelled.
    Cancelled { id: u64 },
}

struct Inner {
    next_id: AtomicU64,
    transfers: Mutex<HashMap<u64, Transfer>>,
    cancels: Mutex<HashMap<u64, CancellationToken>>,
    pending: Mutex<HashMap<String, ReceivedOffer>>,
    events: broadcast::Sender<ManagerEvent>,
    me: Identity,
    relay: Option<String>,
    client: reqwest::Client,
    download_dir: PathBuf,
    /// Shared inbox subscription (present iff a relay is configured). One instance
    /// so its proof-of-possession session token is reused across polls and acks.
    inbox: Option<Arc<InboxSubscription>>,
}

impl Inner {
    fn emit(&self, ev: ManagerEvent) {
        // No subscribers is fine — the state map is the source of truth.
        let _ = self.events.send(ev);
    }

    fn set_status(&self, id: u64, status: TransferStatus) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.status = status;
        }
        self.cancels.lock().unwrap().remove(&id);
    }

    fn set_progress(&self, id: u64, transferred: u64) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.transferred = transferred;
        }
    }

    fn set_peer(&self, id: u64, peer: PublicId) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.peer = Some(peer);
        }
    }

    /// Rebuild an owned identity for a spawned task (avoids borrowing `self.me`
    /// across the task's awaits). Cheap: it's a 32-byte key.
    fn identity(&self) -> Result<Identity> {
        Identity::from_secret_bytes(&self.me.secret_bytes())
    }
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
        let (events, _) = broadcast::channel(256);
        let inbox = relay
            .as_ref()
            .map(|r| Arc::new(InboxSubscription::new(r.clone(), &me)));
        Self {
            inner: Arc::new(Inner {
                next_id: AtomicU64::new(1),
                transfers: Mutex::new(HashMap::new()),
                cancels: Mutex::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                events,
                me,
                relay,
                client: reqwest::Client::new(),
                download_dir,
                inbox,
            }),
        }
    }

    /// Subscribe to the manager's event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<ManagerEvent> {
        self.inner.events.subscribe()
    }

    /// This client's public id.
    pub fn public_id(&self) -> PublicId {
        self.inner.me.public()
    }

    /// A snapshot of all known transfers (any status).
    pub fn list(&self) -> Vec<Transfer> {
        self.inner
            .transfers
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }

    /// A snapshot of one transfer by id, if still tracked.
    pub fn get(&self, id: u64) -> Option<Transfer> {
        self.inner.transfers.lock().unwrap().get(&id).cloned()
    }

    /// Drop all finished (completed/failed/cancelled) transfers from the list —
    /// e.g. when the user clears their history in the UI.
    pub fn clear_finished(&self) {
        self.inner
            .transfers
            .lock()
            .unwrap()
            .retain(|_, t| t.status == TransferStatus::Active);
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
                    direction,
                    peer,
                    name: name.clone(),
                    total_size,
                    transferred: 0,
                    status: TransferStatus::Active,
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

    /// Poll presence for up to `PRESENCE_GRACE_SECS`, returning as soon as the
    /// contact shows online (covers a client that's just starting up).
    async fn wait_online(&self, relay: &str, contact: &PublicId) -> bool {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(PRESENCE_GRACE_SECS);
        loop {
            if presence::check_online(&self.inner.client, relay, contact)
                .await
                .unwrap_or(false)
            {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_secs(PRESENCE_POLL_SECS)).await;
        }
    }

    // ---- sending ----------------------------------------------------------

    /// Send `payload` to `recipient`, choosing the path by presence: if they're
    /// online, serve it live P2P (`arvc`) and stop on delivery; if offline, deposit
    /// it on the relay mailbox (`arvm`, store-and-forward) so it lands when they
    /// return. Either way an [`Offer`] is placed in their inbox as the notification.
    /// Returns the transfer id.
    pub async fn send_to(
        &self,
        recipient: &PublicId,
        payload: PathBuf,
        name: String,
        archive: bool,
    ) -> Result<u64> {
        let relay = self.inner.relay.clone().context(
            "a relay is required to send to a contact (set ARVOLO_RELAY or config `relay`)",
        )?;

        // Offline path: deposit on the mailbox and post a long-lived `arvm` offer.
        if !self.wait_online(&relay, recipient).await {
            let size = deposit_offline_and_offer(&self.inner, &relay, recipient, &payload, &name)
                .await
                .context("deposit to mailbox")?;
            let (id, _cancel) = self.register(Direction::Send, Some(recipient.clone()), name, size);
            // The sender's work is done (blob is on the relay); knowing when the
            // recipient actually fetches it would need a delivery receipt (future).
            self.inner.set_progress(id, size);
            finish(&self.inner, id, false, Ok(None));
            return Ok(id);
        }

        // Online path: split + serve live, sealed to the recipient.
        let session = flow::prepare_send(
            &payload,
            &name,
            archive,
            Some((&self.inner.me, recipient)),
            Some(relay.clone()),
            RelayChoice::from_env(),
        )
        .await
        .context("prepare send")?;

        let offer = Offer {
            name: name.clone(),
            size: session.total_size,
            chunks: session.chunks as u64,
            ticket: session.ticket.clone(),
        };
        let posted = presence::post_offer(
            &self.inner.client,
            &relay,
            recipient,
            &self.inner.me,
            &offer,
            None,
        )
        .await
        .context("deliver offer")?;

        let (id, cancel) = self.register(
            Direction::Send,
            Some(recipient.clone()),
            name.clone(),
            session.total_size,
        );

        let inner = self.inner.clone();
        let recipient_owned = recipient.clone();
        tokio::spawn(async move {
            // A push targets one recipient: stop serving once they have the whole
            // file (`Delivered`). Presence is best-effort (a just-departed client
            // lingers "online" until its beacon lapses), so a watchdog falls back
            // to the mailbox if nobody actually starts pulling within the grace.
            let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let delivered = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let c = connected.clone();
            let d = delivered.clone();
            let stop = cancel.clone();
            let inner_cb = inner.clone();

            let wd_connected = connected.clone();
            let wd_cancel = cancel.clone();
            let watchdog = tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(LIVE_FALLBACK_SECS)).await;
                if !wd_connected.load(Ordering::Relaxed) {
                    wd_cancel.cancel();
                }
            });

            let result = session
                .serve(cancel, move |ev| match ev {
                    SendEvent::ReceiverConnected => c.store(true, Ordering::Relaxed),
                    SendEvent::Progress { transferred, total } => {
                        inner_cb.set_progress(id, transferred);
                        inner_cb.emit(ManagerEvent::Progress {
                            id,
                            transferred,
                            total_size: total,
                        });
                    }
                    SendEvent::Delivered => {
                        d.store(true, Ordering::Relaxed);
                        stop.cancel();
                    }
                    SendEvent::Ready { .. }
                    | SendEvent::ReceiverDropped { .. }
                    | SendEvent::Backfilled
                    | SendEvent::BackfillFailed { .. } => {}
                })
                .await;
            watchdog.abort();

            let was_delivered = delivered.load(Ordering::Relaxed);
            let was_connected = connected.load(Ordering::Relaxed);
            match result {
                Err(e) => finish(&inner, id, false, Err(e)),
                Ok(()) if was_delivered => {
                    // The chunk sender has no per-byte progress; on full delivery
                    // mark the whole size moved so the history isn't "0 B". Read the
                    // total in its own statement so the lock is released before
                    // `set_progress` re-locks.
                    let total = inner
                        .transfers
                        .lock()
                        .unwrap()
                        .get(&id)
                        .map(|t| t.total_size);
                    if let Some(total) = total {
                        inner.set_progress(id, total);
                    }
                    finish(&inner, id, false, Ok(None));
                }
                // Nobody ever connected (stale presence / recipient vanished) →
                // deposit to the mailbox so the file still lands later. First
                // retract the now-useless live offer so the recipient doesn't see a
                // dangling one whose accept would fail.
                Ok(()) if !was_connected => {
                    let _ = presence::retract_offer(
                        &inner.client,
                        &relay,
                        &recipient_owned,
                        &posted.id,
                        &posted.poster_token,
                    )
                    .await;
                    match deposit_offline_and_offer(
                        &inner,
                        &relay,
                        &recipient_owned,
                        &payload,
                        &name,
                    )
                    .await
                    {
                        Ok(sz) => {
                            inner.set_progress(id, sz);
                            finish(&inner, id, false, Ok(None));
                        }
                        Err(e) => finish(&inner, id, false, Err(e)),
                    }
                }
                // Connected but the loop ended without delivery = a user cancel.
                Ok(()) => finish(&inner, id, true, Ok(None)),
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

        let out_path = out.unwrap_or_else(|| self.inner.download_dir.join(&offer.offer.name));
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }

        let (id, cancel) = self.register(
            Direction::Recv,
            Some(offer.sender.clone()),
            offer.offer.name.clone(),
            offer.offer.size,
        );

        // Ack the offer now that we've taken ownership of it (best-effort).
        if let Some(sub) = &self.inner.inbox {
            let _ = sub.ack(offer_id).await;
        }

        let inner = self.inner.clone();
        let ticket = offer.offer.ticket.clone();
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
            // sender to stream from, so no per-chunk progress.
            if !crate::chunked::ChunkTicket::looks_like(&ticket) {
                inner.set_peer(id, offer.sender.clone());
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
                    RecvEvent::Saved { .. }
                    | RecvEvent::Control { .. }
                    | RecvEvent::Warning { .. } => {}
                },
            )
            .await;
            finish(&inner, id, cancelled.is_cancelled(), result.map(Some));
        });
        Ok(id)
    }

    /// Reject a pending offer: drop it and ack it so it stops coming back.
    pub async fn reject_offer(&self, offer_id: &str) {
        self.inner.pending.lock().unwrap().remove(offer_id);
        if let Some(sub) = &self.inner.inbox {
            let _ = sub.ack(offer_id).await;
        }
    }
}

/// How many finished transfers to keep in the list before pruning the oldest.
const MAX_FINISHED_RETAINED: usize = 512;
/// How often a listening client refreshes its presence beacon (< the relay's
/// `PRESENCE_TTL` of ~30s, so it never lapses while online).
const BEACON_REFRESH_SECS: u64 = 10;
/// How long `send_to` waits for the recipient to show online before falling back
/// to the offline mailbox, and how often it re-checks within that window.
const PRESENCE_GRACE_SECS: u64 = 10;
const PRESENCE_POLL_SECS: u64 = 2;
/// TTL of an offline (mailbox) deposit + its inbox offer: long enough for the
/// recipient to come back within a week.
const OFFLINE_TTL_SECS: u64 = 7 * 24 * 3600;
/// If a live send gets no receiver within this window, fall back to the mailbox.
/// Covers stale presence (a departed client whose beacon hasn't lapsed yet) and
/// an online recipient who ignores the offer. Generous enough for a real
/// cross-internet iroh connection to establish (relay handshake + hole-punch on a
/// cold endpoint can take well over 20s) before we give up on the direct path.
const LIVE_FALLBACK_SECS: u64 = 45;

/// Deposit `payload` to the relay mailbox (sealed to `recipient`) and post a
/// long-lived `arvm` offer pointing at it. Returns the payload size. Shared by the
/// up-front offline path and the live-send watchdog fallback.
async fn deposit_offline_and_offer(
    inner: &Inner,
    relay: &str,
    recipient: &PublicId,
    payload: &Path,
    name: &str,
) -> Result<u64> {
    let size = std::fs::metadata(payload).map(|m| m.len()).unwrap_or(0);
    let deposited = flow::deposit_offline(
        payload,
        recipient,
        &inner.me,
        relay,
        OFFLINE_TTL_SECS,
        1,
        None,
    )
    .await
    .context("deposit to mailbox")?;
    let offer = Offer {
        name: name.to_string(),
        size,
        chunks: 0,
        ticket: deposited.ticket.encode(),
    };
    presence::post_offer(
        &inner.client,
        relay,
        recipient,
        &inner.me,
        &offer,
        Some(OFFLINE_TTL_SECS),
    )
    .await
    .context("deliver offer")?;
    Ok(size)
}

/// Drop the oldest finished transfers (lowest ids) so at most `keep` remain.
/// Active transfers are never pruned.
fn prune_finished(transfers: &mut HashMap<u64, Transfer>, keep: usize) {
    let mut finished: Vec<u64> = transfers
        .iter()
        .filter(|(_, t)| t.status != TransferStatus::Active)
        .map(|(id, _)| *id)
        .collect();
    if finished.len() <= keep {
        return;
    }
    finished.sort_unstable();
    let drop_count = finished.len() - keep;
    for id in finished.into_iter().take(drop_count) {
        transfers.remove(&id);
    }
}

/// Finalize a transfer's state and emit the terminal event.
fn finish(inner: &Inner, id: u64, cancelled: bool, result: Result<Option<PathBuf>>) {
    match result {
        Ok(path) if cancelled => {
            inner.set_status(id, TransferStatus::Cancelled);
            inner.emit(ManagerEvent::Cancelled { id });
            let _ = path;
        }
        Ok(path) => {
            inner.set_status(id, TransferStatus::Completed);
            inner.emit(ManagerEvent::Completed { id, path });
        }
        Err(e) => {
            let msg = format!("{e:#}");
            inner.set_status(id, TransferStatus::Failed(msg.clone()));
            inner.emit(ManagerEvent::Failed { id, error: msg });
        }
    }
}
