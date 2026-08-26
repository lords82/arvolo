use std::path::Path;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use crate::backfill::RelayRelease;
use crate::chunked::{ChunkSender, ChunkTicket, KeyDelivery, SeedRequest};
use crate::crypto::{seal, Identity, PublicId};
use crate::transfer::RelayChoice;

use super::CHUNK_KEY_AAD;

/// Progress events emitted while serving a file.
#[derive(Debug, Clone)]
pub enum SendEvent {
    /// The file is split, served, and the ticket is ready to hand out.
    Ready {
        chunks: usize,
        total_size: u64,
        ticket: String,
        has_relay: bool,
    },
    /// A receiver's control channel connected — it has started pulling. Lets a
    /// single-recipient sender tell "nobody showed up" from "transfer underway".
    ReceiverConnected,
    /// Send progress. `transferred` is the greater of the bytes actually pushed
    /// onto the wire and the bytes the receiver has acked (delivered chunks ×
    /// chunk size), capped at the total and never moving backwards.
    ///
    /// The wire figure is what makes this move at all early on: acks arrive one
    /// whole 16 MiB chunk at a time and the receiver pulls several at once, so
    /// the first of them lands only after four chunks have gone up. It leads
    /// delivery slightly (by what QUIC keeps in flight) and can't be trusted as
    /// "arrived" — that is what [`SendEvent::Delivered`] is for.
    Progress { transferred: u64, total: u64 },
    /// A receiver disconnected having fetched **every** chunk — the file reached
    /// it in full. (A ticket may serve several receivers, so this can fire more
    /// than once; a single-recipient sender can stop on the first.)
    Delivered,
    /// The receiver dropped; we're backfilling the undelivered tail to the relay.
    ReceiverDropped { missing: usize },
    /// The undelivered tail is now on the relay; the sender can go offline.
    Backfilled,
    /// Everything the recipient still lacks is on the relay, this ticket is sealed
    /// to that one recipient, and nobody else is pulling — so `serve` is ending on
    /// purpose rather than waiting for a cancel. The relay can finish without us.
    ///
    /// Distinct from [`Self::Backfilled`], which fires whenever a tail lands and
    /// says nothing about whether we are still needed.
    Offloaded,
    /// A backfill attempt failed (transfer can still be retried).
    BackfillFailed { reason: String },
    /// The relay refused further backfill because this transfer hit the relay's
    /// per-session offload cap (a free, shared relay bounds how much any one
    /// transfer may lean on it). The rest must go over direct P2P — or via a
    /// private relay without the cap. `limit_bytes` is the relay's cap.
    RelayCapped { limit_bytes: u64 },
    /// The number of distinct peers currently downloading from us changed
    /// (0, 1, or many — a shared ticket can serve a whole swarm).
    Peers { count: usize },
    /// The receiver cancelled ON PURPOSE (it said so on the control channel —
    /// not a crash, not a network drop). `serve` ends right after emitting
    /// this: no backfill, nothing to wait for.
    RecipientCancelled,
}

/// A prepared send: the file is split and served, and the ticket is ready. Call
/// [`SendSession::serve`] to keep serving (and lazily backfill on a drop).
pub struct SendSession {
    pub ticket: String,
    pub chunks: usize,
    pub total_size: u64,
    pub has_relay: bool,
    sender: ChunkSender,
    relay: Option<RelayRelease>,
    client: reqwest::Client,
    /// Sealed to one recipient (`--to`): their deliberate cancel ends the send.
    /// A shared ticket outlives any one receiver's refusal.
    sealed_to_recipient: bool,
}

impl SendSession {
    /// The raw per-transfer content key. The CLI persists this so an interrupted
    /// send can be resumed (its ticket stays valid). Capability secret — store
    /// it protected.
    pub fn content_key(&self) -> [u8; crate::crypto::CHUNK_KEY_LEN] {
        self.sender.key()
    }

    /// The transport secret seed this send is bound under. The CLI persists it so
    /// a resumed send rebinds the *same* node id and the old ticket reconnects.
    pub fn node_seed(&self) -> [u8; 32] {
        self.sender.node_seed()
    }

