//! Transfer flows: the full send/recv orchestration, composed from the crate's
//! primitives ([`crate::chunked`], [`crate::crypto`], [`crate::backfill`]).
//!
//! The CLI and any UI (desktop/browser/mobile) drive transfers through here,
//! reporting progress via a callback and cancelling via a [`CancellationToken`]
//! — so orchestration lives once, in the core, not in each front-end.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use crate::backfill::RelayRelease;
use crate::chunked::{
    ChunkReceiver, ChunkSender, ChunkTicket, KeyDelivery, SeedRequest, CHUNK_SIZE,
};
use crate::crypto::{
    open, open_chunk, random_chunk_key, random_pw_salt, seal, seal_chunk, unwrap_with_password,
    wrap_with_password, Identity, PublicId, Sealed, CHUNK_KEY_LEN,
};
use crate::offline::OfflineTicket;
use crate::transfer::RelayChoice;

/// AAD binding the sealed content key to its purpose (`--to` sends).
const CHUNK_KEY_AAD: &[u8] = b"arvolo/chunk-key/v1";

/// AAD binding the sealed content key to the offline-mailbox purpose (distinct
/// from the P2P [`CHUNK_KEY_AAD`] so a key sealed for one flow can't be replayed
/// into the other).
const MAILBOX_KEY_AAD: &[u8] = b"arvolo/mailbox-key/v1";

/// Where a received chunk was pulled from (the selected primary provider).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkSource {
    Sender,
    Relay,
}

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
    /// Send progress, from the receiver's chunk acks. `transferred` is a byte
    /// estimate (delivered chunks × chunk size, capped at the total).
    Progress { transferred: u64, total: u64 },
    /// A receiver disconnected having fetched **every** chunk — the file reached
    /// it in full. (A ticket may serve several receivers, so this can fire more
    /// than once; a single-recipient sender can stop on the first.)
    Delivered,
    /// The receiver dropped; we're backfilling the undelivered tail to the relay.
    ReceiverDropped { missing: usize },
    /// The undelivered tail is now on the relay; the sender can go offline.
    Backfilled,
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
}

/// Progress events emitted while receiving a file.
#[derive(Debug, Clone)]
pub enum RecvEvent {
    /// Who sent this transfer, emitted before `Started`. `Some(pubkey_bytes)` is
    /// an HPKE-authenticated sender (`--to`); `None` is a `Plain` ticket that
    /// anyone holding it could have produced (anonymous, unauthenticated).
    Sender {
        id: Option<Vec<u8>>,
    },
    Started {
        total: usize,
        resuming_from: usize,
        /// Plaintext size of the whole file (progress-bar length).
        total_size: u64,
        /// Bytes already on disk from a resumed partial (progress-bar start).
        resumed_bytes: u64,
    },
    Control {
        connected: bool,
    },
    Chunk {
        index: usize,
        total: usize,
        source: ChunkSource,
        bytes: u64,
    },
    Saved {
        path: PathBuf,
    },
    Warning {
        message: String,
    },
    /// Swarm progress: how many peers we currently know, and how many pieces we've
    /// pulled from peers (vs. the origin/relay). Emitted while swarming.
    Swarm {
        peers: usize,
        pieces_from_peers: u64,
    },
}

// ---- send -----------------------------------------------------------------

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
    path: &Path,
    name: &str,
    archive: bool,
    to: Option<(&Identity, &PublicId)>,
    seed_relay: Option<String>,
    relay: RelayChoice,
) -> Result<SendSession> {
    anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
    let sender = ChunkSender::serve(path, relay)
        .await
        .context("start sender")?;
    let client = reqwest::Client::new();

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
        sender,
        relay: None,
        client: reqwest::Client::new(),
    })
}

impl SendSession {
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

        // Poll the receiver's chunk-ack count and surface byte progress on change.
        let chunk_size = self.sender.chunk_size() as u64;
        let mut last_delivered = 0usize;
        let mut last_peers = 0usize;
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {
                    let d = self.sender.delivered_count();
                    if d != last_delivered {
                        last_delivered = d;
                        let transferred = (d as u64).saturating_mul(chunk_size).min(self.total_size);
                        on(SendEvent::Progress { transferred, total: self.total_size });
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
                undelivered = self.sender.receiver_gone() => {
                    // Empty tail ⇒ that receiver fetched the whole file.
                    if undelivered.is_empty() {
                        on(SendEvent::Delivered);
                        continue;
                    }
                    let Some(r) = &self.relay else { continue };
                    on(SendEvent::ReceiverDropped { missing: undelivered.len() });
                    let chunks: Vec<_> =
                        undelivered.iter().map(|&i| self.sender.chunks()[i]).collect();
                    let req = SeedRequest {
                        sender: self.sender.addr(),
                        chunks,
                        token: r.token.clone(),
                        // Whole-file id (not just this tail) so the relay meters
                        // the transfer as one durable session across resumes.
                        swarm_id: crate::swarm::swarm_id(
                            self.sender.chunks(),
                            self.total_size,
                        ),
                    };
                    match self
                        .client
                        .post(format!("{}/v1/seed", r.http))
                        .body(req.encode()?)
                        .send()
                        .await
                    {
                        Ok(resp) if resp.status().is_success() => {
                            self.sender.mark_on_relay(&undelivered);
                            on(SendEvent::Backfilled);
                        }
                        Ok(resp) if resp.status() == reqwest::StatusCode::PAYMENT_REQUIRED => {
                            // The relay's per-session offload cap (free shared tier).
                            // Body carries the cap in bytes; fall back to P2P.
                            let limit_bytes = resp
                                .text()
                                .await
                                .ok()
                                .and_then(|b| b.trim().parse().ok())
                                .unwrap_or(0);
                            on(SendEvent::RelayCapped { limit_bytes });
                        }
                        Ok(resp) => on(SendEvent::BackfillFailed {
                            reason: format!("relay rejected: {}", resp.status()),
                        }),
                        Err(e) => on(SendEvent::BackfillFailed { reason: e.to_string() }),
                    }
                }
            }
        }
        self.sender.shutdown().await;
        Ok(())
    }
}

// ---- recv -----------------------------------------------------------------

/// How many chunks to fetch in parallel. Tunable via `ARVOLO_CONCURRENCY`
/// (default 4, clamped to 1..=16).
fn fetch_concurrency() -> usize {
    std::env::var("ARVOLO_CONCURRENCY")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 16)
}

