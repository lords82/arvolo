use std::collections::HashMap;
use std::path::PathBuf;
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
/// How often a sender re-checks whether a deposited blob was fetched. The check
/// starts at `POLL_SECS` (a pickup moments after the deposit shows up fast) and
/// backs off geometrically to `POLL_MAX_SECS`, running for the blob's whole life
/// rather than a short window: a recipient who collects on day 5 is the normal
/// case, not an edge one. At the cap that's ~700 requests over a 7-day TTL.
pub(super) const OFFLINE_CONFIRM_POLL_SECS: u64 = 3;
pub(super) const OFFLINE_CONFIRM_POLL_MAX_SECS: u64 = 15 * 60;
/// How far before the relay-side expiry the pickup poll takes its last look, and
/// why the verdict can be trusted at all.
///
/// The relay deletes a blob on **both** fetch and expiry, so "gone" alone doesn't
/// say which happened — and once the row is deleted the relay has forgotten the
/// download count too, so the answer is unrecoverable afterwards. Time is what
/// disambiguates: gone *before* the TTL can only be a fetch. Stopping this much
/// earlier keeps that inference off the boundary, where the two causes meet.
///
/// Note the margin is not covering a clock *offset* between us and the relay: a TTL
/// is a duration, and both sides deadlined at `created + ttl` from the same moment
/// by their own clock, so a standing offset cancels out. What it covers is the two
/// clocks disagreeing about how long that duration lasted — drift across the week,
/// or either one being stepped (NTP, a suspended VM) — plus the round-trip of the
/// look itself. The cost is a pickup in the final minutes reading as an expiry — a
/// far better error than the reverse (claiming delivery that never happened).
pub(super) const OFFLINE_CONFIRM_MARGIN_SECS: u64 = 120;
/// How long cancelling an awaiting-pickup deposit may spend withdrawing the blob
/// from the relay before it gives up and ends the row anyway. The user is waiting
/// on this: the row can't flip to Cancelled until the withdrawal returns.
pub(super) const CANCEL_RELAY_SECS: u64 = 10;

/// How long to hold before re-trying the relay after it was unavailable/errored.
pub(super) const RELAY_RETRY_SECS: u64 = 15 * 60;
/// How often to re-check the recipient's presence while a send is held.
pub(super) const WAITING_POLL_SECS: u64 = 15;
/// Backoff bounds between live attempts that found nobody, doubling from the
/// first to the second. A live attempt is not free to repeat: it posts an offer
/// into the recipient's inbox, which is a capped resource and, on their side, a
/// notification. Against a stale beacon — online by the relay's reckoning, nobody
/// answering — the un-backed-off loop spent one of those every few seconds.
///
/// The cap is minutes rather than hours because the case on the other side of the
/// trade is real too: a recipient who genuinely comes back should not wait long
/// for the P2P window. Nothing is lost meanwhile — the mailbox path runs in
/// parallel and delivers whether or not they ever appear.
pub(super) const LIVE_RETRY_MIN_SECS: u64 = 30;
pub(super) const LIVE_RETRY_MAX_SECS: u64 = 5 * 60;

/// Optional bounds on a share's life, from `ARVOLO_SHARE_COPIES` /
/// `ARVOLO_SHARE_DAYS` (config keys `share_copies` / `share_days`).
///
/// Unset means what it has always meant: a share serves until it is stopped by
/// hand. That is the right default — the file is meant to be fetchable, and
/// guessing when someone has stopped needing it is not the engine's call. But
/// unbounded has a cost that accrues quietly: every completed download turns into
/// a share, each one persisted and resumed at every restart, so the list grows by
/// one row per file ever received and the machine keeps announcing itself to the
/// swarm for all of them. A limit is how someone says "keep it available, but not
/// forever" without having to remember to come back and tidy up.
///
/// 0 is read as "no limit" rather than "stop immediately": a limit of zero copies
/// would make a share that can never serve anybody, which nobody means to ask for.
fn share_copies_limit() -> Option<u64> {
    std::env::var("ARVOLO_SHARE_COPIES")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
}

fn share_days_limit() -> Option<u64> {
    std::env::var("ARVOLO_SHARE_DAYS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
}
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
                // A handle's label is its original path: the best a restart can
                // do is retry by name (and fail visibly if the daemon may not).
                payload: h.source.label(),
                name: h.name.clone(),
                archive: h.archive,
                paused,
                reason: reason.to_string(),
                note: h.note.clone(),
            },
        );
    }
}

/// What a payload looked like when its digests were taken.
///
/// A persisted preparation skips the read-and-encrypt pass, and that pass is also
/// the only thing that would notice the file changed underneath it. This is the
/// cheap stand-in: metadata that a rewrite, a truncation or a replacement almost
/// always disturbs. `dev`/`ino` catch replacement outright on unix; elsewhere they
/// are zero and only length and mtime speak.
///
/// Not a proof. A tool that restores a file's mtime, or an in-place rewrite of
/// identical length within one filesystem timestamp tick, gets past it — and then
/// the sender serves ciphertext its ticket does not describe, every chunk fails the
/// receiver's integrity check, and the transfer simply never completes. Never a
/// file that arrives wrong.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct PayloadStamp {
    pub(super) len: u64,
    pub(super) mtime_secs: u64,
    pub(super) mtime_nanos: u32,
    pub(super) dev: u64,
    pub(super) ino: u64,
}

/// Stamp the file at `path`. `None` when it cannot be stat'ed or carries no mtime —
/// and no stamp means no persisted preparation, because an unguarded one is worse
/// than none: it would be believed.
pub(super) fn payload_stamp(path: &std::path::Path) -> Option<PayloadStamp> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    #[cfg(unix)]
    let (dev, ino) = {
        use std::os::unix::fs::MetadataExt;
        (md.dev() as u64, md.ino())
    };
    #[cfg(not(unix))]
    let (dev, ino) = (0u64, 0u64);
    Some(PayloadStamp {
        len: md.len(),
        mtime_secs: mtime.as_secs(),
        mtime_nanos: mtime.subsec_nanos(),
        dev,
        ino,
    })
}

/// Write down the preparation a held send has just paid for, so a restarted daemon
/// resumes the same send instead of minting a new one.
///
/// Two stamps, not one. A single stat after the pass has a hole in it: a file
/// rewritten *while* the pass ran would be certified by digests taken across mixed
/// content. Taking one before and one after and requiring them to agree closes it —
/// and when they disagree we write nothing, so the next attempt pays for the pass
/// again, which is exactly what happened before this record existed.
fn persist_prep_record(
    inner: &Inner,
    id: u64,
    payload: &crate::source::SendSource,
    prep: &flow::PrepSlot,
    before: Option<PayloadStamp>,
) {
    let Some(dir) = &inner.state_dir else { return };
    let path = PathBuf::from(payload.label());
    let (Some(before), Some(after)) = (before, payload_stamp(&path)) else {
        return;
    };
    if before != after {
        tracing::info!(
            "not writing down the preparation for {}: it changed while being read",
            path.display()
        );
        return;
    }
    let guard = prep.lock().unwrap();
    let Some(p) = guard.as_ref() else { return };
    persist_prep(
        dir,
        &PrepRecord {
            id,
            key: p.key().to_vec(),
            node_seed: p.node_seed().to_vec(),
            total_size: p.total_size(),
            chunks: p.chunks().to_vec(),
            payload: path.to_string_lossy().into_owned(),
            len: after.len,
            mtime_secs: after.mtime_secs,
            mtime_nanos: after.mtime_nanos,
            dev: after.dev,
            ino: after.ino,
        },
    );
}