    /// True when no relay became reachable while this send was being prepared, so
    /// the ticket it produced names this network's addresses and nothing else.
    ///
    /// Not a failure — a peer on the same LAN is reached by exactly those — but
    /// the caller is the only one in a position to say so, and a wait nobody can
    /// explain is the thing worth avoiding here.
    pub fn local_only(&self) -> bool {
        self.sender.local_only()
    }
}

/// Ask the relay for its chunk-serving address + a seed token (`/v1/addr`), to
/// embed in a ticket as a [`RelayRelease`].
async fn fetch_relay_release(client: &reqwest::Client, url: &str) -> Result<RelayRelease> {
    let url = url.trim_end_matches('/').to_string();
    let resp = client
        .get(format!("{url}/v1/addr"))
        .send()
        .await
        .context("relay /v1/addr")?
        .error_for_status()
        .context("relay rejected addr")?
        .text()
        .await
        .context("read relay addr")?;
    let mut lines = resp.lines();
    let addr = lines
        .next()
        .context("missing relay address")?
        .trim()
        .to_string();
    let token = lines.next().context("missing token")?.trim().to_string();
    Ok(RelayRelease {
        http: url,
        addr,
        token,
    })
}

/// Split and serve `path`; with `seed_relay` set, learn the relay's address +
/// token so the tail can be backfilled if the receiver drops (nothing is
/// uploaded yet — that's lazy, in [`SendSession::serve`]).
pub async fn prepare_send(
    source: impl Into<crate::source::SendSource>,
    name: &str,
    archive: bool,
    to: Option<(&Identity, &PublicId)>,
    seed_relay: Option<String>,
    relay: RelayChoice,
) -> Result<SendSession> {
    prepare_send_reusing(source, name, archive, to, seed_relay, relay, &mut None).await
}

/// What a delivery loop keeps between attempts so it does not pay for the same
/// work twice: the content key, the transport seed, and the chunk digests
/// computed under that key.
///
/// Keeping all three together is not tidiness — the digests are only valid for
/// *that* key, and reusing the seed is what makes the previously handed-out
/// ticket keep working (same node id). Split them and you get a ticket that no
/// longer resolves, or hashes that do not match the ciphertext being served.
pub struct ReusablePrep {
    key: [u8; crate::crypto::CHUNK_KEY_LEN],
    node_seed: [u8; 32],
    chunks: crate::chunked::PreparedChunks,
}

impl ReusablePrep {
    /// A preparation rebuilt from parts somebody wrote down, skipping the pass that
    /// produced them.
    ///
    /// The caller takes on the guarantee that pass would have given: that the
    /// payload still holds the bytes these digests were taken from. Get that wrong
    /// and the sender serves ciphertext the ticket does not describe — every chunk
    /// fails its integrity check at the receiver, so the failure is a transfer that
    /// never completes, never a file that arrives wrong. See
    /// `manager::work::payload_stamp` for the guard the daemon uses.
    pub fn from_parts(
        key: [u8; crate::crypto::CHUNK_KEY_LEN],
        node_seed: [u8; 32],
        total_size: u64,
        chunks: Vec<crate::hash::Hash>,
    ) -> Result<Self> {
        Ok(Self {
            key,
            node_seed,
            chunks: crate::chunked::PreparedChunks::from_digests(total_size, chunks)?,
        })
    }

    /// The content key. A capability secret: whoever holds it decrypts the payload.
    pub fn key(&self) -> [u8; crate::crypto::CHUNK_KEY_LEN] {
        self.key
    }

    /// The transport seed, which is what reproduces the node id an already-handed-out
    /// ticket resolves to.
    pub fn node_seed(&self) -> [u8; 32] {
        self.node_seed
    }

    pub fn total_size(&self) -> u64 {
        self.chunks.total_size()
    }

    pub fn chunks(&self) -> &[crate::hash::Hash] {
        self.chunks.chunks()
    }
}

/// A preparation owned by something that outlives the task using it.
///
/// The delivery loop of a held `send --to` dies and respawns on every pause and
/// resume; what it must not lose therefore cannot live in it. See
/// [`prepare_send_in_slot`], and `manager::state::Held`, which is what holds the
/// other end of this `Arc`.
pub type PrepSlot = std::sync::Arc<std::sync::Mutex<Option<ReusablePrep>>>;