/// The client's scratch directory for temporary artifacts, per `ARVOLO_TEMP_DIR`
/// (the CLI sets it to `<config>/tmp`). Falls back to the system temp dir for a
/// bare library user. Kept off the download directory and off a small tmpfs.
/// Created if missing.
fn client_temp_dir() -> PathBuf {
    let dir = std::env::var("ARVOLO_TEMP_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Where an archive download is staged on disk: `<temp>/arvolo-{hash}.tar` under
/// [`client_temp_dir`]. Deterministic in the ticket's first chunk hash, so a
/// partial resumes and the manager can recompute the same path to keep seeding
/// the tar — never landing in the download directory.
pub(crate) fn archive_stage_path(chunks: &[crate::hash::Hash]) -> PathBuf {
    let hash = chunks.first().map(|h| h.to_string()).unwrap_or_default();
    client_temp_dir().join(format!("arvolo-{hash}.tar"))
}

/// Whether a completed receiver should keep serving the file to the swarm
/// (seed-after-complete). **On by default** — seeding is the norm; set
/// `ARVOLO_SEED=0` (or `false`/`no`) to opt out. For archives it keeps the staged
/// tar on disk. Read here and in the manager alike.
pub(crate) fn seeding_enabled() -> bool {
    !matches!(
        std::env::var("ARVOLO_SEED")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("0") | Some("false") | Some("no") | Some("off")
    )
}

/// Remove any `{download}.arvpart.*` per-chunk staging files.
fn remove_stage_files(download: &Path) {
    let Some(name) = download.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let prefix = format!("{name}.arvpart.");
    let dir = match download.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|f| f.starts_with(&prefix))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Fetch the file described by `ticket` into `out` (default derived from the
/// ticket). Resumes a partial output, prefers P2P, falls back to the relay, and
/// releases relay chunks as they're taken. Returns the output path. If `cancel`
/// fires mid-transfer it returns early with the partial (resumable) output.
/// A supervised control channel to the sender. Unlike a one-shot `open_control`,
/// it keeps the channel connected across churn: on a drop it reconnects with
/// backoff, so the sender's `RelayHas` updates keep flowing into `on_relay` and
/// acks keep reaching the sender. It also publishes the sender's live/offline
/// state so the fetch scheduler can prefer P2P while the sender is up and lean on
/// the relay while it's down — re-evaluated per fetch attempt, not fixed at start.
struct ControlHandle {
    /// True while a control connection to the sender is currently up.
    sender_live: Arc<std::sync::atomic::AtomicBool>,
    /// Best-effort ack of a committed chunk to the sender (dropped while offline).
    ack_tx: tokio::sync::mpsc::UnboundedSender<u32>,
}

fn spawn_control_supervisor(
    receiver: ChunkReceiver,
    sender_addr: iroh::EndpointAddr,
    on_relay: Arc<Mutex<std::collections::HashSet<u32>>>,
    cancel: CancellationToken,
) -> ControlHandle {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    let sender_live = Arc::new(AtomicBool::new(false));
    let (ack_tx, mut ack_rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
    let live = sender_live.clone();
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        // Once the receiver finishes and drops `ack_tx`, stop selecting on it (a
        // closed channel returns immediately) but keep the connection for
        // `RelayHas` updates until cancelled.
        let mut ack_open = true;
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let opened = tokio::select! {
                _ = cancel.cancelled() => break,
                r = tokio::time::timeout(
                    Duration::from_secs(12),
                    receiver.open_control(&sender_addr, on_relay.clone()),
                ) => r,
            };
            let mut control = match opened {
                Ok(Some(c)) => c,
                _ => {
                    live.store(false, Ordering::Relaxed);
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            live.store(true, Ordering::Relaxed);
            backoff = Duration::from_secs(1);
            // A separate clone of the connection so we can await its close without
            // holding a borrow that would block `control.ack`.
            let conn = control.connection();
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        live.store(false, Ordering::Relaxed);
                        return;
                    }
                    _ = conn.closed() => break,
                    maybe = ack_rx.recv(), if ack_open => {
                        match maybe {
                            Some(idx) => {
                                if control.ack(idx).await.is_err() {
                                    break;
                                }
                            }
                            None => ack_open = false,
                        }
                    }
                }
            }
            live.store(false, Ordering::Relaxed);
        }
        live.store(false, Ordering::Relaxed);
    });
    ControlHandle {
        sender_live,
        ack_tx,
    }
}

/// How often a swarm member re-announces to the tracker (well within the relay's
/// `SWARM_PEER_TTL_SECS`) and refreshes its peer list.
const SWARM_ANNOUNCE_SECS: u64 = 20;

/// Whether to join the peer swarm for shared tickets. On by default. Set
/// `ARVOLO_SWARM=off` (or `relay-only`) for the privacy escape hatch: don't
/// announce our address to the tracker and don't seed to peers — fetch only from
/// the origin and the relay, exposing our node to neither other peers. Trades
/// swarm efficiency for not revealing our address to the swarm.
/// How long a completed peer keeps seeding to the swarm, from `ARVOLO_SEED_AFTER`
/// (seconds; 0 or unset = don't linger after completing).
fn seed_after_complete() -> std::time::Duration {
    std::time::Duration::from_secs(
        std::env::var("ARVOLO_SEED_AFTER")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0),
    )
}

fn swarm_enabled() -> bool {
    !matches!(
        std::env::var("ARVOLO_SWARM")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no" | "relay-only" | "relay_only"
    )
}

/// Shared list of known swarm peers: each entry is a peer's serving address and
/// its raw bitfield (which pieces it can serve).
pub(crate) type SwarmPeers = Arc<Mutex<Vec<(iroh::EndpointAddr, Vec<u8>)>>>;

