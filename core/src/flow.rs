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
use crate::chunked::{ChunkReceiver, ChunkSender, ChunkTicket, KeyDelivery, SeedRequest};
use crate::crypto::{
    open, open_chunk, random_pw_salt, seal, unwrap_with_password, wrap_with_password, Identity,
    PublicId, Sealed,
};
use crate::offline::OfflineTicket;
use crate::transfer::RelayChoice;

/// AAD binding the sealed content key to its purpose (`--to` sends).
const CHUNK_KEY_AAD: &[u8] = b"arvolo/chunk-key/v1";

/// Per-chunk fetch resilience: when a chunk is transiently unavailable (a dropped
/// direct P2P connection, or a relay whose backfill hasn't reached it yet), keep
/// retrying the same providers with exponential backoff, from `START` up to a `CAP`
/// interval, *indefinitely* — the transfer never fails on its own, it just waits
/// and resumes whenever the sender/relay has the chunk again (torrent-style). Only
/// a user cancel stops it.
const CHUNK_RETRY_BACKOFF_START_SECS: u64 = 2;
const CHUNK_RETRY_BACKOFF_CAP_SECS: u64 = 5 * 60;

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

    let mut relay = None;
    if let Some(url) = seed_relay {
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
        relay = Some(RelayRelease {
            http: url,
            addr,
            token,
        });
    }

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
        relay: None,
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

/// Ordered providers for chunk `i`, re-evaluated from *live* state (so it adapts
/// as the sender reconnects or the relay's backfill lands): sender-first (P2P)
/// while the sender is connected and the chunk isn't already on the relay;
/// relay-first when the chunk is on the relay or the sender is offline. Returns
/// `(relay_first, providers)`. `providers` always lists both sources (when
/// present) so the fetch falls back to the other one.
fn ordered_providers(
    i: usize,
    sender_addr: &Option<iroh::EndpointAddr>,
    relay_addr: &Option<iroh::EndpointAddr>,
    on_relay: &Mutex<std::collections::HashSet<u32>>,
    sender_live: &Option<Arc<std::sync::atomic::AtomicBool>>,
    peers: &[(iroh::EndpointAddr, Vec<u8>)],
) -> (bool, Vec<iroh::EndpointAddr>) {
    use std::sync::atomic::Ordering;
    let sender_up = sender_addr.is_some()
        && sender_live
            .as_ref()
            .map(|b| b.load(Ordering::Relaxed))
            .unwrap_or(false);
    let relay_first = !sender_up || on_relay.lock().unwrap().contains(&(i as u32));
    let mut providers = Vec::new();
    if relay_first {
        relay_addr.iter().for_each(|a| providers.push(a.clone()));
        sender_addr.iter().for_each(|a| providers.push(a.clone()));
    } else {
        sender_addr.iter().for_each(|a| providers.push(a.clone()));
        relay_addr.iter().for_each(|a| providers.push(a.clone()));
    }
    // Swarm peers that advertise this piece, as extra fallback sources.
    for (addr, bf) in peers {
        if crate::swarm::bitfield_has(bf, i) {
            providers.push(addr.clone());
        }
    }
    (relay_first, providers)
}

/// How often a swarm member re-announces to the tracker (well within the relay's
/// `SWARM_PEER_TTL_SECS`) and refreshes its peer list.
const SWARM_ANNOUNCE_SECS: u64 = 20;

/// Whether to join the peer swarm for shared tickets. On by default. Set
/// `ARVOLO_SWARM=off` (or `relay-only`) for the privacy escape hatch: don't
/// announce our address to the tracker and don't seed to peers — fetch only from
/// the origin and the relay, exposing our node to neither other peers. Trades
/// swarm efficiency for not revealing our address to the swarm.
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