/// [`prepare_send_reusing`] against a slot shared with whoever outlives this task.
///
/// Take, prepare, put back — the lock is never held across the await, so the future
/// stays `Send` and two attempts can never be preparing the same slot anyway (the
/// loop that owns it is single-threaded through this call).
pub async fn prepare_send_in_slot(
    source: impl Into<crate::source::SendSource>,
    name: &str,
    archive: bool,
    to: Option<(&Identity, &PublicId)>,
    seed_relay: Option<String>,
    relay: RelayChoice,
    slot: &PrepSlot,
) -> Result<SendSession> {
    let mut prep = slot.lock().unwrap().take();
    let out = prepare_send_reusing(source, name, archive, to, seed_relay, relay, &mut prep).await;
    *slot.lock().unwrap() = prep;
    out
}

/// [`prepare_send`], reusing an earlier preparation when one is handed in.
///
/// The pass this skips reads and encrypts the whole payload — around a minute
/// and a half per 10 GB — and a delivery loop that retries while the recipient is
/// not yet connectable used to redo it on *every* attempt, spending far more time
/// re-hashing than transferring. With `prep` threaded through, the first attempt
/// pays for it and the rest start immediately; as a consequence the ticket also
/// stays byte-identical across attempts, so an offer already sitting in the
/// recipient's inbox keeps pointing at something real.
pub async fn prepare_send_reusing(
    source: impl Into<crate::source::SendSource>,
    name: &str,
    archive: bool,
    to: Option<(&Identity, &PublicId)>,
    seed_relay: Option<String>,
    relay: RelayChoice,
    prep: &mut Option<ReusablePrep>,
) -> Result<SendSession> {
    let source = source.into();
    // Only a path can be pre-checked; a handed-off descriptor IS the proof of
    // access, and may have no visible path at all.
    if let crate::source::SendSource::Path(p) = &source {
        anyhow::ensure!(p.is_file(), "{} is not a file", p.display());
    }
    let reuse = match prep.take() {
        Some(p) => p,
        None => {
            let key = crate::crypto::random_chunk_key();
            ReusablePrep {
                key,
                node_seed: crate::node::random_node_seed(),
                chunks: crate::chunked::PreparedChunks::compute(source.clone(), key).await?,
            }
        }
    };
    // Put it back *before* serving, not after: binding the endpoint can fail (a
    // port exhausted, P2P turned off between attempts) and the `?` below would then
    // throw away a pass that costs half a minute on a large file — the exact waste
    // this function exists to avoid.
    let (key, node_seed, chunks) = (reuse.key, reuse.node_seed, reuse.chunks.clone());
    *prep = Some(reuse);
    let sender = ChunkSender::serve_prepared(source, relay, key, node_seed, chunks)
        .await
        .context("start sender")?;
    let client = crate::http::client();

    // Deliver the content key: sealed to a recipient with `--to`, else in the
    // clear (the ticket itself is the capability).
    let key = match to {
        Some((me, recipient)) => {
            let sealed = seal(&sender.key(), recipient, me, CHUNK_KEY_AAD).context("seal key")?;
            KeyDelivery::Sealed {
                encapped_key: sealed.encapped_key,
                ciphertext: sealed.ciphertext,
                sender: me.public().to_bytes(),
            }
        }
        None => KeyDelivery::Plain(sender.key().to_vec()),
    };

    // Embed the relay in the ticket (enables relay backfill + the swarm) —
    // best-effort: if the relay is unreachable, fall back to a pure-P2P ticket
    // rather than failing the send. So it's safe to default this on.
    let relay = match &seed_relay {
        Some(url) => match fetch_relay_release(&client, url).await {
            Ok(rr) => Some(rr),
            Err(e) => {
                tracing::warn!("relay {url} unavailable ({e:#}); serving peer-to-peer only");
                None
            }
        },
        None => None,
    };

    let ticket = ChunkTicket {
        total_size: sender.total_size(),
        chunk_size: sender.chunk_size(),
        chunks: sender.chunks().to_vec(),
        providers: vec![sender.addr()],
        relay: relay.clone(),
        key,
        name: name.to_string(),
        archive,
    };
    Ok(SendSession {
        ticket: ticket.encode()?,
        chunks: sender.chunks().len(),
        total_size: sender.total_size(),
        has_relay: relay.is_some(),
        sealed_to_recipient: to.is_some(),
        sender,
        relay,
        client,
    })
}

