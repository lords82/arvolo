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
}

/// Live state for an in-progress `send --to` delivery, so it can be paused,
/// resumed, or re-driven. Mirrored to disk (a `SendToRecord`) for durability.
#[derive(Clone)]
struct Held {
    recipient: PublicId,
    payload: PathBuf,
    name: String,
    archive: bool,
    /// Flipped by [`TransferManager::pause`] so the running loop, once its token is
    /// cancelled, knows to pause (keep the transfer) rather than cancel (drop it).
    pause_flag: Arc<std::sync::atomic::AtomicBool>,
}

struct Inner {
    next_id: AtomicU64,
    transfers: Mutex<HashMap<u64, Transfer>>,
    cancels: Mutex<HashMap<u64, CancellationToken>>,
    /// Delivery state for active/paused `send --to` transfers (for pause/resume).
    held: Mutex<HashMap<u64, Held>>,
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

    /// Set a **terminal-or-paused** status: also drops the cancel token, since the
    /// running task is ending (finished, deposited, or paused — a resume installs a
    /// fresh token).
    fn set_status(&self, id: u64, status: TransferStatus) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.status = status;
        }
        self.cancels.lock().unwrap().remove(&id);
    }

    /// Set a **live** status (Active / Waiting) *without* touching the cancel token,
    /// so a still-running transfer stays cancellable across status changes.
    fn set_status_live(&self, id: u64, status: TransferStatus) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.status = status;
        }
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

    fn set_download_peers(&self, id: u64, count: usize) {
        if let Some(t) = self.transfers.lock().unwrap().get_mut(&id) {
            t.download_peers = count;
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
                held: Mutex::new(HashMap::new()),
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

    /// Drop all finished (completed/failed/cancelled) transfers from the list —
    /// e.g. when the user clears their history in the UI. Keeps still-in-flight
    /// transfers (Active, and Deposited ones still awaiting a pickup confirmation).
    pub fn clear_finished(&self) {
        self.inner.transfers.lock().unwrap().retain(|_, t| {
            matches!(
                t.status,
                TransferStatus::Active
                    | TransferStatus::Deposited
                    | TransferStatus::Waiting(_)
                    | TransferStatus::Paused(_)
            )
        });
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
                    download_peers: 0,
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
        // A paused send has no running loop to cancel — end it directly.
        let paused = self
            .inner
            .transfers
            .lock()
            .unwrap()
            .get(&id)
            .map(|t| matches!(t.status, TransferStatus::Paused(_)))
            .unwrap_or(false);
        if paused {
            finish(&self.inner, id, true, Ok(None));
            return;
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
    /// its id immediately; delivery runs in the background (see [`deliver_to`]):
    /// live P2P whenever the recipient is online, else the relay mailbox. If the
    /// relay can't take it (too large / unreachable / error) the send is *held*
    /// (`Waiting`) and keeps retrying rather than failing.
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
            pause_flag,
        ));
        Ok(id)
    }

    /// Pause an in-progress `send --to` (Active or Waiting): stop actively trying
    /// and hold it as `Paused` until `resume`d or `cancel`led. No-op for anything
    /// that isn't a live send. Returns whether it was paused.
    pub fn pause(&self, id: u64) -> bool {
        let can = self
            .inner
            .transfers
            .lock()
            .unwrap()
            .get(&id)
            .map(|t| {
                matches!(
                    t.status,
                    TransferStatus::Active | TransferStatus::Waiting(_)
                )
            })
            .unwrap_or(false);
        let held = self.inner.held.lock().unwrap().get(&id).cloned();
        let (true, Some(h)) = (can, held) else {
            return false;
        };
        h.pause_flag.store(true, Ordering::Relaxed);
        if let Some(c) = self.inner.cancels.lock().unwrap().get(&id) {
            c.cancel(); // wakes the loop, which sees the flag and pauses
        }
        true
    }

    /// Resume a `Paused` send: start delivering again. Returns whether it resumed.
    pub fn resume(&self, id: u64) -> bool {
        let paused = self
            .inner
            .transfers
            .lock()
            .unwrap()
            .get(&id)
            .map(|t| matches!(t.status, TransferStatus::Paused(_)))
            .unwrap_or(false);
        let held = self.inner.held.lock().unwrap().get(&id).cloned();
        let relay = self.inner.relay.clone();
        let (true, Some(h), Some(relay)) = (paused, held, relay) else {
            return false;
        };
        h.pause_flag.store(false, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        self.inner
            .cancels
            .lock()
            .unwrap()
            .insert(id, cancel.clone());
        self.inner.set_status_live(id, TransferStatus::Active);
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
            h.pause_flag,
        ));
        true
    }

    /// Re-register a `send --to` restored from disk after a daemon restart: paused
    /// ones come back `Paused` (awaiting the user); active ones resume delivering.
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
                pause_flag,
            ));
        }
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

    fn resume_serve(&self, rec: SendRecord) -> Result<()> {
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
            // Seed-after-complete: on a clean finish, keep serving the payload to
            // the swarm (opt-in via ARVOLO_SEED). Persisted, so it survives a
            // restart. For an archive `saved` is the unpacked directory, so we seed
            // the staged tar (a file we own and delete when the session ends); for a
            // single file we seed the download itself (owned by the user).
            if let (Ok(saved), false) = (&result, cancelled.is_cancelled()) {
                if flow::seeding_enabled() {
                    let mgr = TransferManager {
                        inner: inner.clone(),
                    };
                    match crate::chunked::ChunkTicket::decode(&ticket) {
                        Ok(t) if t.archive => {
                            let tar = flow::archive_stage_path(&t.chunks);
                            mgr.seed_file(tar, ticket.clone(), true);
                        }
                        _ => mgr.seed_file(saved.clone(), ticket.clone(), false),
                    }
                }
            }
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
        let sendtos = load_sendtos(&dir);
        let n = downloads.len() + sends.len() + sendtos.len();
        for rec in sendtos {
            // Re-registers under a fresh id; drop the stale record either way.
            remove_sendto(&dir, rec.id);
            self.restore_sendto(rec);
        }
        for rec in downloads {
            // Drop the stale record; start_download writes a fresh one (new id).
            remove_download(&dir, rec.id);
            self.start_download(
                rec.ticket,
                PathBuf::from(rec.out_path),
                None,
                rec.name,
                rec.size,
            );
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

/// How long to hold before re-trying the relay after it was unavailable/errored.
const RELAY_RETRY_SECS: u64 = 15 * 60;
/// How often to re-check the recipient's presence while a send is held.
const WAITING_POLL_SECS: u64 = 15;
/// Upper bound on how long a held (`Waiting`) send keeps trying before it gives up
/// and fails — so a too-large file to a recipient who never returns doesn't pin a
/// payload and a task forever. Mirrors the offline mailbox/offer lifetime.
const MAX_WAITING_SECS: u64 = OFFLINE_TTL_SECS;

/// Outcome of one live-P2P delivery attempt to a recipient believed online.
enum LiveOutcome {
    /// The recipient received the whole file.
    Delivered,
    /// Nobody actually connected (stale presence / they vanished) — fall back to
    /// the relay, or keep waiting for them to reappear.
    NotConnected,
    /// The user cancelled the send.
    Cancelled,
    /// An unrecoverable error preparing or serving.
    Fatal(anyhow::Error),
}

fn set_waiting(inner: &Arc<Inner>, id: u64, reason: &str) {
    // Live status: keep the cancel token so pause/cancel still reach the loop.
    inner.set_status_live(id, TransferStatus::Waiting(reason.to_string()));
    inner.emit(ManagerEvent::Waiting {
        id,
        reason: reason.to_string(),
    });
}

/// Write/refresh the on-disk record for a held `send --to` (for durable restore).
fn persist_held_record(inner: &Inner, id: u64, paused: bool, reason: &str) {
    let Some(dir) = &inner.state_dir else { return };
    if let Some(h) = inner.held.lock().unwrap().get(&id) {
        persist_sendto(
            dir,
            &SendToRecord {
                id,
                recipient: h.recipient.to_bytes(),
                payload: h.payload.to_string_lossy().into_owned(),
                name: h.name.clone(),
                archive: h.archive,
                paused,
                reason: reason.to_string(),
            },
        );
    }
}

/// Forget a `send --to`'s delivery state (memory + disk) — called on any terminal
/// end (delivered, deposited, cancelled, failed).
fn drop_held(inner: &Inner, id: u64) {
    inner.held.lock().unwrap().remove(&id);
    if let Some(dir) = &inner.state_dir {
        remove_sendto(dir, id);
    }
}

/// Move a held send to the `Paused` state (keeps its delivery state so it can be
/// resumed) and persist that so a restart restores it paused.
fn set_paused(inner: &Arc<Inner>, id: u64, reason: &str) {
    // The running loop is ending; drop its (now spent) token — resume installs a
    // fresh one. Persist *before* clearing so the record reflects the pause.
    persist_held_record(inner, id, true, reason);
    inner.set_status(id, TransferStatus::Paused(reason.to_string()));
    inner.emit(ManagerEvent::Paused {
        id,
        reason: reason.to_string(),
    });
}

/// The loop's token was cancelled: pause (keep the send) if a pause was requested,
/// else cancel it (drop everything).
fn handle_stop(inner: &Arc<Inner>, id: u64, pause_flag: &std::sync::atomic::AtomicBool) {
    if pause_flag.load(Ordering::Relaxed) {
        set_paused(inner, id, "paused");
    } else {
        finish(inner, id, true, Ok(None));
    }
}

/// Background delivery loop for a `send --to`. Prefers live P2P whenever the
/// recipient is online; otherwise hands the file to the relay mailbox. If the
/// relay can't take it, the send is *held* (`Waiting`) instead of failed:
///
/// * **too large** — never retried on the relay (it can only go P2P); the loop
///   just keeps watching presence to deliver live.
/// * **unreachable / error** — retried on the relay on a slow [`RELAY_RETRY_SECS`]
///   interval, while presence is re-checked fast so a P2P window is never missed.
///
/// Ends (terminal state) only on delivery, a fatal error, a successful mailbox
/// deposit, or user cancel.
#[allow(clippy::too_many_arguments)]
async fn deliver_to(
    inner: Arc<Inner>,
    id: u64,
    cancel: CancellationToken,
    relay: String,
    recipient: PublicId,
    payload: PathBuf,
    name: String,
    archive: bool,
    pause_flag: Arc<std::sync::atomic::AtomicBool>,
) {
    use std::time::{Duration, Instant};
    let mut too_big = false;
    let mut next_relay_try = Instant::now();
    let give_up_at = Instant::now() + Duration::from_secs(MAX_WAITING_SECS);
    loop {
        if cancel.is_cancelled() {
            handle_stop(&inner, id, &pause_flag);
            return;
        }
        // Held too long with no delivery → auto-pause (keep it, tell the user) so a
        // too-large file to a recipient who never returns doesn't retry forever.
        if Instant::now() >= give_up_at {
            set_paused(
                &inner,
                id,
                &format!(
                    "still undelivered after {} days — paused; resume to keep trying, or cancel",
                    MAX_WAITING_SECS / 86_400
                ),
            );
            return;
        }

        // 1) Recipient online → deliver live P2P.
        let online = presence::check_online(&inner.client, &relay, &recipient)
            .await
            .unwrap_or(false);
        if online {
            match serve_live_once(
                &inner, id, &cancel, &relay, &recipient, &payload, &name, archive,
            )
            .await
            {
                LiveOutcome::Delivered => {
                    // The chunk sender has no per-byte progress; on full delivery
                    // mark the whole size moved so history isn't "0 B".
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
                    return;
                }
                LiveOutcome::Cancelled => {
                    handle_stop(&inner, id, &pause_flag);
                    return;
                }
                LiveOutcome::Fatal(e) => {
                    finish(&inner, id, false, Err(e));
                    return;
                }
                LiveOutcome::NotConnected => {}
            }
        }

        // 2) Try the relay mailbox — unless it already refused as too large, or
        //    we're backing off after it was unavailable.
        if !too_big && Instant::now() >= next_relay_try {
            match deposit_offline_and_offer(&inner, &relay, &recipient, &payload, &name).await {
                Ok(out) => {
                    drop_held(&inner, id);
                    spawn_offline_confirm(&inner, id, relay.clone(), out.size, out.claim);
                    return;
                }
                Err(flow::DepositError::TooLarge) => {
                    too_big = true;
                    set_waiting(
                        &inner,
                        id,
                        "file too large for this relay — waiting for the recipient to come online for a direct transfer",
                    );
                }
                Err(flow::DepositError::Fatal(e)) => {
                    finish(&inner, id, false, Err(e));
                    return;
                }
                Err(e @ flow::DepositError::Unavailable(_)) => {
                    set_waiting(
                        &inner,
                        id,
                        &format!(
                            "{e} — retrying later, and delivering P2P as soon as the recipient is online"
                        ),
                    );
                    next_relay_try = Instant::now() + Duration::from_secs(RELAY_RETRY_SECS);
                }
            }
        }

        // 3) Idle briefly, then re-check presence (react immediately to pause/cancel).
        tokio::select! {
            _ = cancel.cancelled() => {
                handle_stop(&inner, id, &pause_flag);
                return;
            }
            _ = tokio::time::sleep(Duration::from_secs(WAITING_POLL_SECS)) => {}
        }
    }
}

/// One live-P2P delivery attempt to a recipient believed online: prepare the
/// sealed send, post the offer, and serve it under a two-phase watchdog. Uses a
/// child cancellation token so the watchdog can abandon *this attempt* (fall back)
/// without cancelling the caller's user-cancel token.
#[allow(clippy::too_many_arguments)]
async fn serve_live_once(
    inner: &Arc<Inner>,
    id: u64,
    cancel: &CancellationToken,
    relay: &str,
    recipient: &PublicId,
    payload: &Path,
    name: &str,
    archive: bool,
) -> LiveOutcome {
    let session = match flow::prepare_send(
        payload,
        name,
        archive,
        Some((&inner.me, recipient)),
        Some(relay.to_string()),
        RelayChoice::from_env(),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return LiveOutcome::Fatal(e.context("prepare send")),
    };

    let offer = Offer {
        name: name.to_string(),
        size: session.total_size,
        chunks: session.chunks as u64,
        ticket: session.ticket.clone(),
    };
    let posted = match presence::post_offer(
        &inner.client,
        relay,
        recipient,
        &inner.me,
        &offer,
        None,
    )
    .await
    {
        Ok(p) => p,
        // Can't even notify them → treat as not-connected; the caller retries.
        Err(_) => return LiveOutcome::NotConnected,
    };
    inner.set_status(id, TransferStatus::Active);

    // Per-attempt child token: the watchdog cancels *this* to stop serving on
    // fallback, leaving the caller's token (user cancel) untouched.
    let attempt = cancel.child_token();
    let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let delivered = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let wd_attempt = attempt.clone();
    let wd_connected = connected.clone();
    let wd_inner = inner.clone();
    let wd_relay = relay.to_string();
    let wd_recipient = recipient.clone();
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
            wd_attempt.cancel(); // stale presence -> abandon this attempt
            return;
        }
        let phase2 = Instant::now() + Duration::from_secs(LIVE_CONNECT_SECS);
        while Instant::now() < phase2 {
            if wd_connected.load(Ordering::Relaxed) {
                return; // connected -> keep serving
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        wd_attempt.cancel(); // seen but never connected -> abandon this attempt
    });

    let c = connected.clone();
    let d = delivered.clone();
    let stop = attempt.clone();
    let inner_cb = inner.clone();
    let result = session
        .serve(attempt, move |ev| match ev {
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
            SendEvent::Peers { count } => inner_cb.set_download_peers(id, count),
            SendEvent::Ready { .. }
            | SendEvent::ReceiverDropped { .. }
            | SendEvent::Backfilled
            | SendEvent::BackfillFailed { .. }
            | SendEvent::RelayCapped { .. } => {}
        })
        .await;
    watchdog.abort();

    if let Err(e) = result {
        return LiveOutcome::Fatal(e);
    }
    if delivered.load(Ordering::Relaxed) {
        return LiveOutcome::Delivered;
    }
    if cancel.is_cancelled() {
        return LiveOutcome::Cancelled;
    }
    // Seen/served but nobody completed the pull → retract the dangling offer and
    // report not-connected so the caller falls back to the relay / keeps waiting.
    let _ = presence::retract_offer(
        &inner.client,
        relay,
        recipient,
        &posted.id,
        &posted.poster_token,
    )
    .await;
    LiveOutcome::NotConnected
}

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
) -> std::result::Result<DepositOutcome, flow::DepositError> {
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
    .await?;
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
    .map_err(|e| flow::DepositError::Unavailable(format!("deliver offer: {e:#}")))?;
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
        .filter(|(_, t)| {
            matches!(
                t.status,
                TransferStatus::Completed | TransferStatus::Cancelled | TransferStatus::Failed(_)
            )
        })
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
    // Swarm: announce this server to the tracker with a FULL bitfield (a sender/
    // seeder has every piece), so receivers can discover it as a peer beyond the
    // ticket. Only for a relay-embedded (swarm) anonymous ticket.
    let swarm_cancel = cancel.child_token();
    if let Ok(t) = crate::chunked::ChunkTicket::decode(&session.ticket) {
        if matches!(&t.key, crate::chunked::KeyDelivery::Plain(_)) {
            if let (Some(r), Some(addr)) = (&t.relay, t.providers.first()) {
                if let Ok(my_addr) = crate::chunked::encode_addr(addr) {
                    let n = t.chunks.len();
                    flow::spawn_swarm_coordinator(
                        inner.client.clone(),
                        r.http.clone(),
                        crate::swarm::swarm_id(&t.chunks, t.total_size),
                        my_addr,
                        Arc::new(std::sync::atomic::AtomicUsize::new(n)),
                        n,
                        Arc::new(Mutex::new(Vec::new())),
                        swarm_cancel.clone(),
                    );
                }
            }
        }
    }
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
            SendEvent::Peers { count } => inner_cb.set_download_peers(id, count),
            SendEvent::Ready { .. }
            | SendEvent::ReceiverConnected
            | SendEvent::ReceiverDropped { .. }
            | SendEvent::Backfilled
            | SendEvent::BackfillFailed { .. }
            | SendEvent::RelayCapped { .. } => {}
        })
        .await;
    swarm_cancel.cancel(); // stop announcing / deregister from the tracker
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
    // Drop any held `send --to` delivery state (memory + its durable record).
    drop_held(inner, id);
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

