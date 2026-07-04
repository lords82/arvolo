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
    /// Finished successfully (for a send: the recipient received it).
    Completed,
    /// An offline send: handed to the relay mailbox, delivery not yet confirmed.
    Deposited,
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
    /// An offline send was deposited to the mailbox (awaiting the recipient). It
    /// may later transition to [`Completed`](ManagerEvent::Completed) once the
    /// recipient fetches it (within the confirmation window).
    Deposited { id: u64 },
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
    /// Where to persist resumable-download records (present in the daemon). When
    /// set, an accepted chunked download writes a record here on start and removes
    /// it on finish, so [`resume_incomplete`](TransferManager::resume_incomplete)
    /// can restart it after a daemon/machine restart. `None` = no persistence
    /// (ephemeral one-shot clients).
    state_dir: Option<PathBuf>,
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

    fn set_swarm(&self, id: u64, peers: usize, from_peers: u64) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.swarm_peers = peers;
            t.pieces_from_peers = from_peers;
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
                pending: Mutex::new(HashMap::new()),
                events,
                me,
                relay,
                client: reqwest::Client::new(),
                download_dir,
                state_dir,
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
    /// e.g. when the user clears their history in the UI. Keeps still-in-flight
    /// transfers (Active, and Deposited ones still awaiting a pickup confirmation).
    pub fn clear_finished(&self) {
        self.inner
            .transfers
            .lock()
            .unwrap()
            .retain(|_, t| matches!(t.status, TransferStatus::Active | TransferStatus::Deposited));
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
                    swarm_peers: 0,
                    pieces_from_peers: 0,
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

        // Offline path: deposit on the mailbox and post a long-lived `arvm` offer,
        // then confirm delivery by watching the blob get fetched.
        if !self.wait_online(&relay, recipient).await {
            let out = deposit_offline_and_offer(&self.inner, &relay, recipient, &payload, &name)
                .await
                .context("deposit to mailbox")?;
            let (id, _cancel) =
                self.register(Direction::Send, Some(recipient.clone()), name, out.size);
            spawn_offline_confirm(&self.inner, id, relay.clone(), out.size, out.claim);
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

            // Two-phase watchdog. Phase 1: wait for the offer to be *seen* by a
            // live recipient poll (a real "they're online right now" signal). If
            // it's never seen, presence was stale -> fall back fast. Phase 2: once
            // seen, give the (highly variable) cross-internet P2P connection
            // generous time before giving up.
            let wd_connected = connected.clone();
            let wd_cancel = cancel.clone();
            let wd_inner = inner.clone();
            let wd_relay = relay.clone();
            let wd_recipient = recipient_owned.clone();
            let wd_offer = posted.id.clone();
            let wd_token = posted.poster_token.clone();
            let watchdog = tokio::spawn(async move {
                use std::time::{Duration, Instant};
                let phase1 = Instant::now() + Duration::from_secs(LIVE_CONFIRM_SECS);
                let mut seen = false;
                while Instant::now() < phase1 {
                    if wd_connected.load(Ordering::Relaxed) {
                        return; // already connecting
                    }
                    if matches!(
                        presence::offer_status(
                            &wd_inner.client,
                            &wd_relay,
                            &wd_recipient,
                            &wd_offer,
                            &wd_token,
                        )
                        .await,
                        Ok(presence::OfferStatus::Fetched) | Ok(presence::OfferStatus::Gone)
                    ) {
                        seen = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(OFFER_STATUS_POLL_SECS)).await;
                }
                if !seen {
                    wd_cancel.cancel(); // stale presence -> fall back now
                    return;
                }
                let phase2 = Instant::now() + Duration::from_secs(LIVE_CONNECT_SECS);
                while Instant::now() < phase2 {
                    if wd_connected.load(Ordering::Relaxed) {
                        return; // connected -> keep serving
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                wd_cancel.cancel(); // seen but never connected -> fall back
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
                        Ok(out) => {
                            spawn_offline_confirm(&inner, id, relay.clone(), out.size, out.claim)
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
                },
            );
        }

        tokio::spawn(serve_session(self.inner.clone(), session, id, cancel));
        Ok((id, ticket))
    }

    /// Re-serve a persisted send after a daemon restart: rebind the same node id
    /// and content key (from the saved ticket) so the original ticket reconnects.
    /// Only anonymous (`Plain`-key) tickets are resumable here; `--to` sends aren't.
    fn resume_serve(&self, rec: SendRecord) -> Result<()> {
        let expected = crate::chunked::ChunkTicket::decode(&rec.ticket).context("decode ticket")?;
        let key: [u8; crate::crypto::CHUNK_KEY_LEN] = match &expected.key {
            crate::chunked::KeyDelivery::Plain(k) => {
                k.as_slice().try_into().context("bad content key length")?
            }
            _ => anyhow::bail!("cannot resume a sealed (--to) send"),
        };
        let node_seed: [u8; 32] = rec.node_seed.as_slice().try_into().context("bad node seed")?;
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
                },
            );
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            match flow::resume_send(&path, key, Some(node_seed), &expected, RelayChoice::from_env())
                .await
            {
                Ok(session) => serve_session(inner, session, id, cancel).await,
                Err(e) => finish(&inner, id, false, Err(e)),
            }
        });
        Ok(())
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
            // sender to stream from, so no per-chunk progress.
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
                    | RecvEvent::Warning { .. } => {}
                },
            )
            .await;
            finish(&inner, id, cancelled.is_cancelled(), result.map(Some));
        });
        id
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
        let n = downloads.len() + sends.len();
        for rec in downloads {
            // Drop the stale record; start_download writes a fresh one (new id).
            remove_download(&dir, rec.id);
            self.start_download(rec.ticket, PathBuf::from(rec.out_path), None, rec.name, rec.size);
        }
        for rec in sends {
            remove_send(&dir, rec.id);
            // Re-serve the same ticket (same key + node seed). Best-effort: a send
            // whose file changed/vanished just isn't resumed.
            if let Err(e) = self.resume_serve(rec) {
                tracing::warn!("could not resume a send: {e:#}");
            }
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
/// Phase 1 of the live-send watchdog: how long to wait for the offer to be *seen*
/// by a live recipient poll before concluding presence was stale and falling back.
const LIVE_CONFIRM_SECS: u64 = 12;
/// Phase 2: once the offer is seen, how long to let the P2P connection establish
/// (cross-internet iroh cold-start + hole-punch is highly variable) before giving
/// up and falling back to the mailbox.
const LIVE_CONNECT_SECS: u64 = 90;
/// How often the watchdog polls the offer's seen-status during phase 1.
const OFFER_STATUS_POLL_SECS: u64 = 2;
/// How long (and how often) a stay-open sender polls to confirm an offline blob
/// was fetched before leaving the transfer as merely "deposited".
const OFFLINE_CONFIRM_SECS: u64 = 90;
const OFFLINE_CONFIRM_POLL_SECS: u64 = 3;

/// What a mailbox deposit yields: the payload size + the claim to poll for delivery.
struct DepositOutcome {
    size: u64,
    claim: String,
}

/// Deposit `payload` to the relay mailbox (sealed to `recipient`) and post a
/// long-lived `arvm` offer pointing at it. Shared by the up-front offline path and
/// the live-send watchdog fallback.
async fn deposit_offline_and_offer(
    inner: &Inner,
    relay: &str,
    recipient: &PublicId,
    payload: &Path,
    name: &str,
) -> Result<DepositOutcome> {
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
    let claim = deposited.ticket.claim.clone();
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
    Ok(DepositOutcome { size, claim })
}

/// Poll the relay until the deposited blob `claim` is fetched (delivered) or the
/// confirmation window elapses. On delivery, flip the transfer to Completed and
/// emit it; otherwise leave it as Deposited. Runs as a detached task.
async fn confirm_offline_delivery(inner: Arc<Inner>, id: u64, relay: String, claim: String) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(OFFLINE_CONFIRM_SECS);
    loop {
        if matches!(
            flow::claim_status(&relay, &claim).await,
            Ok(flow::ClaimStatus::Gone)
        ) {
            inner.set_status(id, TransferStatus::Completed);
            inner.emit(ManagerEvent::Completed { id, path: None });
            return;
        }
        if std::time::Instant::now() >= deadline {
            return; // stays Deposited; the blob still lives on the relay
        }
        tokio::time::sleep(std::time::Duration::from_secs(OFFLINE_CONFIRM_POLL_SECS)).await;
    }
}