/// Announce a swarm member to the tracker on a timer — publishing its serving
/// address + which pieces it can serve, and learning the other peers (into
/// `peers`). Deregisters on cancel. Used by receivers (partial bitfield) and by
/// senders/seeders (full bitfield) alike.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_swarm_coordinator(
    client: reqwest::Client,
    relay_http: String,
    swarm_id: String,
    my_addr: String,
    have: Arc<std::sync::atomic::AtomicUsize>,
    n_chunks: usize,
    peers: SwarmPeers,
    cancel: CancellationToken,
) {
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    let url = format!(
        "{}/v1/swarm/{}/announce",
        relay_http.trim_end_matches('/'),
        swarm_id
    );
    tokio::spawn(async move {
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let h = have.load(Ordering::Relaxed).min(n_chunks);
            let mut bf = crate::swarm::bitfield_new(n_chunks);
            for i in 0..h {
                crate::swarm::bitfield_set(&mut bf, i);
            }
            let req = crate::swarm::AnnounceReq {
                node_addr: my_addr.clone(),
                bitfield: bf,
                n_chunks: n_chunks as u32,
                event: if h >= n_chunks {
                    "completed"
                } else {
                    "progress"
                }
                .to_string(),
                want: 30,
            };
            if let Ok(resp) = client.post(&url).json(&req).send().await {
                if let Ok(ar) = resp.json::<crate::swarm::AnnounceResp>().await {
                    let decoded: Vec<(iroh::EndpointAddr, Vec<u8>)> = ar
                        .peers
                        .into_iter()
                        .filter_map(|p| {
                            crate::chunked::decode_addr(&p.node_addr)
                                .ok()
                                .map(|a| (a, p.bitfield))
                        })
                        .collect();
                    *peers.lock().unwrap() = decoded;
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(SWARM_ANNOUNCE_SECS)) => {}
            }
        }
        // Best-effort deregister.
        let req = crate::swarm::AnnounceReq {
            node_addr: my_addr,
            bitfield: Vec::new(),
            n_chunks: n_chunks as u32,
            event: "stopped".to_string(),
            want: 0,
        };
        let _ = client.post(&url).json(&req).send().await;
    });
}

/// Endgame: once this few pieces are left in flight, also fetch each from a second
/// (or third) source so one slow provider can't stall the finish.
const ENDGAME_PIECES: usize = 4;
/// Max concurrent sources fetching a single endgame piece.
const ENDGAME_PARALLEL: usize = 3;
/// After a provider fails a fetch, don't reassign to it for this long (avoids
/// spinning on a dead/refusing source while still load-balancing across the rest).
const PROVIDER_COOLDOWN_SECS: u64 = 3;
/// After every source for a piece has failed, wait this long before re-queuing it
/// (so a piece no one can serve yet doesn't busy-loop).
const PIECE_BACKOFF_SECS: u64 = 2;