/// Re-serve a file under a *previously used* content `key` and transport
/// `node_seed`, so a ticket handed out by an earlier [`prepare_send`] stays
/// valid after the sender restarted.
///
/// Two things must be reproduced for the *old* ticket to reconnect: the chunk
/// hashes and the node id. [`crate::crypto::seal_chunk`] is deterministic in
/// `(key, index)`, so the same key over the same bytes yields identical hashes;
/// and rebinding the same `node_seed` yields the same node id (which discovery
/// re-resolves to the new address). `expected` is that original ticket: we
/// recompute the hashes and require them to match, rejecting a changed/wrong
/// file up front instead of failing mid-transfer with "chunk not available".
///
/// Resume is pure P2P (no relay seeding); recovering an interrupted transfer is
/// independent of `--seed-relay`.
pub async fn resume_send(
    path: &Path,
    key: [u8; crate::crypto::CHUNK_KEY_LEN],
    node_seed: Option<[u8; 32]>,
    expected: &ChunkTicket,
    relay: RelayChoice,
) -> Result<SendSession> {
    anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
    // `Some` rebinds the original node id so the *old* ticket reconnects; `None`
    // (e.g. resuming from a plain ticket, which carries no transport secret) uses
    // a fresh id — the caller then serves the reprinted ticket.
    let node_seed = node_seed.unwrap_or_else(crate::node::random_node_seed);
    let sender = ChunkSender::serve_resume(path, relay, key, node_seed)
        .await
        .context("start sender")?;

    anyhow::ensure!(
        sender.chunks() == expected.chunks.as_slice(),
        "file no longer matches the ticket (changed, truncated, or wrong file) — \
         cannot resume this send; start a new one"
    );

    let ticket = ChunkTicket {
        total_size: sender.total_size(),
        chunk_size: sender.chunk_size(),
        chunks: sender.chunks().to_vec(),
        providers: vec![sender.addr()],
        // Keep the original relay in the ticket so a resumed sender / a seeder still
        // joins the swarm (the tracker announce in the manager keys off it). We
        // don't re-seed to the relay here — just stay discoverable.
        relay: expected.relay.clone(),
        key: expected.key.clone(),
        name: expected.name.clone(),
        archive: expected.archive,
    };
    Ok(SendSession {
        ticket: ticket.encode()?,
        chunks: sender.chunks().len(),
        total_size: sender.total_size(),
        has_relay: false,
        // A resumed send: the ticket says whether it was sealed to somebody.
        sealed_to_recipient: matches!(expected.key, crate::chunked::KeyDelivery::Sealed { .. }),
        sender,
        relay: None,
        client: crate::http::client(),
    })
}

/// The byte figure to report, from the two things the sender knows: `sent`, the
/// chunk bytes handed to QUIC, and `acked`, the chunks a receiver confirmed.
///
/// Both are already "the furthest-along receiver" by their own measure, which for a
/// shared ticket may be two different receivers — so the result is the best anyone
/// is doing, not a single receiver's position. That is the only reading of one
/// progress number that stays meaningful when a ticket is served to several people
/// at once; per-receiver progress would need per-receiver rows to show it in.
///
/// Take whichever is further along. Mid-transfer that is the wire count (acks
/// land one whole chunk at a time, and the first only after several are in
/// flight); on a *resumed* send it is the acks, since the receiver confirms
/// pieces we never re-sent this session. Then cap at the payload — the wire
/// count sums every receiver and every re-send, so a re-fetched piece would push
/// it past 100% — and never let the reported value move backwards.
fn progress_figure(sent: u64, acked_chunks: usize, chunk_size: u64, total: u64, last: u64) -> u64 {
    let acked = (acked_chunks as u64).saturating_mul(chunk_size);
    sent.max(acked).min(total).max(last)
}