/// Rebuild a preparation from what was written down, if the payload is still the
/// bytes it was taken from. `None` — for any reason at all — means preparing again,
/// which is what every restart did before.
pub(super) fn rebuild_prep(
    rec: &PrepRecord,
    source: &crate::source::SendSource,
) -> Option<flow::ReusablePrep> {
    let path = PathBuf::from(source.label());
    let now = payload_stamp(&path)?;
    let then = PayloadStamp {
        len: rec.len,
        mtime_secs: rec.mtime_secs,
        mtime_nanos: rec.mtime_nanos,
        dev: rec.dev,
        ino: rec.ino,
    };
    if now != then || rec.len != rec.total_size {
        tracing::info!(
            "preparing {} again: it is not the file the stored preparation describes",
            path.display()
        );
        return None;
    }
    let key: [u8; crate::crypto::CHUNK_KEY_LEN] = rec.key.as_slice().try_into().ok()?;
    let node_seed: [u8; 32] = rec.node_seed.as_slice().try_into().ok()?;
    match flow::ReusablePrep::from_parts(key, node_seed, rec.total_size, rec.chunks.clone()) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::info!(
                "stored preparation for {} is unusable: {e:#}",
                path.display()
            );
            None
        }
    }
}

/// The preparation slot a held `send --to` owns, so the delivery loop reads it from
/// something that outlives the loop. A send with no held entry gets a private slot:
/// the reuse within one task still works, which is what the slot did before it had
/// anywhere longer-lived to live.
pub(super) fn held_prep(inner: &Inner, id: u64) -> flow::PrepSlot {
    inner
        .held
        .lock()
        .unwrap()
        .get(&id)
        .map(|h| h.prep.clone())
        .unwrap_or_default()
}