pub async fn recv_chunked(
    ticket: &str,
    out: Option<PathBuf>,
    identity: Option<&Identity>,
    relay: RelayChoice,
    cancel: CancellationToken,
    on: impl Fn(RecvEvent) + Send + Sync,
) -> Result<PathBuf> {
    use std::collections::{HashMap, HashSet};
    use std::io::{Read, Seek, SeekFrom, Write};
    use tokio::task::JoinSet;

    let t = ChunkTicket::decode(ticket).context("invalid ticket")?;
    let user_out = out;
    // For an archive the payload is unpacked into this directory; the tar itself is
    // staged as a hidden sibling (see `archive_stage_path`).
    let archive_dir: Option<PathBuf> = t
        .archive
        .then(|| user_out.clone().unwrap_or_else(|| PathBuf::from(&t.name)));
    // Where the payload lands on disk: the staged tar for archives (so a partial
    // resumes and — with ARVOLO_SEED — the tar can keep being seeded), else the
    // requested path or a default from the name.
    let download: PathBuf = match &archive_dir {
        Some(_) => archive_stage_path(&t.chunks),
        None => user_out
            .clone()
            .unwrap_or_else(|| default_from_name(&t.name, &t.chunks)),
    };
    // Make sure the staging directory (parent of `download`) exists before we
    // start writing per-chunk `.arvpart` files into it.
    if let Some(parent) = download.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create staging dir {}", parent.display()))?;
        }
    }
    let sender_addr = t.providers.first().cloned();
    let relay_addr = match &t.relay {
        Some(r) => Some(crate::chunked::decode_addr(&r.addr).context("relay address")?),
        None => None,
    };
    // Recover the content key: sealed tickets need our identity and verify the
    // sender; plain tickets carry the key directly. A sealed key that opens is a
    // proof the sender is who the ticket claims, so we surface it only *after*
    // `open` succeeds; a plain ticket has no authenticated sender.
    let (key_bytes, sender_id): (Vec<u8>, Option<Vec<u8>>) = match &t.key {
        KeyDelivery::Plain(k) => (k.clone(), None),
        KeyDelivery::Sealed {
            encapped_key,
            ciphertext,
            sender,
        } => {
            let me = identity.context(
                "this transfer is addressed to a specific recipient; run with your identity",
            )?;
            let sender_pub = PublicId::from_bytes(sender).context("invalid sender in ticket")?;
            let k = open(
                &Sealed {
                    encapped_key: encapped_key.clone(),
                    ciphertext: ciphertext.clone(),
                },
                me,
                &sender_pub,
                CHUNK_KEY_AAD,
            )
            .context("decrypt content key (not the intended recipient, or wrong sender)")?;
            (k, Some(sender.clone()))
        }
    };
    on(RecvEvent::Sender { id: sender_id });
    let key: [u8; crate::crypto::CHUNK_KEY_LEN] = key_bytes
        .as_slice()
        .try_into()
        .context("invalid content key length")?;
    let total_chunks = t.chunks.len() as u32;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&download)
        .with_context(|| format!("open {}", download.display()))?;
    let existing = file.metadata()?.len();
    let start = ((existing / t.chunk_size as u64) as usize).min(t.chunks.len());
    on(RecvEvent::Started {
        total: t.chunks.len(),
        resuming_from: start,
        total_size: t.total_size,
        resumed_bytes: start as u64 * t.chunk_size as u64,
    });

    let receiver = ChunkReceiver::open(relay.clone()).await?;
    let client = reqwest::Client::new();

    // Control channel to the sender. Patience scales with fallback availability:
    // one short attempt if a relay can finish the job, three for pure P2P.
    let on_relay: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));
    // Supervised control channel: keeps `on_relay` fresh and the sender's
    // live/offline state current across churn (reconnecting on drop), rather than
    // a one-shot open whose verdict is frozen for the whole transfer. A child of
    // `cancel` so it stops on cancel; we also cancel it explicitly on completion.
    let ctrl_cancel = cancel.child_token();
    let ctrl = sender_addr.clone().map(|s| {
        spawn_control_supervisor(receiver.clone(), s, on_relay.clone(), ctrl_cancel.clone())
    });
    let sender_live = ctrl.as_ref().map(|h| h.sender_live.clone());
    let ack_tx = ctrl.as_ref().map(|h| h.ack_tx.clone());
    on(RecvEvent::Control {
        connected: sender_addr.is_some(),
    });

    // Swarm (multi-peer) coordination — only for shared `arvc…` tickets: a Plain,
    // ticket-carried key means every holder has the same key and piece hashes, so
    // pieces are shareable. We seed the pieces we've verified (via `ChunkSeeder`)
    // and discover/pull from other peers through the relay tracker. `have` is the
    // count of contiguous committed pieces — both the seeder (what it may serve)
    // and the announce bitfield read it; the commit loop bumps it.
    let have = Arc::new(std::sync::atomic::AtomicUsize::new(start));
    let peers: SwarmPeers = Arc::new(Mutex::new(Vec::new()));
    // Peers (by endpoint id) that served corrupt bytes; filtered out of future
    // provider lists. Populated by `fetch_to_file` on an integrity failure.
    let banned: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    // Metric: pieces we pulled from a swarm peer (vs. the origin/relay).
    let from_peers = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let swarm_cancel = cancel.child_token();
    let mut seeder: Option<crate::chunked::ChunkSeeder> = None;
    let mut swarming = false;
    if swarm_enabled() && matches!(&t.key, KeyDelivery::Plain(_)) {
        if let Some(r) = &t.relay {
            match crate::chunked::ChunkSeeder::start(
                download.clone(),
                key,
                &t.chunks,
                total_chunks,
                have.clone(),
                relay.clone(),
            )
            .await
            {
                Ok(s) => {
                    swarming = true;
                    if let Ok(my_addr) = crate::chunked::encode_addr(&s.addr()) {
                        spawn_swarm_coordinator(
                            client.clone(),
                            r.http.clone(),
                            crate::swarm::swarm_id(&t.chunks, t.total_size),
                            my_addr,
                            have.clone(),
                            total_chunks as usize,
                            peers.clone(),
                            swarm_cancel.clone(),
                        );
                    }
                    seeder = Some(s);
                }
                Err(e) => on(RecvEvent::Warning {
                    message: format!("swarm seeder unavailable: {e:#}"),
                }),
            }
        }
    }

    // Fetch up to `concurrency` chunks in parallel (pipelining hides latency),
    // but commit them to the output **in order** so the file grows contiguously
    // and the length-based resume above stays correct. Each in-flight fetch
    // stages its ciphertext in a per-index `.arvpart.{i}` file (BLAKE3-verified
    // by `fetch_to_file`); the committer then decrypts and positions each chunk.
    // Provider order is re-evaluated live per fetch attempt (see `ordered_providers`).
    let concurrency = fetch_concurrency();
    let total = t.chunks.len();
    let stage_path = |i: usize| PathBuf::from(format!("{}.arvpart.{i}", download.display()));
    let stage_dup =
        |i: usize, n: usize| PathBuf::from(format!("{}.arvpart.{i}.eg{n}", download.display()));
    let sender_id = sender_addr.as_ref().map(|a| a.id.to_string());
    let relay_id = relay_addr.as_ref().map(|a| a.id.to_string());

    // Providers that currently hold piece `i` (endpoint id + addr), minus banned or
    // cooled-down ones: the sender has everything (while connected), the relay
    // whatever it has backfilled (`on_relay`), each peer its bitfield.
    let providers_having =
        |i: usize, cooldown: &HashMap<String, std::time::Instant>, now: std::time::Instant| {
            let banned_snap = banned.lock().unwrap();
            let ok = |id: &str| {
                !banned_snap.contains(id) && cooldown.get(id).map(|t| *t <= now).unwrap_or(true)
            };
            let mut out: Vec<(String, iroh::EndpointAddr)> = Vec::new();
            let sender_up = sender_live
                .as_ref()
                .map(|b| b.load(std::sync::atomic::Ordering::Relaxed))
                .unwrap_or(false);
            if sender_up {
                if let Some(a) = &sender_addr {
                    let id = a.id.to_string();
                    if ok(&id) {
                        out.push((id, a.clone()));
                    }
                }
            }
            if on_relay.lock().unwrap().contains(&(i as u32)) {
                if let Some(a) = &relay_addr {
                    let id = a.id.to_string();
                    if ok(&id) {
                        out.push((id, a.clone()));
                    }
                }
            }
            for (a, bf) in peers.lock().unwrap().iter() {
                if crate::swarm::bitfield_has(bf, i) {
                    let id = a.id.to_string();
                    if ok(&id) {
                        out.push((id, a.clone()));
                    }
                }
            }
            out
        };

    // Task result: (piece, provider id, its stage file, outcome). Outcome is
    // `None` if the fetch was cancelled (its piece was won by another source),
    // else `Some(Ok/Err)`.
    #[allow(clippy::type_complexity)]
    let mut set: JoinSet<(usize, String, PathBuf, Option<Result<()>>)> = JoinSet::new();
    let mut remaining: HashSet<usize> = (start..total).collect(); // not yet started
    let mut next_commit = start;
    let mut satisfied: HashSet<usize> = HashSet::new();
    // A committed-pending piece's stage file + which source it came from.
    let mut winner_stage: HashMap<usize, (PathBuf, ChunkSource)> = HashMap::new();
    let mut in_flight: HashMap<String, usize> = HashMap::new(); // provider id -> outstanding
    let mut piece_srcs: HashMap<usize, HashSet<String>> = HashMap::new(); // piece -> providers fetching it
    let mut piece_cancel: HashMap<usize, CancellationToken> = HashMap::new();
    let mut piece_backoff: HashMap<usize, std::time::Instant> = HashMap::new();
    let mut cooldown: HashMap<String, std::time::Instant> = HashMap::new();

    loop {
        let now = std::time::Instant::now();
        // Assignment: keep the window full. Each fresh chunk goes to the provider
        // that has it with the *fewest requests in flight* (load balancing across
        // peers); once only the last few pieces remain, also give an in-flight piece
        // a second/third source (endgame) so a slow provider can't stall the finish.
        while set.len() < concurrency && !cancel.is_cancelled() {
            // Phase 1: a fresh, rarest (fewest-provider) piece that isn't backed off.
            let fresh = remaining
                .iter()
                .copied()
                .filter(|i| piece_backoff.get(i).map(|t| *t <= now).unwrap_or(true))
                .filter_map(|i| {
                    let p = providers_having(i, &cooldown, now);
                    (!p.is_empty()).then_some((i, p))
                })
                .min_by_key(|(_, p)| p.len());
            let (i, provs, is_fresh) = if let Some((i, p)) = fresh {
                (i, p, true)
            } else {
                // Phase 2: endgame — a not-yet-used source for an in-flight tail piece.
                let live: Vec<usize> = piece_srcs
                    .iter()
                    .filter(|(_, s)| !s.is_empty())
                    .map(|(&i, _)| i)
                    .collect();
                if live.is_empty() || live.len() > ENDGAME_PIECES {
                    break;
                }
                let cand = live.into_iter().find_map(|i| {
                    let used = piece_srcs.get(&i);
                    if used.map(|s| s.len()).unwrap_or(0) >= ENDGAME_PARALLEL {
                        return None;
                    }
                    let p: Vec<_> = providers_having(i, &cooldown, now)
                        .into_iter()
                        .filter(|(id, _)| !used.map(|s| s.contains(id)).unwrap_or(false))
                        .collect();
                    (!p.is_empty()).then_some((i, p))
                });
                match cand {
                    Some((i, p)) => (i, p, false),
                    None => break,
                }
            };
            let (prov_id, prov_addr) = provs
                .into_iter()
                .min_by_key(|(id, _)| *in_flight.get(id).unwrap_or(&0))
                .unwrap();
            if is_fresh {
                remaining.remove(&i);
            }
            *in_flight.entry(prov_id.clone()).or_default() += 1;
            let n = {
                let s = piece_srcs.entry(i).or_default();
                s.insert(prov_id.clone());
                s.len()
            };
            let tok = piece_cancel
                .entry(i)
                .or_insert_with(|| cancel.child_token())
                .clone();
            let stage = if n == 1 {
                stage_path(i)
            } else {
                stage_dup(i, n)
            };
            let rx = receiver.clone();
            let bn = banned.clone();
            let hash = t.chunks[i];
            let stage_task = stage.clone();
            let pid = prov_id.clone();
            set.spawn(async move {
                let r = tokio::select! {
                    _ = tok.cancelled() => None,
                    res = async {
                        let mut part = std::fs::OpenOptions::new()
                            .create(true)
                            .truncate(true)
                            .read(true)
                            .write(true)
                            .open(&stage_task)
                            .with_context(|| format!("open {}", stage_task.display()))?;
                        rx.fetch_one(&prov_addr, hash, &mut part, &bn).await
                    } => Some(res),
                };
                (i, pid, stage, r)
            });
        }

        if set.is_empty() {
            if remaining.is_empty() && piece_srcs.values().all(|s| s.is_empty()) {
                break; // all pieces committed
            }
            // Nothing assignable right now (every source for the remaining pieces is
            // cooled down, or no provider has them yet) — wait briefly, then retry.
            tokio::select! {
                _ = cancel.cancelled() => { receiver.close().await; return Ok(download); }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
            continue;
        }

        let joined = tokio::select! {
            _ = cancel.cancelled() => {
                // Partial stages are left on disk and can be resumed later.
                set.shutdown().await;
                receiver.close().await;
                return Ok(download);
            }
            r = set.join_next() => r,
        };
        let Some(res) = joined else { break };
        let (i, pid, stage, outcome) = res.context("fetch task join")?;
        if let Some(c) = in_flight.get_mut(&pid) {
            *c = c.saturating_sub(1);
        }
        if let Some(s) = piece_srcs.get_mut(&i) {
            s.remove(&pid);
        }
        match outcome {
            // Cancelled duplicate (its piece was won by another source) — discard.
            None => {
                let _ = std::fs::remove_file(&stage);
            }
            Some(Ok(())) => {
                if satisfied.contains(&i) {
                    let _ = std::fs::remove_file(&stage); // duplicate; already have it
                } else {
                    satisfied.insert(i);
                    if let Some(tok) = piece_cancel.get(&i) {
                        tok.cancel(); // stop any still-running duplicate racers
                    }
                    let (label, source) = if sender_id.as_deref() == Some(pid.as_str()) {
                        ("origin", ChunkSource::Sender)
                    } else if relay_id.as_deref() == Some(pid.as_str()) {
                        ("relay", ChunkSource::Relay)
                    } else {
                        from_peers.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        ("peer", ChunkSource::Sender)
                    };
                    tracing::info!(
                        "chunk {} ← {label} {}",
                        i + 1,
                        pid.chars().take(12).collect::<String>()
                    );
                    winner_stage.insert(i, (stage, source));
                }
            }
            Some(Err(e)) => {
                // This provider failed: cool it down briefly, and re-queue the piece
                // if no other source is still fetching it.
                let now = std::time::Instant::now();
                cooldown.insert(
                    pid,
                    now + std::time::Duration::from_secs(PROVIDER_COOLDOWN_SECS),
                );
                let _ = std::fs::remove_file(&stage);
                if !satisfied.contains(&i)
                    && piece_srcs.get(&i).map(|s| s.is_empty()).unwrap_or(true)
                {
                    remaining.insert(i);
                    piece_backoff
                        .insert(i, now + std::time::Duration::from_secs(PIECE_BACKOFF_SECS));
                    tracing::warn!("chunk {} provider failed ({e:#}); will reassign", i + 1);
                }
            }
        }

        // Commit the contiguous prefix, reading each piece from its winning stage.
        while let Some((sp, source)) = winner_stage.remove(&next_commit) {
            let i = next_commit;
            let mut ct = Vec::new();
            std::fs::File::open(&sp)
                .with_context(|| format!("open {}", sp.display()))?
                .read_to_end(&mut ct)?;
            // Ciphertext (providers never see plaintext); already BLAKE3-verified.
            let plain = open_chunk(&key, i as u32, total_chunks, &ct)
                .with_context(|| format!("decrypt chunk {}", i + 1))?;
            file.seek(SeekFrom::Start(i as u64 * t.chunk_size as u64))?;
            file.write_all(&plain)?;
            // Clean up this piece's stage files (winner + any endgame duplicates).
            let _ = std::fs::remove_file(&sp);
            let _ = std::fs::remove_file(stage_path(i));
            for n in 1..=ENDGAME_PARALLEL {
                let _ = std::fs::remove_file(stage_dup(i, n));
            }
            piece_cancel.remove(&i);
            piece_srcs.remove(&i);
            piece_backoff.remove(&i);
            on(RecvEvent::Chunk {
                index: i,
                total,
                source,
                bytes: plain.len() as u64,
            });
            // Ack to the sender (best-effort, via the supervisor — dropped while
            // the control channel is down, resumed on reconnect).
            if let Some(tx) = &ack_tx {
                let _ = tx.send(i as u32);
            }
            // Free relay-backfilled chunks as we take them, UNLESS swarming — then
            // we keep the relay's copy so other peers can still fetch it.
            if !swarming {
                if let Some(r) = &t.relay {
                    let _ = client
                        .post(format!(
                            "{}/v1/release/{}/{}",
                            r.http.trim_end_matches('/'),
                            r.token,
                            t.chunks[i]
                        ))
                        .send()
                        .await;
                }
            }
            next_commit += 1;
            // Publish the newly-committed contiguous prefix so the seeder can serve
            // it and the tracker announce advertises it.
            have.store(next_commit, std::sync::atomic::Ordering::Relaxed);
            // Surface swarm metrics (current peer count + pieces pulled from peers).
            if swarming {
                on(RecvEvent::Swarm {
                    peers: peers.lock().unwrap().len(),
                    pieces_from_peers: from_peers.load(std::sync::atomic::Ordering::Relaxed),
                });
            }
        }
    }
    // Cancelled before completing (e.g. the token was already tripped): leave the
    // partial output as-is for a later resume, without finalizing its size.
    if cancel.is_cancelled() && next_commit < total {
        receiver.close().await;
        return Ok(download);
    }
    // Stop the control supervisor (drops the ack channel and closes the connection).
    ctrl_cancel.cancel();
    drop(ack_tx);
    // Finalize the file so the seeder serves the last piece at its true length.
    file.set_len(t.total_size)?;
    drop(file);
    // Seed-after-complete: a fully-downloaded peer keeps serving the swarm for a
    // while (the coordinator keeps announcing, now as a complete seeder). Opt-in
    // via ARVOLO_SEED_AFTER=<seconds> (0/unset = off) so a finished transfer
    // doesn't linger by default. Stops early on cancel.
    if swarming && !cancel.is_cancelled() {
        let dur = seed_after_complete();
        if !dur.is_zero() {
            on(RecvEvent::Warning {
                message: format!(
                    "download complete — seeding to the swarm for {}s (cancel to stop)",
                    dur.as_secs()
                ),
            });
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = tokio::time::sleep(dur) => {}
            }
        }
    }
    // Stop swarming: deregister from the tracker and shut the seeder down.
    swarm_cancel.cancel();
    if let Some(s) = seeder.take() {
        s.shutdown().await;
    }
    receiver.close().await;
    // Tidy up any leftover per-chunk stage files (e.g. an index committed but its
    // removal was interrupted on a previous run).
    remove_stage_files(&download);

    if let Some(dir) = archive_dir {
        // Unpack the tar into the target directory.
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let f = std::fs::File::open(&download).context("open downloaded archive")?;
        tar::Archive::new(f)
            .unpack(&dir)
            .with_context(|| format!("extract into {}", dir.display()))?;
        // Keep the staged tar iff we'll seed it (its bytes are exactly what the
        // sender sealed, so re-sealing reproduces the ticket's hashes). The manager
        // recomputes this path, seeds it, and deletes it when the session ends.
        // Otherwise drop it now so nothing is left behind.
        if !seeding_enabled() {
            let _ = std::fs::remove_file(&download);
        }
        on(RecvEvent::Saved { path: dir.clone() });
        Ok(dir)
    } else {
        on(RecvEvent::Saved {
            path: download.clone(),
        });
        Ok(download)
    }
}