/// Mark a transfer as deposited-to-mailbox and start confirming its delivery.
fn spawn_offline_confirm(inner: &Arc<Inner>, id: u64, relay: String, size: u64, claim: String) {
    inner.set_progress(id, size);
    inner.set_status(id, TransferStatus::Deposited);
    inner.emit(ManagerEvent::Deposited { id });
    tokio::spawn(confirm_offline_delivery(inner.clone(), id, relay, claim));
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

/// Run a send session to completion: report progress, keep serving (a ticket may
/// feed several receivers), and finish on cancel or error — dropping the resume
/// record. Shared by [`TransferManager::serve_ticket`] and `resume_serve`.
async fn serve_session(
    inner: Arc<Inner>,
    session: flow::SendSession,
    id: u64,
    cancel: CancellationToken,
) {
    let total = session.total_size;
    let delivered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let d = delivered.clone();
    let inner_cb = inner.clone();
    let result = session
        .serve(cancel, move |ev| match ev {
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
                inner_cb.set_progress(id, total);
                inner_cb.emit(ManagerEvent::Progress {
                    id,
                    transferred: total,
                    total_size: total,
                });
            }
            SendEvent::Ready { .. }
            | SendEvent::ReceiverConnected
            | SendEvent::ReceiverDropped { .. }
            | SendEvent::Backfilled
            | SendEvent::BackfillFailed { .. } => {}
        })
        .await;
    match result {
        Err(e) => finish(&inner, id, false, Err(e)),
        Ok(()) => finish(&inner, id, !delivered.load(Ordering::Relaxed), Ok(None)),
    }
}