/// Write a resume record with owner-only permissions (`0o600`) on unix. These
/// records embed the ticket, which for a `Plain` key delivery carries the file's
/// content key in the clear — so another local user must not be able to read them.
/// Non-unix keeps the default perms (accepted limitation).
fn write_record_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn persist_download(dir: &Path, rec: &DownloadRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&download_record_path(dir, rec.id), &bytes);
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
    /// A file this send *owns* and should delete when it's removed (cancel /
    /// finish) — e.g. the staged archive tar a seeder keeps only to serve. `None`
    /// for a normal send of a user's own file (never delete that).
    #[serde(default)]
    owned_stage: Option<String>,
}

fn send_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("send-{id}.pc"))
}

fn persist_send(dir: &Path, rec: &SendRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&send_record_path(dir, rec.id), &bytes);
    }
}

fn remove_send(dir: &Path, id: u64) {
    let path = send_record_path(dir, id);
    // Delete any file this send owns (a staged archive tar we kept only to seed)
    // before dropping the record, so a cancelled session leaves nothing behind.
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(rec) = postcard::from_bytes::<SendRecord>(&bytes) {
            if let Some(stage) = &rec.owned_stage {
                let _ = std::fs::remove_file(stage);
            }
        }
    }
    let _ = std::fs::remove_file(path);
}

fn load_sends(dir: &Path) -> Vec<SendRecord> {
    load_records(dir, "send-")
}

/// On-disk record of an in-progress `send --to` delivery, so the daemon can
/// restore it after a restart: a paused one comes back paused, an active one
/// resumes delivering. Small (a recipient id + a path + status).
#[derive(serde::Serialize, serde::Deserialize)]
struct SendToRecord {
    id: u64,
    recipient: Vec<u8>,
    payload: String,
    name: String,
    archive: bool,
    paused: bool,
    reason: String,
}

fn sendto_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("sendto-{id}.pc"))
}

fn persist_sendto(dir: &Path, rec: &SendToRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&sendto_record_path(dir, rec.id), &bytes);
    }
}

fn remove_sendto(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(sendto_record_path(dir, id));
}

fn load_sendtos(dir: &Path) -> Vec<SendToRecord> {
    load_records(dir, "sendto-")
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