// ---- offline mailbox ------------------------------------------------------

/// Result of an offline deposit: the ticket to hand the recipient, plus the
/// sender-only **revoke token** — keep it to later cancel the delivery via
/// [`revoke_offline`]. The relay stores only a hash of it and never learns the
/// token unless a revoke is requested.
pub struct Deposited {
    pub ticket: OfflineTicket,
    pub revoke_token: String,
}

/// HTTP header carrying the base32 revoke-hash at deposit / revoke-token at revoke.
const REVOKE_HASH_HEADER: &str = "x-arvolo-revoke-hash";
const REVOKE_TOKEN_HEADER: &str = "x-arvolo-revoke-token";

fn random_token() -> String {
    let bytes: [u8; 16] = rand::random();
    data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
}

/// Encrypt `path` for `recipient` (authenticated as `me`) and deposit the
/// ciphertext on the relay. When `password` is set, the ciphertext is
/// additionally wrapped under a password-derived key (E2E — the relay can never
/// bypass it), and the recipient must supply the same password to
/// [`fetch_offline`]. Returns the ticket plus a sender-only revoke token.
/// Why an offline mailbox deposit couldn't be placed — lets callers react
/// differently: [`TooLarge`](DepositError::TooLarge) will never fit (deliver live
/// P2P instead), [`Unavailable`](DepositError::Unavailable) is transient (retry
/// later), [`Fatal`](DepositError::Fatal) is a local, unrecoverable error.
#[derive(Debug)]
pub enum DepositError {
    /// The relay refused the file as larger than its per-file cap.
    TooLarge,
    /// The relay was unreachable or returned a transient error. Human reason.
    Unavailable(String),
    /// A local, unrecoverable error (couldn't read or seal the file).
    Fatal(anyhow::Error),
}