/// Rarest-first piece selection: from `remaining`, pick (and remove) the piece
/// held by the fewest sources right now — the origin (if connected) counts for
/// every piece, the relay for pieces it has backfilled, and each peer for the
/// pieces in its bitfield. Ties are broken randomly so concurrent peers don't all
/// grab the same piece. This automatically drains scarce pieces first: a piece
/// only the origin has (availability 1) outranks one that's also on the relay or a
/// peer — so the at-risk source is emptied before it can drop. A piece with zero
/// known sources still gets picked (and then waits/retries) rather than stalling.
fn pick_rarest(
    remaining: &mut Vec<usize>,
    sender_up: bool,
    on_relay: &std::collections::HashSet<u32>,
    peers: &[(iroh::EndpointAddr, Vec<u8>)],
) -> usize {
    let avail = |i: usize| -> usize {
        let mut a = usize::from(sender_up);
        if on_relay.contains(&(i as u32)) {
            a += 1;
        }
        a + peers
            .iter()
            .filter(|(_, bf)| crate::swarm::bitfield_has(bf, i))
            .count()
    };
    let best = remaining.iter().map(|&i| avail(i)).min().unwrap_or(0);
    let candidates: Vec<usize> = remaining
        .iter()
        .enumerate()
        .filter(|(_, &i)| avail(i) == best)
        .map(|(pos, _)| pos)
        .collect();
    use rand::Rng;
    let pos = candidates[rand::rng().random_range(0..candidates.len())];
    remaining.swap_remove(pos)
}

