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
    /// [`Offer::sender_name`].
    pub fn set_display_name(&self, name: String) {
        *self.inner.display_name.lock().unwrap() = name;
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
            h.note,
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
        // back to a ticket-derived name if nothing usable remains.
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
            // Treat a stopped-but-incomplete outcome (user cancel OR disk-full pause)
            // as not-completed, so it is never recorded/seeded as a finished download.
            let stopped_incomplete = matches!(&result, Ok(o) if !o.is_complete());
            finish(
                &inner,
                id,
                cancelled.is_cancelled() || stopped_incomplete,
                result.map(|o| Some(o.into_path())),
            );
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