impl std::fmt::Display for DepositError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DepositError::TooLarge => write!(f, "the relay refused the file as too large"),
            DepositError::Unavailable(m) => write!(f, "relay unavailable: {m}"),
            DepositError::Fatal(e) => write!(f, "{e:#}"),
        }
    }
}
impl std::error::Error for DepositError {}

pub async fn deposit_offline(
    path: &Path,
    recipient: &PublicId,
    me: &Identity,
    relay: &str,
    ttl: u64,
    max: u32,
    password: Option<&str>,
) -> std::result::Result<Deposited, DepositError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use DepositError::Fatal;
    let io = |e: std::io::Error| Fatal(anyhow::Error::from(e));

    if !path.is_file() {
        return Err(Fatal(anyhow::anyhow!("{} is not a file", path.display())));
    }
    let total_size = tokio::fs::metadata(path).await.map_err(io)?.len();
    let total_chunks = if total_size == 0 {
        0
    } else {
        total_size.div_ceil(CHUNK_SIZE as u64) as u32
    };

    // Fresh content key; password-wrap it (small) if requested, then HPKE-seal it
    // to the recipient. The relay blob is then a stream of AES-GCM chunks under the
    // key — so neither side ever holds the whole file in memory.
    let key = random_chunk_key();
    let salt = if password.map(|p| !p.is_empty()).unwrap_or(false) {
        random_pw_salt().to_vec()
    } else {
        Vec::new()
    };
    let key_plain = if salt.is_empty() {
        key.to_vec()
    } else {
        wrap_with_password(password.unwrap(), &salt, &key)
            .context("wrap key with password")
            .map_err(Fatal)?
    };
    let sealed = seal(&key_plain, recipient, me, MAILBOX_KEY_AAD)
        .context("seal content key")
        .map_err(Fatal)?;

    // Seal each 16 MiB chunk into a temp file (bounded memory), then stream that
    // file to the relay.
    let revoke_token = random_token();
    let tmp = std::env::temp_dir().join(format!("arvolo-mb-{revoke_token}.tmp"));
    let seal_res = async {
        let mut infile = tokio::fs::File::open(path).await.map_err(io)?;
        let mut outfile = tokio::fs::File::create(&tmp).await.map_err(io)?;
        let mut buf = vec![0u8; CHUNK_SIZE as usize];
        for idx in 0..total_chunks {
            let want = if idx == total_chunks - 1 {
                (total_size - idx as u64 * CHUNK_SIZE as u64) as usize
            } else {
                CHUNK_SIZE as usize
            };
            infile.read_exact(&mut buf[..want]).await.map_err(io)?;
            let ct = seal_chunk(&key, idx, total_chunks, &buf[..want]).map_err(Fatal)?;
            outfile.write_all(&ct).await.map_err(io)?;
        }
        outfile.flush().await.map_err(io)?;
        Ok::<(), DepositError>(())
    }
    .await;
    if let Err(e) = seal_res {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }

    // Sender-held revoke secret; the relay stores only its BLAKE3 hash.
    let revoke_hash = blake3::hash(revoke_token.as_bytes());
    let relay = relay.trim_end_matches('/').to_string();
    let url = format!("{relay}/v1/deposit?ttl={ttl}&max={max}");
    let upload = match tokio::fs::File::open(&tmp).await {
        Ok(f) => f,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(io(e));
        }
    };
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(upload));
    let result = reqwest::Client::new()
        .post(&url)
        .header(
            "x-arvolo-encapped-key",
            data_encoding::BASE32_NOPAD.encode(&sealed.encapped_key),
        )
        .header(
            REVOKE_HASH_HEADER,
            data_encoding::BASE32_NOPAD.encode(revoke_hash.as_bytes()),
        )
        .body(body)
        .send()
        .await;
    let _ = tokio::fs::remove_file(&tmp).await;

    let resp = result.map_err(|e| DepositError::Unavailable(e.to_string()))?;
    let status = resp.status();
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        return Err(DepositError::TooLarge);
    }
    if !status.is_success() {
        return Err(DepositError::Unavailable(format!(
            "relay returned {status}"
        )));
    }
    let claim = resp
        .text()
        .await
        .map_err(|e| DepositError::Unavailable(e.to_string()))?;

    Ok(Deposited {
        ticket: OfflineTicket {
            relay,
            claim: claim.trim().to_string(),
            sender: me.public().to_bytes(),
            salt,
            wrapped_key: sealed.ciphertext,
            total_size,
        },
        revoke_token,
    })
}

