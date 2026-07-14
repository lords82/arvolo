use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use crate::chunked::{ChunkReceiver, ChunkTicket, KeyDelivery};
use crate::crypto::{open, open_chunk, Identity, PublicId, Sealed};
use crate::transfer::RelayChoice;

use super::archive::unpack_archive_safely;
use super::ctrl::{spawn_control_supervisor, ControlHandle};
use super::sidecar::{read_sidecar, remove_stage_files, sidecar_path, write_sidecar};
use super::storage::{available_space, disk_full_reason, is_local_storage_error};
use super::CHUNK_KEY_AAD;

/// Where a received chunk was pulled from (the selected primary provider).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkSource {
    Sender,
    Relay,
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
    /// The download stopped before completing on an unrecoverable *local*
    /// condition (e.g. the disk is full) — not a provider/network fault. The
    /// partial output + resume sidecar are left on disk; re-run to resume once
    /// the condition is cleared. `reason` is a human-facing explanation.
    Paused {
        reason: String,
    },
    /// Swarm progress: how many peers we currently know, and how many pieces we've
    /// pulled from peers (vs. the origin/relay). Emitted while swarming.
    Swarm {
        peers: usize,
        pieces_from_peers: u64,
    },
}

/// How a [`recv_chunked`] call ended. Distinguishes a finished download from one
/// that stopped early but is *resumable* (its partial output + sidecar remain on
/// disk), so a caller doesn't mistake a paused/cancelled transfer for a complete
/// one (and, e.g., seed a partial file).
pub enum RecvOutcome {
    /// Fully downloaded, verified, and finalized at this path (the unpacked
    /// directory for an archive, else the output file).
    Completed(PathBuf),
    /// Stopped before completing because the caller's cancellation token fired.
    /// The partial output + resume sidecar remain on disk — re-run to resume.
    Cancelled(PathBuf),
    /// Stopped before completing on an unrecoverable *local* condition (e.g. the
    /// disk is full). Not a provider/network fault and not a user cancel: the
    /// partial output + sidecar remain, so re-running resumes once it's cleared.
    Paused { output: PathBuf, reason: String },
}