/// Forget a `send --to`'s delivery state (memory + disk) — called on any terminal
/// end (delivered, deposited, cancelled, failed).
pub(super) fn drop_held(inner: &Inner, id: u64) {
    inner.held.lock().unwrap().remove(&id);
    if let Some(dir) = &inner.state_dir {
        remove_sendto(dir, id);
        // The preparation goes with it: it is only meaningful as this send's, and
        // it holds a content key.
        remove_prep(dir, id);
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
    payload: crate::source::SendSource,
    name: String,
    archive: bool,
    note: String,
    pause_flag: Arc<std::sync::atomic::AtomicBool>,
) {
    // The offer outlives the individual attempts now (see `deliver_to_inner`), so
    // somebody has to own its end. The loop has a dozen ways out and every one of
    // them means the same thing for the offer — nobody is serving it any more — so
    // the withdrawal belongs here, once, rather than at each `return`.
    let mut standing: Option<StandingOffer> = None;
    deliver_to_inner(
        &inner,
        id,
        &cancel,
        &relay,
        &recipient,
        &payload,
        &name,
        archive,
        &note,
        &pause_flag,
        &mut standing,
    )
    .await;
    if let Some(s) = standing {
        let _ = presence::retract_offer(
            &inner.client,
            &relay,
            &recipient,
            &s.posted.id,
            &s.posted.poster_token,
        )
        .await;
    }
}

/// The offer a held `send --to` keeps on the relay, and the last status we read
/// for it. The pair travels together because the wake-up is a *change* of status:
/// asking "is it still what I last saw?" needs both halves, and separating them
/// invites asking with a reading that belongs to an offer already replaced.
pub(super) struct StandingOffer {
    posted: presence::PostedOffer,
    seen: presence::OfferStatus,
}

#[allow(clippy::too_many_arguments)]
async fn deliver_to_inner(
    inner: &Arc<Inner>,
    id: u64,
    cancel: &CancellationToken,
    relay: &str,
    recipient: &PublicId,
    payload: &crate::source::SendSource,
    name: &str,
    archive: bool,
    note: &str,
    pause_flag: &Arc<std::sync::atomic::AtomicBool>,
    standing: &mut Option<StandingOffer>,
) {
    use std::time::{Duration, Instant};
    let mut too_big = false;
    let mut next_relay_try = Instant::now();
    let give_up_at = Instant::now() + Duration::from_secs(MAX_WAITING_SECS);
    // Backoff for live attempts that find nobody. Retrying on the plain poll
    // interval against a presence beacon that says "online" while nobody ever
    // connects spends one whole preparation every few seconds — eighty-four of
    // them, in the case that prompted this. Growing the gap costs a slower
    // reconnect when they really do come back, which the wake-up below and the
    // mailbox path both cover.
    let mut live_backoff = Duration::ZERO;
    let mut next_live_try = Instant::now();
    // Set when the standing offer's status moves under us: somebody read it, or took
    // it. That is better evidence of a recipient being there than a presence beacon
    // — it is them *acting on this send* — and it is the only evidence there is for
    // one running a bare `arvolo recv`, which reads its inbox and dials without ever
    // publishing a beacon. Without this the wake-up would fire into a presence check
    // that says "offline" and the attempt would be skipped.
    let mut proven_present = false;
    // The chunk digests, and the key and seed they belong to. Recomputing them per
    // attempt made a large file spend most of its life re-encrypting itself between
    // tries — and minted a new ticket each time, so an offer the recipient had
    // already seen went stale. See [`flow::ReusablePrep`].
    //
    // Read from the held send rather than owned here: this task is what a pause
    // destroys, and the preparation must not be. `Held` survives it.
    let prep = held_prep(inner, id);
    loop {
        if cancel.is_cancelled() {
            handle_stop(inner, id, pause_flag);
            return;
        }
        // Held too long with no delivery → auto-pause (keep it, tell the user) so a
        // too-large file to a recipient who never returns doesn't retry forever.
        if Instant::now() >= give_up_at {
            set_paused(
                inner,
                id,
                &format!(
                    "still undelivered after {} days — paused; resume to keep trying, or cancel",
                    MAX_WAITING_SECS / 86_400
                ),
            );
            return;
        }

        // 1) Recipient online → deliver live P2P.
        //
        // Unless P2P is off, in which case there is no live path to take and the
        // probe would only be spent to learn something we cannot act on. Worse than
        // wasteful, before this check: `serve_live_once` would reach the endpoint
        // bind, get the refusal, and report it as `Fatal` — so turning P2P off used
        // to *fail* every send to a recipient who happened to be online, instead of
        // quietly depositing it as the whole point of the setting is.
        let online = crate::transfer::p2p_enabled()
            && Instant::now() >= next_live_try
            && (std::mem::take(&mut proven_present)
                || presence::check_online(&inner.client, relay, recipient)
                    .await
                    .unwrap_or(false));
        if online {
            match serve_live_once(
                inner, id, cancel, relay, recipient, payload, name, archive, note, &prep, standing,
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
                    finish(inner, id, false, Ok(None));
                    return;
                }
                LiveOutcome::Cancelled => {
                    handle_stop(inner, id, pause_flag);
                    return;
                }
                LiveOutcome::Fatal(e) => {
                    finish(inner, id, false, Err(e));
                    return;
                }
                LiveOutcome::NotConnected => {
                    // Their beacon said online and nothing came of it. Wait longer
                    // before spending another offer on the same claim.
                    live_backoff = match live_backoff {
                        Duration::ZERO => Duration::from_secs(LIVE_RETRY_MIN_SECS),
                        d => (d * 2).min(Duration::from_secs(LIVE_RETRY_MAX_SECS)),
                    };
                    next_live_try = Instant::now() + live_backoff;
                }
            }
        }

        // 2) Try the relay mailbox — unless it already refused as too large, or
        //    we're backing off after it was unavailable.
        if !too_big && Instant::now() >= next_relay_try {
            // Interruptible: the upload is the longest thing this loop does, and a
            // cancel that is only noticed once it *finishes* is a cancel that does
            // nothing for as long as the file takes — minutes on a big one, with the
            // UI saying "cancelling…" throughout. Dropping the future here abandons
            // the request in flight; the partial blob expires on the relay with its
            // slot, which is what an incomplete deposit does anyway.
            let opts = MailboxOpts::default();
            let attempt =
                deposit_offline_and_offer(inner, relay, recipient, payload, name, note, &opts);
            let outcome = tokio::select! {
                _ = cancel.cancelled() => {
                    handle_stop(inner, id, pause_flag);
                    return;
                }
                out = attempt => out,
            };
            match outcome {
                Ok(out) => {
                    drop_held(inner, id);
                    spawn_offline_confirm(
                        inner,
                        id,
                        relay.to_string(),
                        out,
                        unix_now().saturating_add(OFFLINE_TTL_SECS),
                    );
                    return;
                }
                Err(flow::DepositError::TooLarge) => {
                    too_big = true;
                    set_waiting(
                        inner,
                        id,
                        "file too large for this relay — waiting for the recipient to come online for a direct transfer",
                    );
                }
                Err(flow::DepositError::Fatal(e)) => {
                    finish(inner, id, false, Err(e));
                    return;
                }
                Err(e @ flow::DepositError::Unavailable(_)) => {
                    set_waiting(
                        inner,
                        id,
                        &format!(
                            "{e} — retrying later, and delivering P2P as soon as the recipient is online"
                        ),
                    );
                    next_relay_try = Instant::now() + Duration::from_secs(RELAY_RETRY_SECS);
                }
            }
        }

        // 3) Wait for something to change, then go round again (reacting at once to
        //    pause/cancel).
        //
        // "Something" is, above all, the recipient accepting. While an offer of ours
        // stands we spend the wait held open on its status instead of asleep: the
        // relay answers the moment they ack, so an accept lands here in a round trip
        // rather than at the end of a backoff that grows to five minutes. That gap
        // was the whole bug — a file too large for the mailbox can *only* move while
        // both ends are awake together, and the sender was reliably asleep for the
        // several minutes after the recipient said yes.
        //
        // `offer_status_waiting` never returns early without news, so this doubles
        // as the idle sleep; a relay too old to hold the connection falls back to
        // exactly the interval this used to sleep.
        let woken = match standing.as_mut() {
            Some(s) => {
                let now = tokio::select! {
                    _ = cancel.cancelled() => {
                        handle_stop(inner, id, pause_flag);
                        return;
                    }
                    st = presence::offer_status_waiting(
                        &inner.client,
                        relay,
                        recipient,
                        &s.posted.id,
                        &s.posted.poster_token,
                        s.seen,
                        WAITING_POLL_SECS,
                    ) => st.ok(),
                };
                match now {
                    // An offer that expired or was withdrawn from under us is not
                    // something to keep waiting on: forget it, and the next live
                    // attempt posts a fresh one.
                    Some(presence::OfferStatus::Gone) => {
                        *standing = None;
                        false
                    }
                    // A reading we did not have before: somebody on their side just
                    // touched this offer. A relay that cannot hold the connection
                    // answers the old reading instead, and this is false — which is
                    // the polling behaviour this replaced.
                    Some(st) => {
                        let changed = st != s.seen;
                        s.seen = st;
                        changed
                    }
                    None => false,
                }
            }
            None => tokio::select! {
                _ = cancel.cancelled() => {
                    handle_stop(inner, id, pause_flag);
                    return;
                }
                _ = tokio::time::sleep(Duration::from_secs(WAITING_POLL_SECS)) => false,
            },
        };
        if woken {
            // They are there, right now — the one moment worth spending an attempt
            // on. Whatever the backoff had grown to was measuring how long nobody had
            // come; that question is answered.
            live_backoff = Duration::ZERO;
            next_live_try = Instant::now();
            proven_present = true;
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
    payload: &crate::source::SendSource,
    name: &str,
    archive: bool,
    note: &str,
    prep: &flow::PrepSlot,
    standing: &mut Option<StandingOffer>,
) -> LiveOutcome {
    // Say what this is before doing it — but only when there is something to say.
    // The first attempt reads and encrypts the whole payload (a minute and a half
    // per 10 GB on an SSD) and posts nothing anywhere: the recipient cannot know a
    // file is coming, because the offer carries a ticket this pass has not
    // produced yet. Left as `Active` the row said "0 B" for all that time, which
    // is indistinguishable from a transfer that is failing to move. A later
    // attempt reuses that work and starts serving at once, so it says nothing.
    let fresh = prep.lock().unwrap().is_none();
    if fresh {
        inner.set_status_live(id, TransferStatus::Preparing);
        inner.emit(ManagerEvent::Preparing { id });
    }
    // Stamped before the pass, compared after it — see `persist_prep_record`.
    let before = fresh
        .then(|| payload_stamp(&PathBuf::from(payload.label())))
        .flatten();
    let session = match flow::prepare_send_in_slot(
        payload.clone(),
        name,
        archive,
        Some((&inner.me, recipient)),
        Some(relay.to_string()),
        RelayChoice::from_env(),
        prep,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return LiveOutcome::Fatal(e.context("prepare send")),
    };
    // Written down *before* anyone is told about it. The invariant is that a ticket
    // in somebody's hands can be reproduced: put the offer out first and a crash in
    // between leaves the recipient holding a ticket this daemon can no longer serve,
    // which is the whole bug.
    if fresh {
        persist_prep_record(inner, id, payload, prep, before);
    }

    let offer = Offer {
        name: name.to_string(),
        size: session.total_size,
        chunks: session.chunks as u64,
        ticket: session.ticket.clone(),
        note: note.to_string(),
        sender_name: inner.display_name.lock().unwrap().clone(),
        origin: *inner.device_id.lock().unwrap(),
    };
    // Post an offer only when we don't already have one standing. The ticket is
    // byte-stable across attempts (see [`flow::prepare_send_reusing`]), so a second
    // post says nothing new — it just leaves another copy of the same offer in the
    // recipient's inbox, and splits the one thing the caller waits on across ids it
    // no longer holds. Whoever accepts the copy from an hour ago is then accepting
    // an offer nobody is listening for.
    //
    // `Gone` (expired, or withdrawn) is the one answer that needs a fresh post; an
    // unreachable relay reads as gone too, and re-posting is the safe way to be
    // wrong about that.
    if let Some(s) = standing.as_mut() {
        match presence::offer_status(
            &inner.client,
            relay,
            recipient,
            &s.posted.id,
            &s.posted.poster_token,
        )
        .await
        {
            Ok(presence::OfferStatus::Gone) => *standing = None,
            Ok(st) => s.seen = st,
            // An unreachable relay reads like a missing offer, and re-posting is the
            // safe way to be wrong about that: a duplicate costs the recipient a
            // second row, a missing one costs them the file.
            Err(_) => *standing = None,
        }
    }
    if standing.is_none() {
        match presence::post_offer(&inner.client, relay, recipient, &inner.me, &offer, None).await {
            Ok(p) => {
                *standing = Some(StandingOffer {
                    posted: p,
                    seen: presence::OfferStatus::Pending,
                })
            }
            // Can't even notify them → treat as not-connected; the caller retries.
            // Say why in the log: a silent discard here once hid a systematically
            // failing offer as an endless quiet retry loop.
            Err(e) => {
                tracing::warn!("posting offer for transfer {id} failed: {e:#}");
                return LiveOutcome::NotConnected;
            }
        }
    }
    let (offer_id, poster_token) = {
        let s = standing.as_ref().expect("an offer stands by here");
        (s.posted.id.clone(), s.posted.poster_token.clone())
    };
    // `set_status_live`, emphatically not `set_status`: the latter is for statuses
    // that mean the task is over (terminal, or paused) and so drops the transfer's
    // cancel token. Used here it threw the token away at the exact moment the send
    // started serving — leaving a running transfer that nothing could stop. Every
    // live P2P send was uncancellable from this line on: `cancel` found no token,
    // did nothing, said nothing, and the row served on for ever.
    inner.set_status_live(id, TransferStatus::Active);

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
    let wd_offer = offer_id.clone();
    let wd_token = poster_token.clone();
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
                // Anything but `Pending` proves a client of theirs was there. The
                // question here is only "is that presence real?", so `Arrived` —
                // the weakest of the three — is already enough to answer it.
                Ok(presence::OfferStatus::Arrived)
                    | Ok(presence::OfferStatus::Taken)
                    | Ok(presence::OfferStatus::Gone)
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
    let recipient_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let rc = recipient_cancelled.clone();
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
            SendEvent::RecipientCancelled => rc.store(true, Ordering::Relaxed),
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
    if recipient_cancelled.load(Ordering::Relaxed) {
        // They said no, on the wire and on purpose. Retract the standing offer
        // and end the send — retrying would re-offer a file just refused.
        let _ = presence::retract_offer(&inner.client, relay, recipient, &offer_id, &poster_token)
            .await;
        *standing = None;
        return LiveOutcome::Fatal(anyhow::anyhow!(
            "the recipient cancelled the transfer on their side"
        ));
    }
    if cancel.is_cancelled() {
        return LiveOutcome::Cancelled;
    }
    // Seen/served but nobody completed the pull. The offer *stays*: this attempt is
    // over, the send is not, and that row is the only way the recipient can say yes
    // between attempts — and their saying yes is what wakes us (see the wait in
    // `deliver_to_inner`). Withdrawing it here is what used to make every attempt
    // post a new one, so the accept that did arrive named an offer this loop had
    // already thrown away. The caller withdraws it once, when the send really ends.
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
    /// The encoded `arvm…` ticket for the blob. The recipient's daemon does not
    /// need it (the inbox offer carries its own copy) — this is the sender's, to
    /// hand over out-of-band when the inbox route isn't wanted or isn't working.
    pub(super) ticket: String,
    /// The download cap actually asked of the relay. Carried rather than assumed:
    /// the receipt written from this outcome is what "Link e depositi" reads to
    /// tell the user how many pickups a sealed file still has, and hardcoding the
    /// burn-after-read default there made an explicit `--max 5` read back as 1.
    pub(super) max: u32,
    /// The TTL the relay **granted** (see [`flow::Deposited::ttl_secs`]) — for the
    /// same reason `max` is carried: everything that later states a deadline, from
    /// the local record to the delivery-confirmation deadline, has to state this one
    /// rather than the one we asked for.
    pub(super) ttl_secs: u64,
}

/// A sealed mailbox deposit is burn-after-read: it is sealed to one recipient, so
/// the relay only ever needs to serve it once. Named because the deposit call and
/// the [`DepositInfo`] describing it must not drift apart.
pub(super) const OFFLINE_MAX_DOWNLOADS: u32 = 1;

/// Deposit `payload` to the relay mailbox (sealed to `recipient`) and post an
/// `arvm` offer pointing at it. Shared by the up-front offline path, the
/// live-send watchdog fallback, and the explicit "deposit it whatever happens"
/// send.
///
/// `opts` is what the two callers differ on. The automatic fallback takes the
/// defaults — it is standing in for a live delivery the user asked for, and a
/// password nobody typed cannot be part of that. An explicit deposit carries
/// whatever the user chose.
pub(super) async fn deposit_offline_and_offer(
    inner: &Inner,
    relay: &str,
    recipient: &PublicId,
    payload: &crate::source::SendSource,
    name: &str,
    note: &str,
    opts: &MailboxOpts,
) -> std::result::Result<DepositOutcome, flow::DepositError> {
    let size = payload.len().unwrap_or(0);
    let ttl = opts.ttl.unwrap_or(OFFLINE_TTL_SECS);
    let max = opts.max.unwrap_or(OFFLINE_MAX_DOWNLOADS);
    let deposited = flow::deposit_offline(
        payload,
        name,
        recipient,
        &inner.me,
        relay,
        ttl,
        max,
        opts.password.as_deref(),
    )
    .await?;
    let claim = deposited.ticket.claim.clone();
    let ticket = deposited.ticket.encode();
    // From here on the relay's answer replaces our request. Posting the offer with
    // the TTL we *asked* for is what let an offer outlive its own blob: a relay
    // capping deposits at a day still keeps the offer for the week we named, so the
    // recipient sees a perfectly ordinary arrival, accepts it on day two, and gets a
    // 404 from a claim that was reaped hours earlier. Tying the offer to the granted
    // TTL makes the arrival disappear exactly when the file it points at does.
    let ttl = deposited.ttl_secs;
    let offer = Offer {
        name: name.to_string(),
        size,
        chunks: 0,
        ticket: deposited.ticket.encode(),
        note: note.to_string(),
        sender_name: inner.display_name.lock().unwrap().clone(),
        origin: *inner.device_id.lock().unwrap(),
    };
    let posted = presence::post_offer(
        &inner.client,
        relay,
        recipient,
        &inner.me,
        &offer,
        Some(ttl),
    )
    .await
    .map_err(|e| flow::DepositError::Unavailable(format!("deliver offer: {e:#}")))?;
    Ok(DepositOutcome {
        size,
        claim,
        revoke_token: deposited.revoke_token,
        offer_id: posted.id,
        poster_token: posted.poster_token,
        ticket,
        max,
        ttl_secs: ttl,
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
///
/// They are also *bounded*. The engine's HTTP client carries no timeout, so an
/// unreachable or stalled relay would keep both calls hanging — and since the row
/// only flips (and the event only fires) once they return, the user would press
/// "cancel" and watch nothing happen, with no way to tell a slow relay from a dead
/// button. A withdrawal we can't complete in [`CANCEL_RELAY_SECS`] is one the TTL
/// will have to finish for us; ending the row promptly is worth more than waiting.
pub(super) fn cancel_deposited(inner: Arc<Inner>, id: u64) {
    let rec = inner
        .state_dir
        .as_ref()
        .and_then(|dir| load_depositeds(dir).into_iter().find(|r| r.id == id));
    tokio::spawn(async move {
        if let Some(rec) = &rec {
            let withdraw = async {
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
            };
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(CANCEL_RELAY_SECS), withdraw)
                    .await;
        }
        if let Some(dir) = &inner.state_dir {
            remove_deposited(dir, id);
        }
        inner.set_status(id, TransferStatus::Cancelled);
        inner.emit(ManagerEvent::Cancelled { id });
    });
}

/// Watch a deposited blob until its fate is known: Completed once the recipient
/// fetches it, Failed once the relay-side TTL runs out with it still sitting
/// there. Backs off from seconds to `POLL_MAX_SECS` and keeps watching for the
/// blob's whole life, so a pickup on day 5 still lands. Runs as a detached task.
///
/// `expires` is the relay-side expiry (unix secs). The last look happens
/// `MARGIN` before it — see [`OFFLINE_CONFIRM_MARGIN_SECS`] for why the verdict
/// hinges on that gap.
///
/// A relay we can't reach yields no verdict at all: on error we keep retrying,
/// and if the deadline passes without a single successful look the transfer is
/// left Deposited. "I never saw it picked up" and "it wasn't picked up" are
/// different claims, and only the second one earns a terminal state.
pub(super) async fn confirm_offline_delivery(
    inner: Arc<Inner>,
    id: u64,
    relay: String,
    claim: String,
    expires: u64,
    offer: Option<(PublicId, String, String)>,
) {
    let verdict_at = expires.saturating_sub(OFFLINE_CONFIRM_MARGIN_SECS);
    let mut delay = OFFLINE_CONFIRM_POLL_SECS;
    loop {
        let look = flow::claim_info(&relay, &claim).await.ok();
        // Only asked when the blob look was inconclusive, and only when there is an
        // offer to ask about: a gone blob before the TTL already settles it, and a
        // deposit handed over as a bare ticket has nobody to have acked.
        let offer_state = match (
            &offer,
            read_deposit(look.as_ref(), None, unix_now(), verdict_at),
        ) {
            (Some((recipient, offer_id, token)), Verdict::KeepWatching) => {
                presence::offer_status(&inner.client, &relay, recipient, offer_id, token)
                    .await
                    .ok()
            }
            _ => None,
        };
        if let Some(st) = offer_state {
            inner.set_offer_status(id, st.as_str());
        }
        match read_deposit(look.as_ref(), offer_state, unix_now(), verdict_at) {
            Verdict::PickedUp => {
                complete_deposit(&inner, id);
                return;
            }
            Verdict::Expired => {
                let msg = "expired on the relay — the recipient never collected it".to_string();
                inner.set_status(id, TransferStatus::Failed(msg.clone()));
                inner.emit(ManagerEvent::Failed { id, error: msg });
                retire_record(&inner, id);
                return;
            }
            Verdict::Unknown => {
                retire_record(&inner, id);
                return;
            }
            Verdict::KeepWatching => {}
        }
        // Never sleep past the verdict: waking after the relay has already dropped
        // the row would turn a real pickup into an indistinguishable "gone".
        let wait = delay.min(verdict_at.saturating_sub(unix_now())).max(1);
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        delay = delay.saturating_mul(2).min(OFFLINE_CONFIRM_POLL_MAX_SECS);
    }
}

/// What one look at a deposited blob's status means for the transfer.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// The recipient fetched it.
    PickedUp,
    /// The TTL ran out with it still sitting on the relay.
    Expired,
    /// The deadline arrived with the relay unreachable, so we never learned
    /// anything. Distinct from `Expired`: "I never saw it picked up" is not the
    /// same claim as "it wasn't picked up", and only the latter may be shown.
    Unknown,
    /// No conclusion yet — look again later.
    KeepWatching,
}

/// Decide a deposit's fate from one status look. Pure: the network and the clock
/// are the caller's problem, which is what makes the interesting cases (a pickup
/// on day 5, a relay that was down at the deadline) testable without either.
///
/// `look` is `None` when the relay couldn't be reached. Note that only *this*
/// look counts, never an earlier one: a blob seen on day 3 says nothing about
/// day 7 if the relay went dark in between — it could have been collected the
/// whole time. An expiry verdict needs the relay to still be saying "it's here"
/// as the TTL runs out.
pub(super) fn read_deposit(
    look: Option<&flow::ClaimInfo>,
    offer: Option<presence::OfferStatus>,
    now: u64,
    verdict_at: u64,
) -> Verdict {
    // The recipient's own ack, when the deposit left an offer to be acked. It is
    // the only positive report of a *person* acting, and it needs no inference:
    // everything below this line exists because, without it, "they took it" and
    // "it expired" both looked like the blob being gone, and only the clock could
    // tell them apart. That reasoning still runs for a deposit handed over as a
    // bare ticket, which has no offer and so no ack.
    if matches!(offer, Some(presence::OfferStatus::Taken)) {
        return Verdict::PickedUp;
    }
    match look {
        // A download count is the direct answer, when the relay is new enough to
        // report it. It only ever reaches 1 here (deposits are max=1, so the fetch
        // deletes the row), but reading it keeps this correct if a multi-download
        // deposit ever grows a tracked row.
        Some(i) if i.downloads.is_some_and(|d| d >= 1) => Verdict::PickedUp,
        // Gone, and the TTL hasn't run out: only a fetch removes it this early.
        Some(i) if !i.present => Verdict::PickedUp,
        _ if now < verdict_at => Verdict::KeepWatching,
        // Still there as the TTL runs out: nobody ever came for it.
        Some(_) => Verdict::Expired,
        None => Verdict::Unknown,
    }
}

/// Retire a watched deposit's durable record: the blob is past saving either way,
/// so dropping it also stops a restart from re-watching a dead deposit.
fn retire_record(inner: &Arc<Inner>, id: u64) {
    if let Some(dir) = &inner.state_dir {
        remove_deposited(dir, id);
    }
}

/// Mark a watched deposit as picked up and retire its durable record.
fn complete_deposit(inner: &Arc<Inner>, id: u64) {
    inner.set_status(id, TransferStatus::Completed);
    inner.emit(ManagerEvent::Completed { id, path: None });
    retire_record(inner, id);
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
        max,
        // The sender's hand-delivery ticket. Returned to the caller *and* carried
        // on the event: the caller shows it once, and a front-end that only sees
        // events (a daemon filing its receipt) has no other way to keep it — and
        // it cannot be rebuilt later from the claim. See [`DepositInfo::ticket`].
        ticket,
        // Deliberately unused here: this function deadlines off the absolute
        // `expires` it is handed, precisely so a restored deposit cannot have its
        // deadline pushed out by being restored. The duration is the caller's to
        // turn into that instant, once.
        ttl_secs: _,
    } = out;
    inner.set_progress(id, size);
    inner.set_status(id, TransferStatus::Deposited);

    // Read the row once, for the event and the record both.
    let (recipient, name) = {
        let transfers = inner.transfers.lock().unwrap();
        let t = transfers.get(&id);
        (
            t.and_then(|t| t.peer.clone()),
            t.map(|t| t.name.clone()).unwrap_or_default(),
        )
    };

    // What the pickup watcher needs to ask after the offer, taken before the record
    // write below consumes the same fields. `None` when the deposit left no offer
    // (a bare `arvm…` ticket to hand over), in which case there is no ack to wait
    // for and the blob's own status is all there is.
    let offer_probe = match (&recipient, offer_id.is_empty() || poster_token.is_empty()) {
        (Some(r), false) => Some((r.clone(), offer_id.clone(), poster_token.clone())),
        _ => None,
    };

    // Emitted *before* the record is written, so it carries what a front-end needs
    // to file its own receipt rather than making it read ours back — see [`DepositInfo`].
    inner.emit(ManagerEvent::Deposited {
        id,
        info: DepositInfo {
            relay: relay.clone(),
            claim: claim.clone(),
            revoke_token: revoke_token.clone(),
            name: name.clone(),
            size,
            expires,
            max,
            recipient: recipient.clone(),
            offer_id: offer_id.clone(),
            poster_token: poster_token.clone(),
            ticket,
        },
    });

    if let Some(dir) = &inner.state_dir {
        persist_deposited(
            dir,
            &DepositedRecord {
                id,
                recipient: recipient.map(|p| p.to_bytes().to_vec()).unwrap_or_default(),
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
    tokio::spawn(confirm_offline_delivery(
        inner.clone(),
        id,
        relay,
        claim,
        expires,
        offer_probe,
    ));
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
    // A share with a day limit stops on its own. The deadline is measured from
    // when the share *first* began, carried across restarts in its sidecar — from
    // the transfer row it would restart with every daemon, and a limit that resets
    // on reboot is not a limit.
    if let Some(days) = share_days_limit() {
        let started = inner
            .transfers
            .lock()
            .unwrap()
            .get(&id)
            .map(|t| t.share_started)
            .unwrap_or(0);
        if started > 0 {
            let deadline = started.saturating_add(days.saturating_mul(86_400));
            let left = deadline.saturating_sub(unix_now());
            let stop = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(left)).await;
                tracing::info!("share reached its {days}-day limit — stopping");
                stop.cancel();
            });
        }
    }
    let copies_cap = share_copies_limit();

    let delivered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let d = delivered.clone();
    let inner_cb = inner.clone();
    // Last progress figure seen, to turn a per-receiver running total into bytes
    // actually uploaded. A ticket serves one receiver after another and each starts
    // from zero, so a drop is a new receiver, not lost ground — count the new value
    // whole. Interleaved receivers make this an estimate, which is what it is
    // documented as; `copies_served` below is the exact number.
    let last_progress = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let lp = last_progress.clone();
    let cap_reached = cancel.clone();
    let result = session
        .serve(cancel, move |ev| match ev {
            SendEvent::Progress { transferred, total } => {
                let prev = lp.swap(transferred, Ordering::Relaxed);
                let delta = if transferred >= prev {
                    transferred - prev
                } else {
                    transferred
                };
                inner_cb.add_bytes_served(id, delta);
                inner_cb.set_progress(id, transferred);
                inner_cb.emit(ManagerEvent::Progress {
                    id,
                    transferred,
                    total_size: total,
                });
            }
            SendEvent::Delivered => {
                d.store(true, Ordering::Relaxed);
                // A whole copy left the machine. Count it, stamp it, and reset the
                // running total so the next receiver's first report isn't read as
                // a continuation of this one.
                let prev = lp.swap(0, Ordering::Relaxed);
                inner_cb.add_bytes_served(id, total.saturating_sub(prev));
                inner_cb.record_pickup(id, unix_now());
                inner_cb.persist_share_stats(id);
                // Enough copies out: stop serving. Checked here rather than on a
                // timer because this is the only moment the count can change.
                if let Some(cap) = copies_cap {
                    let served = inner_cb
                        .transfers
                        .lock()
                        .unwrap()
                        .get(&id)
                        .map(|t| t.copies_served)
                        .unwrap_or(0);
                    if served >= cap {
                        tracing::info!("share served {served} copies (limit {cap}) — stopping");
                        cap_reached.cancel();
                    }
                }
                inner_cb.set_progress(id, total);
                inner_cb.emit(ManagerEvent::Progress {
                    id,
                    transferred: total,
                    total_size: total,
                });
            }
            SendEvent::Peers { count } => inner_cb.set_download_peers(id, count),
            // A shared ticket outlives any single receiver's refusal — their
            // abort already suppressed that receiver's backfill; keep serving.
            SendEvent::RecipientCancelled => {}
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
        // The counters belonged to the share, and the share is over. Left behind
        // they would be inherited by whatever id came round to that number next.
        remove_share(dir, id);
    }
    // Drop any held `send --to` delivery state (memory + its durable record).
    drop_held(inner, id);
    // Terminal: this download stops answering for its content. A completed one
    // deliberately included — the file may since have been deleted, and telling
    // somebody "you already have this" is a different feature with a different way
    // of being wrong.
    inner.download_content.lock().unwrap().remove(&id);
    match result {
        Ok(path) if cancelled => {
            inner.set_status(id, TransferStatus::Cancelled);
            inner.emit(ManagerEvent::Cancelled { id });
            let _ = path;
        }
        Ok(path) => {
            if let Some(p) = &path {
                inner.set_path(id, p.clone());
            }
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

// ---- hosting a short pairing code -----------------------------------------

/// Drive a [`CodeHost`](crate::code::CodeHost) for a transfer: announce the code,
/// answer receivers, and persist the counters as they change.
///
/// The code and the send it points at are separate lifetimes on purpose. A
/// one-shot code retires the moment its receiver has the ticket, while the send
/// keeps serving — the download has only just started, and whoever holds the
/// ticket may reconnect for days.
pub(super) async fn code_host_task(
    inner: Arc<Inner>,
    rec: CodeRecord,
    host: crate::code::CodeHost,
    opts: crate::code::HostOpts,
    state: crate::code::HostState,
    cancel: CancellationToken,
) {
    use crate::code::HostEvent;

    let id = rec.id;
    inner.set_code(id, Some(rec.shown.clone()));
    inner.emit(ManagerEvent::CodeReady {
        id,
        code: rec.shown.clone(),
    });

    let payload = rec.payload.clone();
    let ev_inner = inner.clone();
    let st_inner = inner.clone();
    // Everything the persisted record needs that doesn't change, so the state
    // callback can rebuild it without borrowing the host it is running inside.
    let template = rec;

    let result = host
        .run(
            &payload,
            &opts,
            state,
            cancel,
            move |ev| match ev {
                HostEvent::Paired { done, .. } => {
                    ev_inner.emit(ManagerEvent::CodePaired { id, done })
                }
                HostEvent::Closed { reason } => ev_inner.emit(ManagerEvent::CodeClosed {
                    id,
                    reason: close_reason_text(reason),
                }),
                HostEvent::BadCode { failures, max, .. } => {
                    tracing::warn!("pairing code: wrong code attempt {failures}/{max}")
                }
                HostEvent::Listening | HostEvent::Rejected { .. } => {}
            },
            move |s| {
                if let Some(dir) = &st_inner.state_dir {
                    persist_code(
                        dir,
                        &CodeRecord {
                            id,
                            slot: template.slot.clone(),
                            secret: template.secret.clone(),
                            relay: template.relay.clone(),
                            owner_token: template.owner_token.clone(),
                            payload: template.payload.clone(),
                            shown: template.shown.clone(),
                            max_sessions: template.max_sessions,
                            max_failures: template.max_failures,
                            sessions_done: s.sessions_done,
                            failures: s.failures,
                        },
                    );
                }
            },
        )
        .await;

    // However it ended, the code is not coming back — don't restore it on the
    // next start, and stop showing it on the transfer.
    inner.set_code(id, None);
    if let Some(dir) = &inner.state_dir {
        remove_code(dir, id);
    }
    if let Err(e) = result {
        tracing::warn!("pairing code host stopped: {e:#}");
        inner.emit(ManagerEvent::CodeClosed {
            id,
            reason: format!("{e:#}"),
        });
    }
}

/// A one-line explanation of why a code stopped, for a UI or a log.
fn close_reason_text(reason: crate::code::CloseReason) -> String {
    use crate::code::CloseReason::*;
    match reason {
        MaxSessions => "used up — every receiver it was meant for has it".to_string(),
        TooManyFailures => "retired after too many wrong-code attempts".to_string(),
        Expired => "the rendezvous slot expired".to_string(),
        Taken => "the rendezvous slot was taken over".to_string(),
        Cancelled => "cancelled".to_string(),
    }
}

// ---- resumable-download persistence ---------------------------------------

#[cfg(test)]
mod prep_record_tests {
    use super::*;

    fn write(path: &std::path::Path, body: &[u8]) {
        std::fs::write(path, body).unwrap();
    }

    /// The stamp is the whole guard: a persisted preparation is believed on its word,
    /// and this is the word.
    #[test]
    fn a_stamp_notices_a_payload_that_moved_on() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("payload.bin");
        write(&p, b"the original bytes");
        let before = payload_stamp(&p).expect("stat");

        assert_eq!(
            Some(before),
            payload_stamp(&p),
            "an untouched file is itself"
        );

        // Different length: caught by length alone.
        write(&p, b"the original bytes, and then some");
        assert_ne!(Some(before), payload_stamp(&p));

        // Same length, replaced file: caught by the inode on unix. (On other
        // platforms this leans on the mtime, which a rewrite moves.)
        let same_len = dir.path().join("same-len.bin");
        write(&same_len, b"the original bytez");
        std::fs::rename(&same_len, &p).unwrap();
        assert_ne!(Some(before), payload_stamp(&p));

        assert!(
            payload_stamp(&dir.path().join("nothing-here")).is_none(),
            "no file, no stamp — and no stamp means no persisted preparation"
        );
    }

    /// What a restart reads. Also pins the permissions: the record carries the
    /// content key in the clear.
    #[test]
    fn a_prep_record_round_trips_privately() {
        let dir = tempfile::tempdir().unwrap();
        let rec = PrepRecord {
            id: 7,
            key: vec![3u8; crate::crypto::CHUNK_KEY_LEN],
            node_seed: vec![9u8; 32],
            total_size: 1234,
            chunks: vec![crate::hash::Hash::new(b"a"), crate::hash::Hash::new(b"b")],
            payload: "/tmp/payload.bin".into(),
            len: 1234,
            mtime_secs: 42,
            mtime_nanos: 7,
            dev: 1,
            ino: 2,
        };
        persist_prep(dir.path(), &rec);
        let back = load_prep(dir.path(), 7).expect("written and read back");
        assert_eq!(back.key, rec.key);
        assert_eq!(back.node_seed, rec.node_seed);
        assert_eq!(back.chunks, rec.chunks);
        assert_eq!(back.total_size, rec.total_size);
        assert_eq!(back.ino, rec.ino);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(prep_record_path(dir.path(), 7))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "it holds a content key");
        }

        remove_prep(dir.path(), 7);
        assert!(load_prep(dir.path(), 7).is_none());
    }

    /// The guard says no to a payload that is not the one the digests came from, and
    /// the send simply prepares again — which is what every restart used to do.
    #[test]
    fn a_changed_payload_is_prepared_again() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("payload.bin");
        write(&p, &vec![7u8; 4096]);
        let stamp = payload_stamp(&p).unwrap();
        let source = crate::source::SendSource::Path(p.clone());
        let mk = |stamp: PayloadStamp, total_size: u64| PrepRecord {
            id: 1,
            key: vec![3u8; crate::crypto::CHUNK_KEY_LEN],
            node_seed: vec![9u8; 32],
            total_size,
            // One chunk's worth of digests for a payload well under 16 MiB.
            chunks: vec![crate::hash::Hash::new(b"only-chunk")],
            payload: p.to_string_lossy().into_owned(),
            len: stamp.len,
            mtime_secs: stamp.mtime_secs,
            mtime_nanos: stamp.mtime_nanos,
            dev: stamp.dev,
            ino: stamp.ino,
        };

        assert!(
            rebuild_prep(&mk(stamp, 4096), &source).is_some(),
            "the file the digests were taken from is still there"
        );

        write(&p, &vec![7u8; 8192]);
        assert!(
            rebuild_prep(&mk(stamp, 4096), &source).is_none(),
            "a payload that changed under us must be prepared again"
        );

        // A record whose own size and length disagree is not a preparation.
        let stamp = payload_stamp(&p).unwrap();
        assert!(rebuild_prep(&mk(stamp, 4096), &source).is_none());
    }
}

#[cfg(test)]
mod deposit_verdict_tests {
    use super::{presence, read_deposit, Verdict};
    use crate::flow::ClaimInfo;

    const DEADLINE: u64 = 1_000;

    fn present() -> ClaimInfo {
        ClaimInfo {
            present: true,
            downloads: Some(0),
            max_downloads: Some(1),
        }
    }
    fn gone() -> ClaimInfo {
        ClaimInfo {
            present: false,
            downloads: None,
            max_downloads: None,
        }
    }

    /// The recipient's ack settles it on the spot, with the blob still sitting
    /// there and the TTL nowhere near — the two facts the inference below needs and
    /// cannot have this early. This is the case that used to be unanswerable.
    #[test]
    fn an_acked_offer_is_a_pickup_whatever_the_blob_says() {
        assert_eq!(
            read_deposit(
                Some(&present()),
                Some(presence::OfferStatus::Taken),
                10,
                DEADLINE
            ),
            Verdict::PickedUp
        );
    }

    /// And only `Taken`. The middle state is set by any read of their inbox — a
    /// listing as much as their daemon's poll — so treating it as a pickup would
    /// report a glance at a list as a delivered file.
    #[test]
    fn an_offer_that_only_arrived_is_not_a_pickup() {
        for st in [
            presence::OfferStatus::Pending,
            presence::OfferStatus::Arrived,
            presence::OfferStatus::Gone,
        ] {
            assert_eq!(
                read_deposit(Some(&present()), Some(st), 10, DEADLINE),
                Verdict::KeepWatching,
                "{st:?} must not conclude anything"
            );
        }
    }

    /// The regression this whole change exists for: the old watcher gave up after
    /// 90 seconds, so a recipient who collected on day 5 left the row lying about
    /// itself forever. Long after the old window, "gone" still means delivered.
    #[test]
    fn gone_long_after_the_old_window_is_still_a_pickup() {
        assert_eq!(
            read_deposit(Some(&gone()), None, 900, DEADLINE),
            Verdict::PickedUp
        );
    }

    /// An older relay reports presence only (`downloads: None`), so the pickup has
    /// to be read from the blob's disappearance alone.
    #[test]
    fn a_presence_only_relay_still_resolves_a_pickup() {
        let old = ClaimInfo {
            present: false,
            downloads: None,
            max_downloads: None,
        };
        assert_eq!(
            read_deposit(Some(&old), None, 10, DEADLINE),
            Verdict::PickedUp
        );
    }

    /// When the relay does count, the count decides — no inference needed.
    #[test]
    fn a_download_count_settles_it_directly() {
        let fetched = ClaimInfo {
            present: true,
            downloads: Some(1),
            max_downloads: Some(2),
        };
        assert_eq!(
            read_deposit(Some(&fetched), None, 10, DEADLINE),
            Verdict::PickedUp
        );
    }

    #[test]
    fn still_sitting_there_means_look_again() {
        assert_eq!(
            read_deposit(Some(&present()), None, 999, DEADLINE),
            Verdict::KeepWatching
        );
    }

    #[test]
    fn still_there_as_the_ttl_runs_out_is_an_expiry() {
        assert_eq!(
            read_deposit(Some(&present()), None, DEADLINE, DEADLINE),
            Verdict::Expired
        );
    }

    /// The verdict rides on *this* look, not on history: a relay that goes dark
    /// before the deadline could have served the file the whole time it was gone.
    /// Reporting "never collected" there would be a guess dressed as a fact.
    #[test]
    fn a_relay_dark_at_the_deadline_yields_no_verdict() {
        assert_eq!(
            read_deposit(None, None, DEADLINE, DEADLINE),
            Verdict::Unknown
        );
    }

    #[test]
    fn an_unreachable_relay_before_the_deadline_just_retries() {
        assert_eq!(
            read_deposit(None, None, 500, DEADLINE),
            Verdict::KeepWatching
        );
    }
}

#[cfg(test)]
mod share_limit_tests {
    use super::{share_copies_limit, share_days_limit};

    /// These read process-global env, which the parallel test runner shares.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with(key: &str, value: Option<&str>, f: impl FnOnce()) {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        f();
        std::env::remove_var(key);
    }

    /// Unset is the default the user chose: a share serves until it is stopped by
    /// hand. Nothing here may invent a limit nobody asked for.
    #[test]
    fn unset_means_no_limit() {
        with("ARVOLO_SHARE_COPIES", None, || {
            assert_eq!(share_copies_limit(), None)
        });
        with("ARVOLO_SHARE_DAYS", None, || {
            assert_eq!(share_days_limit(), None)
        });
    }

    #[test]
    fn a_number_is_the_limit() {
        with("ARVOLO_SHARE_COPIES", Some("5"), || {
            assert_eq!(share_copies_limit(), Some(5))
        });
        with("ARVOLO_SHARE_DAYS", Some(" 30 "), || {
            assert_eq!(share_days_limit(), Some(30))
        });
    }

    /// Zero reads as "no limit", not "stop at once": a share allowed zero copies
    /// could never serve anybody, which is not a thing anyone means to configure.
    /// Nor may nonsense turn into a limit — it falls back to unbounded, the
    /// behaviour that was there before the key existed.
    #[test]
    fn zero_and_nonsense_leave_it_unbounded() {
        with("ARVOLO_SHARE_COPIES", Some("0"), || {
            assert_eq!(share_copies_limit(), None)
        });
        with("ARVOLO_SHARE_DAYS", Some("presto"), || {
            assert_eq!(share_days_limit(), None)
        });
        with("ARVOLO_SHARE_DAYS", Some("-1"), || {
            assert_eq!(share_days_limit(), None)
        });
    }
}