/// Revoke a previously deposited offline blob, deleting it from the relay so it
/// can no longer be fetched. `revoke_token` is the one returned by
/// [`deposit_offline`]. Idempotent: a claim the relay no longer holds is treated
/// as already gone.
pub async fn revoke_offline(relay: &str, claim: &str, revoke_token: &str) -> Result<()> {
    let url = format!("{}/v1/entry/{}", relay.trim_end_matches('/'), claim);
    let resp = reqwest::Client::new()
        .delete(&url)
        .header(REVOKE_TOKEN_HEADER, revoke_token)
        .send()
        .await
        .context("revoke request")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(()); // already gone / expired
    }
    resp.error_for_status()
        .context("relay rejected revoke (wrong token?)")?;
    Ok(())
}

/// Whether a deposited offline blob is still on the relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimStatus {
    /// Still on the relay, not yet fetched.
    Pending,
    /// No longer on the relay — fetched (burn-after-read) or expired. Within a
    /// short poll window (far below the blob TTL) this means it was fetched.
    Gone,
}

/// Live status of a deposited blob on the relay, with download accounting when
/// the relay reports it. `downloads`/`max_downloads` are `None` against an older
/// relay that only signals presence.
#[derive(Debug, Clone, Copy)]
pub struct ClaimInfo {
    pub present: bool,
    pub downloads: Option<u32>,
    pub max_downloads: Option<u32>,
}

#[derive(serde::Deserialize)]
struct ClaimStatusBody {
    downloads: Option<u32>,
    max_downloads: Option<u32>,
}

/// Query a deposited blob's status **and** how many times it's been fetched.
/// Newer relays return the counts as JSON; against an older relay the counts are
/// `None` but presence still resolves.
pub async fn claim_info(relay: &str, claim: &str) -> Result<ClaimInfo> {
    let url = format!("{}/v1/entry/{}/status", relay.trim_end_matches('/'), claim);
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .context("claim status request")?;
    if resp.status().is_success() {
        let (downloads, max_downloads) = match resp.json::<ClaimStatusBody>().await {
            Ok(b) => (b.downloads, b.max_downloads),
            Err(_) => (None, None), // older relay: plain-text body, presence only
        };
        Ok(ClaimInfo {
            present: true,
            downloads,
            max_downloads,
        })
    } else if matches!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE
    ) {
        Ok(ClaimInfo {
            present: false,
            downloads: None,
            max_downloads: None,
        })
    } else {
        anyhow::bail!("relay rejected claim status: {}", resp.status())
    }
}

/// Query whether a deposited blob (`claim`) is still on the relay. Lets a sender
/// confirm an offline delivery (poll until [`ClaimStatus::Gone`]).
pub async fn claim_status(relay: &str, claim: &str) -> Result<ClaimStatus> {
    Ok(if claim_info(relay, claim).await?.present {
        ClaimStatus::Pending
    } else {
        ClaimStatus::Gone
    })
}