/// Finalize a transfer's state and emit the terminal event.
fn finish(inner: &Inner, id: u64, cancelled: bool, result: Result<Option<PathBuf>>) {
    // Terminal state: drop any resume record so a restart doesn't re-start it.
    if let Some(dir) = &inner.state_dir {
        remove_download(dir, id);
        remove_send(dir, id);
    }
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

// ---- resumable-download persistence ---------------------------------------

/// On-disk record of an accepted, not-yet-finished chunked download, so the
/// daemon can resume it after a restart. Small (a ticket + a path) — one postcard
/// file per download under the manager's `state_dir`.
#[derive(serde::Serialize, serde::Deserialize)]
struct DownloadRecord {
    id: u64,
    ticket: String,
    out_path: String,
    name: String,
    size: u64,
}

fn download_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("dl-{id}.pc"))
}

fn persist_download(dir: &Path, rec: &DownloadRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(download_record_path(dir, rec.id), bytes);
    }
}

fn remove_download(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(download_record_path(dir, id));
}

fn load_downloads(dir: &Path) -> Vec<DownloadRecord> {
    load_records(dir, "dl-")
}

/// On-disk record of an active send (serving an anonymous `arvc…` ticket), so the
/// daemon can resume serving after a restart. Stores the file path, the ticket
/// (which carries the content key + chunk hashes + name), and the node seed — so
/// the same node id and hashes are reproduced and the ticket already handed out
/// keeps working.
#[derive(serde::Serialize, serde::Deserialize)]
struct SendRecord {
    id: u64,
    path: String,
    node_seed: Vec<u8>,
    ticket: String,
}

fn send_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("send-{id}.pc"))
}

fn persist_send(dir: &Path, rec: &SendRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(send_record_path(dir, rec.id), bytes);
    }
}

fn remove_send(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(send_record_path(dir, id));
}

fn load_sends(dir: &Path) -> Vec<SendRecord> {
    load_records(dir, "send-")
}

/// Read every postcard record whose filename starts with `prefix` from `dir`.
fn load_records<T: serde::de::DeserializeOwned>(dir: &Path, prefix: &str) -> Vec<T> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_match = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with(prefix) && n.ends_with(".pc"))
            .unwrap_or(false);
        if !is_match {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(rec) = postcard::from_bytes::<T>(&bytes) {
                out.push(rec);
            }
        }
    }
    out
}