/// Announce this receiver to the swarm tracker on a timer — publishing our seeder
/// address + which pieces we can serve, and learning the other peers (into
/// `peers`, used by [`ordered_providers`]). Deregisters on cancel.
#[allow(clippy::too_many_arguments)]
fn spawn_swarm_coordinator(
    client: reqwest::Client,
    relay_http: String,
    swarm_id: String,
    my_addr: String,
    have: Arc<std::sync::atomic::AtomicUsize>,
    n_chunks: usize,
    peers: Arc<Mutex<Vec<(iroh::EndpointAddr, Vec<u8>)>>>,
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
                event: if h >= n_chunks { "completed" } else { "progress" }.to_string(),
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
    // Where the payload lands on disk: a stable temp tar for archives (so a
    // partial resumes), else the requested path or a default from the name.
    let download: PathBuf = if t.archive {
        std::env::temp_dir().join(format!(
            "arvolo-{}.tar",
            t.chunks.first().map(|h| h.to_string()).unwrap_or_default()
        ))
    } else {
        user_out
            .clone()
            .unwrap_or_else(|| default_from_name(&t.name, &t.chunks))
    };
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
    let peers: Arc<Mutex<Vec<(iroh::EndpointAddr, Vec<u8>)>>> = Arc::new(Mutex::new(Vec::new()));
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

    let mut set: JoinSet<Result<usize>> = JoinSet::new();
    // Pieces still to fetch. We pick the rarest of these each time we fill the
    // window (rather than scanning in order), so scarce sources are drained first.
    let mut remaining: Vec<usize> = (start..total).collect();
    let mut next_commit = start;
    let mut ready: HashSet<usize> = HashSet::new();
    let mut sources: HashMap<usize, ChunkSource> = HashMap::new();

    loop {
        // Refill the in-flight window, rarest-piece-first.
        while !remaining.is_empty() && set.len() < concurrency && !cancel.is_cancelled() {
            let sender_up = sender_addr.is_some()
                && sender_live
                    .as_ref()
                    .map(|b| b.load(std::sync::atomic::Ordering::Relaxed))
                    .unwrap_or(false);
            let i = {
                let orl = on_relay.lock().unwrap();
                let pl = peers.lock().unwrap();
                pick_rarest(&mut remaining, sender_up, &orl, &pl)
            };
            let (relay_first, _) =
                ordered_providers(i, &sender_addr, &relay_addr, &on_relay, &sender_live, &[]);
            sources.insert(
                i,
                if relay_first {
                    ChunkSource::Relay
                } else {
                    ChunkSource::Sender
                },
            );
            let rx = receiver.clone();
            let hash = t.chunks[i];
            let sp = stage_path(i);
            let ct = cancel.clone();
            // Clones for the live provider re-evaluation inside the retry loop.
            let sa = sender_addr.clone();
            let ra = relay_addr.clone();
            let orl = on_relay.clone();
            let sl = sender_live.clone();
            let pl = peers.clone();
            set.spawn(async move {
                use std::time::Duration;
                let mut part = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(&sp)
                    .with_context(|| format!("open {}", sp.display()))?;
                // Resilient fetch, BitTorrent-style: a mid-transfer P2P drop or a
                // relay whose backfill hasn't reached a chunk yet makes it
                // transiently unavailable. Rather than failing, keep retrying with
                // capped exponential backoff — the sender (as its direct connection
                // re-establishes) and the relay (as its backfill lands) —
                // *indefinitely*, until the chunk arrives or the user cancels. So a
                // transfer whose sender went away simply waits and resumes whenever
                // the sender comes back; it never gives up on its own. The provider
                // order is re-evaluated each attempt, so a chunk that becomes
                // available on the relay, or a sender that reconnects, is used as
                // soon as it appears. Partial bytes persist in the stage file, so
                // each retry resumes mid-chunk.
                let mut backoff = Duration::from_secs(CHUNK_RETRY_BACKOFF_START_SECS);
                loop {
                    let peer_snapshot = pl.lock().unwrap().clone();
                    let (_, providers) =
                        ordered_providers(i, &sa, &ra, &orl, &sl, &peer_snapshot);
                    match rx.fetch_to_file(&providers, hash, &mut part).await {
                        Ok(()) => break,
                        Err(e) => {
                            if ct.is_cancelled() {
                                return Err(e).with_context(|| format!("fetch chunk {}", i + 1));
                            }
                            tracing::warn!(
                                "chunk {} unavailable ({e:#}); retrying in {}s \
                                 (will keep retrying until it's available or you cancel)",
                                i + 1,
                                backoff.as_secs()
                            );
                            tokio::select! {
                                _ = ct.cancelled() => {
                                    return Err(e).with_context(|| format!("fetch chunk {}", i + 1));
                                }
                                _ = tokio::time::sleep(backoff) => {}
                            }
                            backoff = (backoff * 2).min(Duration::from_secs(CHUNK_RETRY_BACKOFF_CAP_SECS));
                        }
                    }
                }
                Ok::<usize, anyhow::Error>(i)
            });
        }

        if set.is_empty() {
            break;
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
        let done = res.context("fetch task failed")??;
        ready.insert(done);

        // Commit every chunk whose turn has come, ascending, so writes stay
        // contiguous (positioned at `i * chunk_size`).
        while ready.remove(&next_commit) {
            let i = next_commit;
            let source = sources[&i];
            let sp = stage_path(i);
            let mut ct = Vec::new();
            std::fs::File::open(&sp)
                .with_context(|| format!("open {}", sp.display()))?
                .read_to_end(&mut ct)?;
            // Ciphertext (providers never see plaintext); already BLAKE3-verified.
            let plain = open_chunk(&key, i as u32, total_chunks, &ct)
                .with_context(|| format!("decrypt chunk {}", i + 1))?;
            file.seek(SeekFrom::Start(i as u64 * t.chunk_size as u64))?;
            file.write_all(&plain)?;
            let _ = std::fs::remove_file(&sp);
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
            // Free relay-backfilled chunks as we take them. Attempt release for
            // every chunk: with the sender offline there's no control channel to
            // learn `on_relay`, yet those are the chunks the relay holds. The
            // relay's (token, hash) guard makes it a no-op for anything not seeded.
            // BUT in a swarm we deliberately keep the relay's copy so other peers
            // can still fetch it — releasing would strand pieces if we go offline.
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
    // Stop swarming: deregister from the tracker and shut the seeder down. (A
    // completed peer stops seeding when the transfer ends; seed-after-complete is a
    // later enhancement.)
    swarm_cancel.cancel();
    if let Some(s) = seeder.take() {
        s.shutdown().await;
    }
    file.set_len(t.total_size)?;
    drop(file);
    receiver.close().await;
    // Tidy up any leftover per-chunk stage files (e.g. an index committed but its
    // removal was interrupted on a previous run).
    remove_stage_files(&download);

    if t.archive {
        // Unpack the tar into the target directory, then drop the temp archive.
        let dir = user_out.unwrap_or_else(|| PathBuf::from(&t.name));
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let f = std::fs::File::open(&download).context("open downloaded archive")?;
        tar::Archive::new(f)
            .unpack(&dir)
            .with_context(|| format!("extract into {}", dir.display()))?;
        let _ = std::fs::remove_file(&download);
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

/// Default cap on a body downloaded from the (untrusted) relay in
/// [`fetch_offline`], so a hostile relay can't OOM the client. Override with
/// `ARVOLO_MAX_FETCH_BYTES`.
const DEFAULT_MAX_FETCH_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB

fn max_fetch_bytes() -> u64 {
    std::env::var("ARVOLO_MAX_FETCH_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_FETCH_BYTES)
}

/// Encrypt `path` for `recipient` (authenticated as `me`) and deposit the
/// ciphertext on the relay. When `password` is set, the ciphertext is
/// additionally wrapped under a password-derived key (E2E — the relay can never
/// bypass it), and the recipient must supply the same password to
/// [`fetch_offline`]. Returns the ticket plus a sender-only revoke token.
pub async fn deposit_offline(
    path: &Path,
    recipient: &PublicId,
    me: &Identity,
    relay: &str,
    ttl: u64,
    max: u32,
    password: Option<&str>,
) -> Result<Deposited> {
    anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
    let plaintext = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let sealed = seal(&plaintext, recipient, me, b"").context("encrypt")?;

    // Optional outer password-wrap layer over the HPKE ciphertext.
    let (body, salt) = match password {
        Some(pw) if !pw.is_empty() => {
            let salt = random_pw_salt();
            let wrapped =
                wrap_with_password(pw, &salt, &sealed.ciphertext).context("wrap with password")?;
            (wrapped, salt.to_vec())
        }
        _ => (sealed.ciphertext, Vec::new()),
    };

    // Sender-held revoke secret; the relay stores only its BLAKE3 hash.
    let revoke_token = random_token();
    let revoke_hash = blake3::hash(revoke_token.as_bytes());

    let relay = relay.trim_end_matches('/').to_string();
    let url = format!("{relay}/v1/deposit?ttl={ttl}&max={max}");
    let claim = reqwest::Client::new()
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
        .await
        .context("deposit request")?
        .error_for_status()
        .context("relay rejected deposit")?
        .text()
        .await
        .context("read claim")?;

    Ok(Deposited {
        ticket: OfflineTicket {
            relay,
            claim: claim.trim().to_string(),
            sender: me.public().to_bytes(),
            salt,
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

/// Query whether a deposited blob (`claim`) is still on the relay. Lets a sender
/// confirm an offline delivery (poll until [`ClaimStatus::Gone`]).
pub async fn claim_status(relay: &str, claim: &str) -> Result<ClaimStatus> {
    let url = format!("{}/v1/entry/{}/status", relay.trim_end_matches('/'), claim);
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .context("claim status request")?;
    if resp.status().is_success() {
        Ok(ClaimStatus::Pending)
    } else if matches!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE
    ) {
        Ok(ClaimStatus::Gone)
    } else {
        anyhow::bail!("relay rejected claim status: {}", resp.status())
    }
}

/// Fetch and decrypt an offline ticket into `out` (default derived from the
/// claim). Returns the output path and the number of plaintext bytes written.
pub async fn fetch_offline(
    ticket: &str,
    out: Option<PathBuf>,
    me: &Identity,
    password: Option<&str>,
) -> Result<(PathBuf, usize)> {
    let t = OfflineTicket::decode(ticket)?;
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

    // The relay is untrusted: cap the downloaded body so a hostile/buggy relay
    // can't stream an unbounded response and OOM us. Reject early on a declared
    // length, and enforce again while streaming (the header can lie).
    let cap = max_fetch_bytes();
    if let Some(len) = resp.content_length() {
        anyhow::ensure!(len <= cap, "relay response too large ({len} > {cap} bytes)");
    }
    let mut resp = resp;
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.context("read ciphertext")? {
        anyhow::ensure!(
            body.len() as u64 + chunk.len() as u64 <= cap,
            "relay response exceeded {cap}-byte cap"
        );
        body.extend_from_slice(&chunk);
    }

    // Peel the optional password-wrap layer before HPKE-opening.
    let ciphertext = if t.has_password() {
        let pw = password.expect("password presence checked above");
        unwrap_with_password(pw, &t.salt, &body).context("unwrap with password")?
    } else {
        body
    };

    let plaintext = open(
        &Sealed {
            encapped_key: encapped,
            ciphertext,
        },
        me,
        &sender,
        b"",
    )
    .context("decrypt (wrong identity, sender, or tampered)")?;

    let out = out.unwrap_or_else(|| default_out(&t.claim));
    std::fs::write(&out, &plaintext).with_context(|| format!("write {}", out.display()))?;
    Ok((out, plaintext.len()))
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