/// Fetch and decrypt an offline ticket into `out` (default derived from the
/// claim). Returns the output path and the number of plaintext bytes written.
pub async fn fetch_offline(
    ticket: &str,
    out: Option<PathBuf>,
    me: &Identity,
    password: Option<&str>,
) -> Result<(PathBuf, usize)> {
    use tokio::io::AsyncWriteExt;
    let t = OfflineTicket::decode(ticket)?;
    anyhow::ensure!(
        !t.wrapped_key.is_empty(),
        "unsupported offline ticket (older whole-file format is no longer accepted)"
    );
    let sender = PublicId::from_bytes(&t.sender).context("invalid sender in ticket")?;
    if t.has_password() && password.map(|p| p.is_empty()).unwrap_or(true) {
        anyhow::bail!("this link is password-protected — supply the password");
    }

    let url = format!("{}/v1/fetch/{}", t.relay.trim_end_matches('/'), t.claim);
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .context("fetch request")?
        .error_for_status()
        .context("relay rejected fetch (expired or already claimed?)")?;

    let encapped = resp
        .headers()
        .get("x-arvolo-encapped-key")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            data_encoding::BASE32_NOPAD
                .decode(s.to_uppercase().as_bytes())
                .ok()
        })
        .context("missing encapped key from relay")?;

    // Recover the content key: HPKE-open (verifies the sender), then peel the
    // optional password layer. Small — the file itself never passes through here.
    let key_plain = open(
        &Sealed {
            encapped_key: encapped,
            ciphertext: t.wrapped_key.clone(),
        },
        me,
        &sender,
        MAILBOX_KEY_AAD,
    )
    .context("decrypt content key (wrong identity, sender, or tampered)")?;
    let key_bytes = if t.has_password() {
        let pw = password.expect("password presence checked above");
        unwrap_with_password(pw, &t.salt, &key_plain).context("unwrap key with password")?
    } else {
        key_plain
    };
    let key: [u8; CHUNK_KEY_LEN] = key_bytes
        .as_slice()
        .try_into()
        .context("invalid content key length")?;

    let total_size = t.total_size;
    let total_chunks = if total_size == 0 {
        0
    } else {
        total_size.div_ceil(CHUNK_SIZE as u64) as u32
    };

    // Stream the ciphertext chunk stream straight to disk, decrypting a 16 MiB
    // chunk at a time — peak memory is ~one chunk, never the whole file. `carry`
    // reassembles exactly one sealed chunk from arbitrary HTTP frame boundaries.
    let out = out.unwrap_or_else(|| default_out(&t.claim));
    let mut outfile = tokio::fs::File::create(&out)
        .await
        .with_context(|| format!("create {}", out.display()))?;
    let mut resp = resp;
    let mut carry: Vec<u8> = Vec::new();
    let mut eof = false;
    for idx in 0..total_chunks {
        let plain_len = if idx == total_chunks - 1 {
            total_size - idx as u64 * CHUNK_SIZE as u64
        } else {
            CHUNK_SIZE as u64
        };
        let ct_len = plain_len as usize + crate::crypto::CHUNK_TAG_LEN;
        while carry.len() < ct_len && !eof {
            match resp.chunk().await.context("read ciphertext")? {
                Some(b) => carry.extend_from_slice(&b),
                None => eof = true,
            }
        }
        anyhow::ensure!(
            carry.len() >= ct_len,
            "truncated mailbox blob at chunk {idx}"
        );
        let ct: Vec<u8> = carry.drain(..ct_len).collect();
        let plain = open_chunk(&key, idx, total_chunks, &ct).context("decrypt chunk")?;
        outfile.write_all(&plain).await.context("write chunk")?;
    }
    outfile.flush().await.context("flush output")?;
    Ok((out, total_size as usize))
}

/// A stable default output filename derived from a ticket seed.
pub fn default_out(seed: &str) -> PathBuf {
    PathBuf::from(format!("received-{}.bin", &seed[..seed.len().min(16)]))
}

/// Pack files and/or directories into a tar archive at `dest` (each top-level
/// input keeps its base name inside the archive). Used to send folders/multiple
/// files as one transfer; the receiver unpacks it (see [`recv_chunked`]).
/// Pack `paths` into a tar at `dest` *deterministically*: entries are emitted in
/// a stable sorted order with normalized metadata (mtime/uid/gid zeroed, fixed
/// mode), so the same inputs always yield byte-identical output. That is what
/// lets an interrupted archive send be resumed — repacking reproduces the exact
/// chunk hashes the original ticket promised (verified on resume). Symlinks are
/// followed to their target; broken or special files are skipped.
pub fn pack_tar(paths: &[PathBuf], dest: &Path) -> Result<()> {
    // Gather every regular file (name-in-archive → source) plus the directory
    // entries (so empty dirs survive), then sort for a stable layout.
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    for p in paths {
        let base = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        if p.is_dir() {
            collect_dir(p, &base, &mut files, &mut dirs)?;
        } else {
            files.push((base, p.clone()));
        }
    }
    files.sort();
    dirs.sort();
    dirs.dedup();

    let out = std::fs::File::create(dest)
        .with_context(|| format!("create archive {}", dest.display()))?;
    let mut builder = tar::Builder::new(out);
    builder.mode(tar::HeaderMode::Deterministic);

    for d in &dirs {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        builder
            .append_data(&mut header, format!("{d}/"), std::io::empty())
            .with_context(|| format!("archive dir {d}"))?;
    }
    for (name, src) in &files {
        let data = std::fs::File::open(src).with_context(|| format!("open {}", src.display()))?;
        let len = data
            .metadata()
            .with_context(|| format!("stat {}", src.display()))?
            .len();
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(len);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        builder
            .append_data(&mut header, name, data)
            .with_context(|| format!("archive file {name}"))?;
    }
    builder.finish().context("finish archive")?;
    Ok(())
}

/// Recursively collect a directory's regular files and subdirectories in sorted
/// order (following symlinks to their targets), for deterministic packing.
fn collect_dir(
    dir: &Path,
    prefix: &str,
    files: &mut Vec<(String, PathBuf)>,
    dirs: &mut Vec<String>,
) -> Result<()> {
    dirs.push(prefix.to_string());
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("read dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let child = format!("{prefix}/{}", e.file_name().to_string_lossy());
        // `metadata` (not `symlink_metadata`) follows symlinks; skip anything
        // that isn't a plain file or directory (broken links, sockets, …).
        let Ok(md) = std::fs::metadata(&path) else {
            continue;
        };
        if md.is_dir() {
            collect_dir(&path, &child, files, dirs)?;
        } else if md.is_file() {
            files.push((child, path));
        }
    }
    Ok(())
}

/// Default single-file output: the ticket's suggested name (its final path
/// component, to avoid traversal), falling back to a seed-derived name.
fn default_from_name(name: &str, chunks: &[crate::reexport::Hash]) -> PathBuf {
    let base = std::path::Path::new(name)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty() && s != "." && s != "..");
    match base {
        Some(n) => PathBuf::from(n),
        None => default_out(&chunks.first().map(|h| h.to_string()).unwrap_or_default()),
    }
}
