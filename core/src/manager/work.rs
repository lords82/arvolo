use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::crypto::PublicId;
use crate::flow::{self, SendEvent};
use crate::presence::{self, Offer};
use crate::transfer::RelayChoice;

use super::*;

use super::records::*;
use super::state::*;

/// How many finished transfers to keep in the list before pruning the oldest.
pub(super) const MAX_FINISHED_RETAINED: usize = 512;
/// How often a listening client refreshes its presence beacon (< the relay's
/// `PRESENCE_TTL` of ~30s, so it never lapses while online).
pub(super) const BEACON_REFRESH_SECS: u64 = 10;
/// TTL of an offline (mailbox) deposit + its inbox offer: long enough for the
/// recipient to come back within a week.
pub(super) const OFFLINE_TTL_SECS: u64 = 7 * 24 * 3600;
/// Phase 1 of the live-send watchdog: how long to wait for the offer to be *seen*
/// by a live recipient poll before concluding presence was stale and falling back.
pub(super) const LIVE_CONFIRM_SECS: u64 = 12;
/// Phase 2: once the offer is seen, how long to let the P2P connection establish
/// (cross-internet iroh cold-start + hole-punch is highly variable) before giving
/// up and falling back to the mailbox.
pub(super) const LIVE_CONNECT_SECS: u64 = 90;
/// How often the watchdog polls the offer's seen-status during phase 1.
pub(super) const OFFER_STATUS_POLL_SECS: u64 = 2;
/// How long (and how often) a stay-open sender polls to confirm an offline blob
/// was fetched before leaving the transfer as merely "deposited".
pub(super) const OFFLINE_CONFIRM_SECS: u64 = 90;
pub(super) const OFFLINE_CONFIRM_POLL_SECS: u64 = 3;

/// How long to hold before re-trying the relay after it was unavailable/errored.
pub(super) const RELAY_RETRY_SECS: u64 = 15 * 60;
/// How often to re-check the recipient's presence while a send is held.
pub(super) const WAITING_POLL_SECS: u64 = 15;
/// Upper bound on how long a held (`Waiting`) send keeps trying before it gives up
/// and fails — so a too-large file to a recipient who never returns doesn't pin a
/// payload and a task forever. Mirrors the offline mailbox/offer lifetime.
pub(super) const MAX_WAITING_SECS: u64 = OFFLINE_TTL_SECS;

/// Outcome of one live-P2P delivery attempt to a recipient believed online.
pub(super) enum LiveOutcome {
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

pub(super) fn set_waiting(inner: &Arc<Inner>, id: u64, reason: &str) {
    // Live status: keep the cancel token so pause/cancel still reach the loop.
    inner.set_status_live(id, TransferStatus::Waiting(reason.to_string()));
    inner.emit(ManagerEvent::Waiting {
        id,
        reason: reason.to_string(),
    });
}

/// Write/refresh the on-disk record for a held `send --to` (for durable restore).
pub(super) fn persist_held_record(inner: &Inner, id: u64, paused: bool, reason: &str) {
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
                note: h.note.clone(),
            },
        );
    }
}

/// Forget a `send --to`'s delivery state (memory + disk) — called on any terminal
/// end (delivered, deposited, cancelled, failed).
pub(super) fn drop_held(inner: &Inner, id: u64) {
    inner.held.lock().unwrap().remove(&id);
    if let Some(dir) = &inner.state_dir {
        remove_sendto(dir, id);
    }
}