/// How many chunks one seed request carries. The relay pulls every chunk in a
/// request before it answers, so this is also how long the sender goes without
/// being able to notice a cap, a cancel, or the receiver coming back. One request
/// for the whole tail — what this replaced — was uninterruptible for as long as
/// the upload took.
const SEED_BATCH: usize = 64;

/// How long the relay is asked to keep a parked payload.
///
/// The same week an inbox offer lives, and not by coincidence: the offer is what
/// names these pieces, so pieces that lapse first leave the recipient an arrival
/// pointing at nothing — the exact failure the mailbox once had when a relay
/// capped a deposit's TTL and the offer outlived the blob. The relay clamps this
/// to its own maximum and says what it granted.
pub const PARK_TTL_SECS: u64 = 7 * 24 * 3600;

/// How an offload ended. Only [`Self::Complete`] means the relay can finish the
/// job without us.
#[derive(Debug, PartialEq, Eq)]
enum Offload {
    /// Every requested piece is on the relay (some may already have been).
    ///
    /// `granted` is the lifetime the relay agreed to, when a batch was actually
    /// sent. `None` means every piece was already there, so nothing was asked and
    /// the deadline is whatever an earlier pass established — the caller falls
    /// back to what it requested, which can only be an over-estimate of what is
    /// left, never an under-estimate of what was granted.
    Complete { granted: Option<u64> },
    /// The relay refused further seeding: this transfer hit its offload cap.
    Capped,
    /// The relay has no room at all — not this transfer's budget, the whole store.
    ///
    /// Kept apart from [`Self::Failed`] for the same reason the mailbox keeps
    /// `Busy` apart from `Unavailable`: nothing is broken and nothing is
    /// unreachable, other people are simply using the relay, and it clears as
    /// their transfers finish. Worth saying to the user in those words, and worth
    /// retrying — unlike a rejection, which will keep being one.
    Full,
    /// The relay rejected or was unreachable.
    Failed,
    /// The caller cancelled mid-way; some batches may have landed.
    Cancelled,
}