impl RecvOutcome {
    /// The output path in every outcome (finalized, or the resumable partial).
    pub fn path(&self) -> &Path {
        match self {
            RecvOutcome::Completed(p) | RecvOutcome::Cancelled(p) => p,
            RecvOutcome::Paused { output, .. } => output,
        }
    }
    /// Consumes the outcome, yielding its output path.
    pub fn into_path(self) -> PathBuf {
        match self {
            RecvOutcome::Completed(p) | RecvOutcome::Cancelled(p) => p,
            RecvOutcome::Paused { output, .. } => output,
        }
    }
    /// True only for a fully-completed download (not cancelled, not paused).
    pub fn is_complete(&self) -> bool {
        matches!(self, RecvOutcome::Completed(_))
    }
}

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
    have: crate::chunked::HaveBitfield,
    n_chunks: usize,
    peers: SwarmPeers,
    cancel: CancellationToken,
) {
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
            // Announce the *actual* bitfield of verified pieces (possibly disjoint),
            // so peers know exactly which pieces we can serve.
            let bf = have.lock().unwrap().clone();
            let count = crate::swarm::bitfield_count(&bf) as usize;
            let req = crate::swarm::AnnounceReq {
                node_addr: my_addr.clone(),
                bitfield: bf,
                n_chunks: n_chunks as u32,
                event: if count >= n_chunks {
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
) -> Result<RecvOutcome> {
    use std::collections::{HashMap, HashSet};
    use std::io::{Read, Seek, SeekFrom, Write};
    use tokio::task::JoinSet;

    let t = ChunkTicket::decode(ticket).context("invalid ticket")?;
    let user_out = out;
    // For an archive the payload is unpacked into this directory; the tar itself is
    // staged as a hidden sibling (see `archive_stage_path`). The ticket `name` is
    // attacker-authored, so the default unpack dir must be reduced to a single safe
    // component (`safe_download_name`) — otherwise an `arvc` ticket with an absolute
    // or `..` name would let `unpack_in` write entries outside the download dir even
    // though each entry itself is a normal component.
    let archive_dir: Option<PathBuf> = t.archive.then(|| {
        user_out.clone().unwrap_or_else(|| {
            PathBuf::from(
                safe_download_name(&t.name)
                    .unwrap_or_else(|| default_out(&t.name).display().to_string()),
            )
        })
    });
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
    // Resume from the sidecar bitfield of already-verified pieces. This tolerates
    // the sparse, out-of-order output a piece-swarm produces (a length-based resume
    // cannot). An absent/mismatched sidecar starts fresh — any bytes already on disk
    // are overwritten as pieces commit.
    let have_bits = read_sidecar(&download, t.chunks.len());
    let resuming_from = crate::swarm::bitfield_count(&have_bits) as usize;

    // Pre-flight: if the filesystem plainly lacks room for what's left to fetch,
    // pause up front instead of downloading gigabytes only to fill the disk
    // partway. Best-effort — an unknown free-space figure (non-unix / syscall
    // failure) never blocks the transfer, and a real mid-transfer ENOSPC is still
    // caught and paused. `resuming_from * chunk_size` over-counts committed bytes
    // (the last piece may be short), which only makes this *less* eager to pause.
    let remaining_bytes = t
        .total_size
        .saturating_sub(resuming_from as u64 * t.chunk_size as u64);
    if let Some(avail) = available_space(&download) {
        // One chunk of headroom covers the transient per-piece staging file.
        if avail < remaining_bytes.saturating_add(t.chunk_size as u64) {
            let reason = disk_full_reason(&download);
            on(RecvEvent::Paused {
                reason: reason.clone(),
            });
            return Ok(RecvOutcome::Paused {
                output: download,
                reason,
            });
        }
    }

    on(RecvEvent::Started {
        total: t.chunks.len(),
        resuming_from,
        total_size: t.total_size,
        resumed_bytes: resuming_from as u64 * t.chunk_size as u64,
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

    // Swarm (multi-peer) coordination. A piece is addressed by BLAKE3 of its
    // ciphertext and re-sealed deterministically from the content key, so any two
    // downloaders holding the **same content key** produce byte-identical,
    // interchangeable pieces and can swarm together. That holds for a shared
    // `arvc…` ticket (Plain key) and — because we share one identity across a
    // user's devices — also for a `--to` ticket Sealed to that identity: every
    // device unseals the same content key, so your own devices co-swarm a sealed
    // transfer. By this point `key` is the recovered content key regardless of
    // delivery mode; the swarm_id derives from the (sealed) ticket's chunk hashes,
    // so it stays secret to whoever opened the ticket and no stranger can join.
    // We seed the pieces we've verified (via `ChunkSeeder`) and discover/pull from
    // other peers through the relay tracker. `have_bf` is a **bitfield of verified
    // pieces** (arbitrary/disjoint, not a prefix) — the seeder serves any set bit
    // and the announce advertises the whole bitfield; the commit path sets bits.
    let have_bf: crate::chunked::HaveBitfield = Arc::new(Mutex::new(have_bits));
    let peers: SwarmPeers = Arc::new(Mutex::new(Vec::new()));
    // Peers (by endpoint id) that served corrupt bytes; filtered out of future
    // provider lists. Populated by `fetch_to_file` on an integrity failure.
    let banned: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    // Metric: pieces we pulled from a swarm peer (vs. the origin/relay).
    let from_peers = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let swarm_cancel = cancel.child_token();
    let mut seeder: Option<crate::chunked::ChunkSeeder> = None;
    let mut swarming = false;
    if swarm_enabled() {
        if let Some(r) = &t.relay {
            match crate::chunked::ChunkSeeder::start(
                download.clone(),
                key,
                &t.chunks,
                total_chunks,
                have_bf.clone(),
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
                            have_bf.clone(),
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

    // Fetch up to `concurrency` chunks in parallel (pipelining hides latency) and
    // commit each **out of order** the moment it's verified: decrypt it, write it at
    // its final offset `i*chunk_size` (a sparse file until complete), set its bit in
    // `have_bf`, and persist the sidecar. Disjoint pieces are what let a user's
    // devices swap different pieces. Each in-flight fetch stages its ciphertext in a
    // per-index `.arvpart.{i}` file (BLAKE3-verified by `fetch_one`).
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
    // Pieces still to fetch: everything not already present per the resume sidecar.
    let mut remaining: HashSet<usize> = {
        let bf = have_bf.lock().unwrap();
        (0..total)
            .filter(|&i| !crate::swarm::bitfield_has(&bf, i))
            .collect()
    };
    // Pieces already committed (seeded from the sidecar), so a late duplicate fetch
    // is discarded rather than re-committed.
    let mut satisfied: HashSet<usize> = {
        let bf = have_bf.lock().unwrap();
        (0..total)
            .filter(|&i| crate::swarm::bitfield_has(&bf, i))
            .collect()
    };
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
            // Phase 1: a fresh piece, rarest-first (fewest providers) with a RANDOM
            // tie-break among equally-rare pieces. Because the origin has every
            // piece, a piece a peer already holds has one more provider than one it
            // lacks, so rarest-first already steers each device toward pieces its
            // peers are missing; the random tie-break stops two devices in identical
            // state from picking the same piece, spreading distinct pieces faster.
            let fresh = {
                use rand::Rng;
                let mut cands: Vec<(usize, Vec<(String, iroh::EndpointAddr)>)> = remaining
                    .iter()
                    .copied()
                    .filter(|i| piece_backoff.get(i).map(|t| *t <= now).unwrap_or(true))
                    .filter_map(|i| {
                        let p = providers_having(i, &cooldown, now);
                        (!p.is_empty()).then_some((i, p))
                    })
                    .collect();
                cands.iter().map(|(_, p)| p.len()).min().map(|min| {
                    cands.retain(|(_, p)| p.len() == min);
                    cands.swap_remove(rand::rng().random_range(0..cands.len()))
                })
            };
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
                _ = cancel.cancelled() => { receiver.close().await; return Ok(RecvOutcome::Cancelled(download)); }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
            continue;
        }

        let joined = tokio::select! {
            _ = cancel.cancelled() => {
                // Partial stages are left on disk and can be resumed later.
                set.shutdown().await;
                receiver.close().await;
                return Ok(RecvOutcome::Cancelled(download));
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
                    // Commit out of order: decrypt, write at the final offset, then
                    // mark the bit (only after the bytes are flushed) and persist the
                    // sidecar so a restart resumes exactly the missing pieces.
                    let mut ct = Vec::new();
                    std::fs::File::open(&stage)
                        .with_context(|| format!("open {}", stage.display()))?
                        .read_to_end(&mut ct)?;
                    let plain = open_chunk(&key, i as u32, total_chunks, &ct)
                        .with_context(|| format!("decrypt chunk {}", i + 1))?;
                    file.seek(SeekFrom::Start(i as u64 * t.chunk_size as u64))?;
                    if let Err(io) = file.write_all(&plain).and_then(|()| file.flush()) {
                        let e = anyhow::Error::new(io).context("write chunk");
                        // Disk full mid-commit: pause resumably (the piece isn't marked
                        // in the bitfield, so a resume re-fetches it) instead of aborting.
                        if is_local_storage_error(&e) {
                            let reason = disk_full_reason(&download);
                            on(RecvEvent::Paused {
                                reason: reason.clone(),
                            });
                            receiver.close().await;
                            return Ok(RecvOutcome::Paused {
                                output: download,
                                reason,
                            });
                        }
                        return Err(e);
                    }
                    let _ = std::fs::remove_file(&stage);
                    let _ = std::fs::remove_file(stage_path(i));
                    for n in 1..=ENDGAME_PARALLEL {
                        let _ = std::fs::remove_file(stage_dup(i, n));
                    }
                    piece_cancel.remove(&i);
                    piece_srcs.remove(&i);
                    piece_backoff.remove(&i);
                    remaining.remove(&i);
                    {
                        let mut bf = have_bf.lock().unwrap();
                        crate::swarm::bitfield_set(&mut bf, i);
                        write_sidecar(&download, &bf);
                    }
                    on(RecvEvent::Chunk {
                        index: i,
                        total,
                        source,
                        bytes: plain.len() as u64,
                    });
                    // Ack to the sender (best-effort, via the supervisor — dropped
                    // while the control channel is down, resumed on reconnect).
                    if let Some(tx) = &ack_tx {
                        let _ = tx.send(i as u32);
                    }
                    // Free relay-backfilled chunks as we take them, UNLESS swarming —
                    // then keep the relay's copy so other peers can still fetch it.
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
                    // Surface swarm metrics (peer count + pieces pulled from peers).
                    if swarming {
                        on(RecvEvent::Swarm {
                            peers: peers.lock().unwrap().len(),
                            pieces_from_peers: from_peers
                                .load(std::sync::atomic::Ordering::Relaxed),
                        });
                    }
                }
            }
            Some(Err(e)) => {
                // A local, unrecoverable filesystem error (disk full / quota / read-only)
                // is NOT a provider fault: reassigning the piece to another peer would
                // just spin without progress. Stop cleanly and resumably — the partial
                // output + sidecar stay on disk — and report a pause to the caller.
                if is_local_storage_error(&e) {
                    let _ = std::fs::remove_file(&stage);
                    let reason = disk_full_reason(&download);
                    on(RecvEvent::Paused {
                        reason: reason.clone(),
                    });
                    receiver.close().await;
                    return Ok(RecvOutcome::Paused {
                        output: download,
                        reason,
                    });
                }
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
    }
    // Cancelled before completing (e.g. the token was already tripped): leave the
    // partial output (+ sidecar) as-is for a later resume, without finalizing.
    if cancel.is_cancelled()
        && crate::swarm::bitfield_count(&have_bf.lock().unwrap()) < total as u32
    {
        receiver.close().await;
        return Ok(RecvOutcome::Cancelled(download));
    }
    // Stop the control supervisor — but not before its acks are on the wire. The
    // last chunk's ack was queued moments ago; dropping it would leave the sender
    // believing the file never fully arrived (it would keep serving/re-offering
    // something we already have). Close the ack channel, cancel, then wait for the
    // supervisor to flush. Bounded: a sender that has vanished must not stall our
    // completion, and a lost ack then costs only a redundant offer, not the file.
    // Dropping the ack channel tells the supervisor the download is done; it then
    // flushes its acks and exits by itself. Wait for that instead of cancelling it,
    // because cancelling is what used to lose them: a small file can finish before
    // the control connection has even finished opening, and a cancel at that moment
    // strands every ack. The sender would then never learn the file arrived — it
    // would keep serving and re-offering something already delivered.
    //
    // Bounded, and only cancel if it overruns: a sender that vanished mid-close must
    // not hold up our completion, and a lost ack then costs a redundant offer, not
    // the file. The bound clears the graceful-close wait inside `Control::finish`.
    drop(ack_tx);
    match ctrl {
        Some(h) => {
            // Both senders must go for the channel to close: this one and the clone
            // above. Leaving the handle's copy alive keeps `recv()` pending forever,
            // so the supervisor never reaches its flush-and-exit and we would sit out
            // the whole timeout on every completed download.
            let ControlHandle {
                ack_tx: owner_tx,
                task,
                sender_live: _,
            } = h;
            drop(owner_tx);
            if tokio::time::timeout(std::time::Duration::from_secs(8), task)
                .await
                .is_err()
            {
                ctrl_cancel.cancel();
            }
        }
        None => ctrl_cancel.cancel(),
    }
    // Finalize the file so the seeder serves the last piece at its true length.
    file.set_len(t.total_size)?;
    drop(file);
    // Download complete — the resume sidecar is no longer needed.
    let _ = std::fs::remove_file(sidecar_path(&download));
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
        // Unpack the tar into the target directory, hardened against path-traversal
        // and symlink escapes (the archive comes from a possibly-anonymous sender).
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        unpack_archive_safely(&download, &dir)
            .with_context(|| format!("extract into {}", dir.display()))?;
        // Keep the staged tar iff we'll seed it (its bytes are exactly what the
        // sender sealed, so re-sealing reproduces the ticket's hashes). The manager
        // recomputes this path, seeds it, and deletes it when the session ends.
        // Otherwise drop it now so nothing is left behind.
        if !seeding_enabled() {
            let _ = std::fs::remove_file(&download);
        }
        on(RecvEvent::Saved { path: dir.clone() });
        Ok(RecvOutcome::Completed(dir))
    } else {
        on(RecvEvent::Saved {
            path: download.clone(),
        });
        Ok(RecvOutcome::Completed(download))
    }
}

/// A stable default output filename derived from a ticket seed.
pub fn default_out(seed: &str) -> PathBuf {
    PathBuf::from(format!("received-{}.bin", &seed[..seed.len().min(16)]))
}

/// Reduce a sender-supplied (untrusted) name to a single, safe path component,
/// stripping any directory parts so a malicious `name` like `../../.ssh/x` or an
/// absolute path can't escape the intended download directory. Returns `None` when
/// nothing usable remains (empty, `.`, `..`) so the caller can fall back to a
/// generated name.
pub fn safe_download_name(name: &str) -> Option<String> {
    std::path::Path::new(name)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty() && s != "." && s != "..")
}

/// Default single-file output: the ticket's suggested name (its final path
/// component, to avoid traversal), falling back to a seed-derived name.
fn default_from_name(name: &str, chunks: &[crate::reexport::Hash]) -> PathBuf {
    match safe_download_name(name) {
        Some(n) => PathBuf::from(n),
        None => default_out(&chunks.first().map(|h| h.to_string()).unwrap_or_default()),
    }
}