/// Move a held send to the `Paused` state (keeps its delivery state so it can be
/// resumed) and persist that so a restart restores it paused.
pub(super) fn set_paused(inner: &Arc<Inner>, id: u64, reason: &str) {
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
pub(super) fn handle_stop(inner: &Arc<Inner>, id: u64, pause_flag: &std::sync::atomic::AtomicBool) {
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
pub(super) async fn deliver_to(
    inner: Arc<Inner>,
    id: u64,
    cancel: CancellationToken,
    relay: String,
    recipient: PublicId,
    payload: PathBuf,
    name: String,
    archive: bool,
    note: String,
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
                &inner, id, &cancel, &relay, &recipient, &payload, &name, archive, &note,
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
            match deposit_offline_and_offer(&inner, &relay, &recipient, &payload, &name, &note)
                .await
            {
                Ok(out) => {
                    drop_held(&inner, id);
                    spawn_offline_confirm(
                        &inner,
                        id,
                        relay.clone(),
                        out,
                        unix_now().saturating_add(OFFLINE_TTL_SECS),
                    );
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
pub(super) async fn serve_live_once(
    inner: &Arc<Inner>,
    id: u64,
    cancel: &CancellationToken,
    relay: &str,
    recipient: &PublicId,
    payload: &Path,
    name: &str,
    archive: bool,
    note: &str,
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
        note: note.to_string(),
        sender_name: inner.display_name.lock().unwrap().clone(),
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
pub(super) struct DepositOutcome {
    pub(super) size: u64,
    pub(super) claim: String,
    /// Sender-only secret authorizing removal of the blob from the relay.
    pub(super) revoke_token: String,
    /// The offer left in the recipient's inbox + its retract token.
    pub(super) offer_id: String,
    pub(super) poster_token: String,
}

/// Deposit `payload` to the relay mailbox (sealed to `recipient`) and post a
/// long-lived `arvm` offer pointing at it. Shared by the up-front offline path and
/// the live-send watchdog fallback.
pub(super) async fn deposit_offline_and_offer(
    inner: &Inner,
    relay: &str,
    recipient: &PublicId,
    payload: &Path,
    name: &str,
    note: &str,
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
        note: note.to_string(),
        sender_name: inner.display_name.lock().unwrap().clone(),
    };
    let posted = presence::post_offer(
        &inner.client,
        relay,
        recipient,
        &inner.me,
        &offer,
        Some(OFFLINE_TTL_SECS),
    )
    .await
    .map_err(|e| flow::DepositError::Unavailable(format!("deliver offer: {e:#}")))?;
    Ok(DepositOutcome {
        size,
        claim,
        revoke_token: deposited.revoke_token,
        offer_id: posted.id,
        poster_token: posted.poster_token,
    })
}

/// Cancel an **awaiting-pickup deposit**: withdraw the blob from the relay and
/// retract the offer from the recipient's inbox, so they can no longer fetch it,
/// then mark the transfer Cancelled. Runs detached (the caller's `cancel` is sync).
///
/// A record written before cancellation was supported carries no revoke token; we
/// can't withdraw those, so the row is cancelled locally and the blob simply
/// lapses at its TTL. Both relay calls are best-effort: a failed revoke must not
/// leave the row stuck, and the relay expires the blob regardless.
pub(super) fn cancel_deposited(inner: Arc<Inner>, id: u64) {
    let rec = inner
        .state_dir
        .as_ref()
        .and_then(|dir| load_depositeds(dir).into_iter().find(|r| r.id == id));
    tokio::spawn(async move {
        if let Some(rec) = &rec {
            if !rec.revoke_token.is_empty() {
                let _ = flow::revoke_offline(&rec.relay, &rec.claim, &rec.revoke_token).await;
            }
            if !rec.offer_id.is_empty() {
                if let Ok(recipient) = PublicId::from_bytes(&rec.recipient) {
                    let _ = presence::retract_offer(
                        &inner.client,
                        &rec.relay,
                        &recipient,
                        &rec.offer_id,
                        &rec.poster_token,
                    )
                    .await;
                }
            }
        }
        if let Some(dir) = &inner.state_dir {
            remove_deposited(dir, id);
        }
        inner.set_status(id, TransferStatus::Cancelled);
        inner.emit(ManagerEvent::Cancelled { id });
    });
}

/// Poll the relay until the deposited blob `claim` is fetched (delivered) or the
/// confirmation window elapses. On delivery, flip the transfer to Completed and
/// emit it; otherwise leave it as Deposited. Runs as a detached task.
pub(super) async fn confirm_offline_delivery(
    inner: Arc<Inner>,
    id: u64,
    relay: String,
    claim: String,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(OFFLINE_CONFIRM_SECS);
    loop {
        if matches!(
            flow::claim_status(&relay, &claim).await,
            Ok(flow::ClaimStatus::Gone)
        ) {
            inner.set_status(id, TransferStatus::Completed);
            inner.emit(ManagerEvent::Completed { id, path: None });
            // Pickup confirmed — the durable deposited record has served its purpose.
            if let Some(dir) = &inner.state_dir {
                remove_deposited(dir, id);
            }
            return;
        }
        if std::time::Instant::now() >= deadline {
            return; // stays Deposited; the blob still lives on the relay
        }
        tokio::time::sleep(std::time::Duration::from_secs(OFFLINE_CONFIRM_POLL_SECS)).await;
    }
}

/// Mark a transfer as deposited-to-mailbox and start confirming its delivery.
/// Persists a durable record so a daemon restart restores the row (and resumes
/// confirming) instead of silently dropping an awaiting-pickup deposit.
/// `expires` is the blob's relay-side expiry (unix secs): a fresh deposit passes
/// now + TTL; a restore passes the original value so restarts never extend it.
pub(super) fn spawn_offline_confirm(
    inner: &Arc<Inner>,
    id: u64,
    relay: String,
    out: DepositOutcome,
    expires: u64,
) {
    let DepositOutcome {
        size,
        claim,
        revoke_token,
        offer_id,
        poster_token,
    } = out;
    inner.set_progress(id, size);
    inner.set_status(id, TransferStatus::Deposited);
    inner.emit(ManagerEvent::Deposited { id });
    if let Some(dir) = &inner.state_dir {
        let (recipient, name) = {
            let transfers = inner.transfers.lock().unwrap();
            let t = transfers.get(&id);
            (
                t.and_then(|t| t.peer.as_ref().map(|p| p.to_bytes().to_vec()))
                    .unwrap_or_default(),
                t.map(|t| t.name.clone()).unwrap_or_default(),
            )
        };
        persist_deposited(
            dir,
            &DepositedRecord {
                id,
                recipient,
                name,
                size,
                relay: relay.clone(),
                claim: claim.clone(),
                expires,
                revoke_token,
                offer_id,
                poster_token,
            },
        );
    }
    tokio::spawn(confirm_offline_delivery(inner.clone(), id, relay, claim));
}

/// Unix seconds now (0 on a pre-epoch clock — never panics).
pub(super) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Drop the oldest finished transfers (lowest ids) so at most `keep` remain.
/// Active transfers are never pruned.
pub(super) fn prune_finished(transfers: &mut HashMap<u64, Transfer>, keep: usize) {
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
pub(super) async fn serve_session(
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
                    // A sender/seeder has every piece: announce a full bitfield.
                    let mut full = crate::swarm::bitfield_new(n);
                    for i in 0..n {
                        crate::swarm::bitfield_set(&mut full, i);
                    }
                    flow::spawn_swarm_coordinator(
                        inner.client.clone(),
                        r.http.clone(),
                        crate::swarm::swarm_id(&t.chunks, t.total_size),
                        my_addr,
                        Arc::new(Mutex::new(full)),
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
pub(super) fn finish(inner: &Inner, id: u64, cancelled: bool, result: Result<Option<PathBuf>>) {
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