impl SendSession {
    /// Put `wanted` (piece indices) on the relay — minus whatever it already
    /// holds — a batch at a time.
    ///
    /// The subtraction is the point. Before it, every drop re-sent the whole
    /// undelivered tail: the relay skips chunk files it already has, so nothing was
    /// re-fetched, but the handler still charges the transfer's offload meter for
    /// every hash in the request *before* it gets that far. A receiver that
    /// connected and dropped repeatedly could therefore burn a transfer's entire
    /// relay budget without the relay storing one new byte — and the 402 that
    /// followed cut it off from a relay that was already holding everything it
    /// needed. Asking first also repairs the sender's own memory, which does not
    /// survive a restart: `mark_on_relay` is re-primed from the answer, so the
    /// `RelayHas` we advertise describes the relay as it *is*, not as this process
    /// happens to remember it.
    async fn offload(
        &self,
        wanted: &[usize],
        ttl_secs: u64,
        cancel: &CancellationToken,
        on: &(impl Fn(SendEvent) + Send + Sync),
    ) -> Offload {
        let Some(r) = &self.relay else {
            return Offload::Failed;
        };
        let hashes: Vec<_> = wanted.iter().map(|&i| self.sender.chunks()[i]).collect();
        // A relay too old for the question answers an empty set: we then seed the
        // lot, exactly as before this existed.
        let held = crate::backfill::relay_have(&self.client, &r.http, &hashes)
            .await
            .unwrap_or_default();
        let already: Vec<usize> = held.iter().map(|&i| wanted[i as usize]).collect();
        if !already.is_empty() {
            self.sender.mark_on_relay(&already);
        }
        let todo: Vec<usize> = wanted
            .iter()
            .copied()
            .enumerate()
            .filter(|(i, _)| !held.contains(&(*i as u32)))
            .map(|(_, piece)| piece)
            .collect();
        if todo.is_empty() {
            on(SendEvent::Backfilled);
            return Offload::Complete { granted: None };
        }
        // Metered on the whole file, not this batch: the relay keys the offload
        // budget on content, so it stays one durable session across resumes.
        let swarm_id = crate::swarm::swarm_id(self.sender.chunks(), self.total_size);
        // The shortest lifetime any batch was granted: a deadline has to be the
        // earliest moment a piece can vanish, not the latest.
        let mut granted: Option<u64> = None;
        for batch in todo.chunks(SEED_BATCH) {
            if cancel.is_cancelled() {
                return Offload::Cancelled;
            }
            let req = SeedRequest {
                sender: self.sender.addr(),
                chunks: batch.iter().map(|&i| self.sender.chunks()[i]).collect(),
                token: r.token.clone(),
                swarm_id: swarm_id.clone(),
            };
            let body = match req.encode() {
                Ok(b) => b,
                Err(e) => {
                    on(SendEvent::BackfillFailed {
                        reason: e.to_string(),
                    });
                    return Offload::Failed;
                }
            };
            let url = format!("{}/v1/seed?ttl={ttl_secs}", r.http);
            let sent = tokio::select! {
                _ = cancel.cancelled() => return Offload::Cancelled,
                r = self.client.post(url).body(body).send() => r,
            };
            match sent {
                Ok(resp) if resp.status().is_success() => {
                    if let Some(ttl) = resp
                        .headers()
                        .get("x-arvolo-ttl")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<u64>().ok())
                    {
                        granted = Some(granted.map_or(ttl, |g: u64| g.min(ttl)));
                    }
                    self.sender.mark_on_relay(batch);
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::PAYMENT_REQUIRED => {
                    // The relay's per-session offload cap (free shared tier). Body
                    // carries the cap in bytes; what fits is already up there, and
                    // the rest has to go direct — or via a relay without the cap.
                    let limit_bytes = resp
                        .text()
                        .await
                        .ok()
                        .and_then(|b| b.trim().parse().ok())
                        .unwrap_or(0);
                    on(SendEvent::RelayCapped { limit_bytes });
                    return Offload::Capped;
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::INSUFFICIENT_STORAGE => {
                    // The relay is out of room for everybody, not just for us. The
                    // batches already up there stay and still help; the rest has to
                    // wait or go direct. Said in the same words the mailbox uses for
                    // the same condition, because it is the same condition.
                    on(SendEvent::BackfillFailed {
                        reason:
                            "the relay has no room right now — it clears as other transfers finish"
                                .into(),
                    });
                    return Offload::Full;
                }
                Ok(resp) => {
                    on(SendEvent::BackfillFailed {
                        reason: format!("relay rejected: {}", resp.status()),
                    });
                    return Offload::Failed;
                }
                Err(e) => {
                    on(SendEvent::BackfillFailed {
                        reason: e.to_string(),
                    });
                    return Offload::Failed;
                }
            }
        }
        on(SendEvent::Backfilled);
        Offload::Complete { granted }
    }

    /// Where this send's pieces can be parked, when a relay was configured.
    pub fn relay_release(&self) -> Option<RelayRelease> {
        self.relay.clone()
    }

    /// The chunk digests this send serves, in order — what a parked copy is
    /// addressed by, and so what a later "is it still there?" has to ask about.
    pub fn chunk_hashes(&self) -> Vec<crate::hash::Hash> {
        self.sender.chunks().to_vec()
    }

    /// Stop serving and release the endpoint. Only needed by a caller that never
    /// entered [`Self::serve`] (which ends with the same teardown); dropping the
    /// session instead leaves the accept loop to be torn down by its own guard.
    ///
    /// Bounded, for the reason [`Self::serve`]'s teardown is: `Endpoint::close()`
    /// waits for connected peers to acknowledge, and the moment you most want to
    /// stop serving is precisely while somebody is pulling — so an unbounded wait
    /// here hands a live receiver the power to hold the caller indefinitely. The
    /// close goes on draining detached; only the waiting ends.
    pub async fn shutdown(self) {
        let teardown = tokio::spawn(self.sender.shutdown());
        if tokio::time::timeout(std::time::Duration::from_secs(10), teardown)
            .await
            .is_err()
        {
            tracing::warn!("sender teardown still draining after 10s — not waiting for it");
        }
    }

    /// Park the whole payload on the relay: seed every piece it does not already
    /// hold, so the recipient can finish from the relay with this daemon switched
    /// off. `true` if the relay ended up holding all of it.
    ///
    /// This is what "park it on the relay now" should do for a send whose recipient
    /// is already holding a chunk ticket, instead of a mailbox deposit. The deposit
    /// would upload the same bytes a second time as a sealed one-shot blob — and
    /// worse for the recipient, who cannot resume one: whatever they had already
    /// pulled is discarded and fetched again.
    ///
    /// The caller keeps serving meanwhile; nothing here stops the endpoint, so a
    /// recipient who turns up while this runs still gets it direct.
    pub async fn park_on_relay(
        &self,
        ttl_secs: u64,
        cancel: &CancellationToken,
        on: impl Fn(SendEvent) + Send + Sync,
    ) -> Option<u64> {
        let all: Vec<usize> = (0..self.chunks).collect();
        match self.offload(&all, ttl_secs, cancel, &on).await {
            Offload::Complete { granted } => {
                on(SendEvent::Offloaded);
                // What the relay agreed to, or what we asked for when it had
                // everything already and nothing needed asking.
                Some(granted.unwrap_or(ttl_secs))
            }
            _ => None,
        }
    }

    /// Serve until `cancel` fires. On each receiver drop, backfill the
    /// undelivered tail to the relay (if configured) and keep serving.
    pub async fn serve(
        self,
        cancel: CancellationToken,
        on: impl Fn(SendEvent) + Send + Sync,
    ) -> Result<()> {
        on(SendEvent::Ready {
            chunks: self.chunks,
            total_size: self.total_size,
            ticket: self.ticket.clone(),
            has_relay: self.has_relay,
        });

        // Poll the bytes on the wire and the receiver's chunk-ack count, and
        // surface byte progress on change.
        let chunk_size = self.sender.chunk_size() as u64;
        let mut last_progress = 0u64;
        let mut last_peers = 0usize;
        // Delivery is concluded from the acks, and both the ack count and a later
        // disconnect can prove the same one — so remember *which receivers* have
        // already been reported, and report each of them once.
        //
        // This was a single bool, which is right for one receiver and wrong for a
        // shared ticket in both directions: the second person to take the file was
        // never counted (so `copies_served` stuck at one and a `--share-copies`
        // limit could never be reached), while the ack count it tested was a union
        // across receivers, so two of them taking complementary halves reported a
        // delivery neither had.
        let mut reported: std::collections::HashSet<iroh::EndpointId> =
            std::collections::HashSet::new();
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {
                    let d = self.sender.delivered_count();
                    let transferred = progress_figure(
                        self.sender.sent_bytes(),
                        d,
                        chunk_size,
                        self.total_size,
                        last_progress,
                    );
                    if transferred != last_progress {
                        last_progress = transferred;
                        on(SendEvent::Progress { transferred, total: self.total_size });
                    }
                    // A receiver that has acked every chunk holds the whole file.
                    // This — not the receiver disconnecting — is what "delivered"
                    // means. Waiting for the disconnect instead left a finished send
                    // Active indefinitely whenever the receiver stayed connected (it
                    // keepalives, so even the idle timeout never fires), which in
                    // turn made the manager's delivery loop keep re-offering a file
                    // that had already arrived.
                    for peer in self.sender.completed_peers() {
                        if reported.insert(peer) {
                            on(SendEvent::Delivered);
                        }
                    }
                    let p = self.sender.active_peers();
                    if p != last_peers {
                        last_peers = p;
                        on(SendEvent::Peers { count: p });
                    }
                }
                _ = self.sender.receiver_connected() => {
                    on(SendEvent::ReceiverConnected);
                }
                _ = self.sender.receiver_aborted() => {
                    // They cancelled on purpose. No backfill for a tail nobody
                    // wants (the abort path never reports one). The rule:
                    // one receiver's "no thanks" must not end anyone else's
                    // download — but when EVERYONE has said no (nobody still
                    // pulling, nobody ever completed a copy) the sender stops
                    // too, instead of serving an empty room forever. Sealed to
                    // one recipient, their single no IS everyone's no.
                    //
                    // `active_peers` may briefly still count the aborter (it was
                    // transferring until moments ago, and the activity window
                    // keeps it counted for a few seconds more), hence `<= 1`
                    // rather than `== 0`. Peers that are connected but idle no
                    // longer count at all — which is right here too: they were
                    // never a reason to keep serving.
                    on(SendEvent::RecipientCancelled);
                    if self.sealed_to_recipient
                        || (self.sender.active_peers() <= 1 && reported.is_empty())
                    {
                        break;
                    }
                }
                (peer, undelivered) = self.sender.receiver_gone() => {
                    // Empty tail ⇒ that receiver fetched the whole file. Still the
                    // authoritative signal for an empty (zero-chunk) payload, which
                    // the ack count above can never satisfy.
                    if undelivered.is_empty() {
                        if reported.insert(peer) {
                            on(SendEvent::Delivered);
                        }
                        continue;
                    }
                    if self.relay.is_none() {
                        continue;
                    }
                    on(SendEvent::ReceiverDropped { missing: undelivered.len() });
                    let outcome = self.offload(&undelivered, PARK_TTL_SECS, &cancel, &on).await;
                    // Nothing left for us to do that the relay cannot: the tail is
                    // there in full, and this ticket is sealed to one recipient who
                    // is not here. Serving an empty room until somebody cancels is
                    // what used to pin this task for ever — `give_up_at` and the
                    // wake-on-accept both live in the caller's loop, which cannot
                    // run while we are inside this one.
                    //
                    // Only on a *complete* offload, and only for a 1:1 send with
                    // nobody else pulling. A partial one (capped, refused) leaves us
                    // the only source there is, and a shared ticket has other people
                    // in the room. Same shape as the abort arm above, for the same
                    // reason: one receiver leaving must not end anyone else's
                    // download.
                    if matches!(outcome, Offload::Complete { .. })
                        && self.sealed_to_recipient
                        && self.sender.active_peers() <= 1
                    {
                        on(SendEvent::Offloaded);
                        break;
                    }
                }
            }
        }
        // Teardown, bounded: `Endpoint::close()` waits for connected peers to
        // acknowledge, and a pause happens mid-pull by construction — measured
        // wedging this await for good, and with it the Paused status the user
        // is watching for. The close keeps draining detached; only the *wait*
        // ends, so the caller can report the pause while iroh says its goodbyes.
        let teardown = tokio::spawn(self.sender.shutdown());
        if tokio::time::timeout(std::time::Duration::from_secs(10), teardown)
            .await
            .is_err()
        {
            tracing::warn!("sender teardown still draining after 10s — not waiting for it");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::progress_figure;

    const CS: u64 = 16 * 1024 * 1024;
    const TOTAL: u64 = 3 * CS;

    #[test]
    fn the_wire_count_moves_before_the_first_ack() {
        // The point of the whole thing: with three chunks in flight and none
        // acked yet, progress must not still read zero.
        assert_eq!(progress_figure(5_000_000, 0, CS, TOTAL, 0), 5_000_000);
    }

    #[test]
    fn acks_win_when_they_are_further_along() {
        // A resumed send: the receiver confirms pieces this session never sent.
        assert_eq!(progress_figure(0, 2, CS, TOTAL, 0), 2 * CS);
    }

    #[test]
    fn a_re_sent_piece_cannot_push_it_past_the_payload() {
        // Two receivers, or one that re-fetched a failed piece: more bytes went
        // out than the file holds.
        assert_eq!(progress_figure(TOTAL * 2, 0, CS, TOTAL, 0), TOTAL);
    }

    #[test]
    fn the_figure_never_moves_backwards() {
        assert_eq!(progress_figure(CS, 0, CS, TOTAL, 2 * CS), 2 * CS);
    }

    #[test]
    fn an_empty_payload_stays_at_zero() {
        assert_eq!(progress_figure(0, 0, CS, 0, 0), 0);
    }
}
