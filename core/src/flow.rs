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

    let receiver = ChunkReceiver::open(relay).await?;
    let client = reqwest::Client::new();

    // Control channel to the sender. Patience scales with fallback availability:
    // one short attempt if a relay can finish the job, three for pure P2P.
    let on_relay: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut control = None;
    if let Some(s) = &sender_addr {
        let attempts = if t.relay.is_some() { 1 } else { 3 };
        for attempt in 1..=attempts {
            match tokio::time::timeout(
                std::time::Duration::from_secs(12),
                receiver.open_control(s, on_relay.clone()),
            )
            .await
            {
                Ok(Some(c)) => {
                    control = Some(c);
                    break;
                }
                _ if attempt < attempts => on(RecvEvent::Warning {
                    message: format!(
                        "control channel attempt {attempt}/{attempts} failed; retrying…"
                    ),
                }),
                _ => {}
            }
        }
    }
    on(RecvEvent::Control {
        connected: control.is_some(),
    });
    if control.is_some() {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    // No control channel => the sender is most likely offline; prefer the relay.
    let sender_offline = control.is_none();

    // Fetch up to `concurrency` chunks in parallel (pipelining hides latency),
    // but commit them to the output **in order** so the file grows contiguously
    // and the length-based resume above stays correct. Each in-flight fetch
    // stages its ciphertext in a per-index `.arvpart.{i}` file (BLAKE3-verified
    // by `fetch_to_file`); the committer then decrypts and positions each chunk.
    let concurrency = fetch_concurrency();
    let total = t.chunks.len();
    let stage_path = |i: usize| PathBuf::from(format!("{}.arvpart.{i}", download.display()));
    // (source, ordered providers) for a chunk: relay-first when the sender pushed
    // it to the relay or is offline, else sender-first with relay fallback.
    let providers_for = |i: usize| {
        let relay_first = on_relay.lock().unwrap().contains(&(i as u32)) || sender_offline;
        let mut providers = Vec::new();
        if relay_first {
            relay_addr.iter().for_each(|a| providers.push(a.clone()));
            sender_addr.iter().for_each(|a| providers.push(a.clone()));
        } else {
            sender_addr.iter().for_each(|a| providers.push(a.clone()));
            relay_addr.iter().for_each(|a| providers.push(a.clone()));
        }
        let source = if relay_first {
            ChunkSource::Relay
        } else {
            ChunkSource::Sender
        };
        (source, providers)
    };

    let mut set: JoinSet<Result<usize>> = JoinSet::new();
    let mut spawn_idx = start;
    let mut next_commit = start;
    let mut ready: HashSet<usize> = HashSet::new();
    let mut sources: HashMap<usize, ChunkSource> = HashMap::new();

    loop {
        // Refill the in-flight window with fetch-only tasks.
        while spawn_idx < total && set.len() < concurrency && !cancel.is_cancelled() {
            let i = spawn_idx;
            let (source, providers) = providers_for(i);
            sources.insert(i, source);
            let rx = receiver.clone();
            let hash = t.chunks[i];
            let sp = stage_path(i);
            set.spawn(async move {
                let mut part = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(&sp)
                    .with_context(|| format!("open {}", sp.display()))?;
                rx.fetch_to_file(&providers, hash, &mut part)
                    .await
                    .with_context(|| format!("fetch chunk {}", i + 1))?;
                Ok::<usize, anyhow::Error>(i)
            });
            spawn_idx += 1;
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
            if let Some(c) = control.as_mut() {
                let _ = c.ack(i as u32).await;
            }
            // Free relay-backfilled chunks as we take them. Attempt release for
            // every chunk: with the sender offline there's no control channel to
            // learn `on_relay`, yet those are the chunks the relay holds. The
            // relay's (token, hash) guard makes it a no-op for anything not seeded.
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
            next_commit += 1;
        }
    }
    // Cancelled before completing (e.g. the token was already tripped): leave the
    // partial output as-is for a later resume, without finalizing its size.
    if cancel.is_cancelled() && next_commit < total {
        receiver.close().await;
        return Ok(download);
    }
    if let Some(c) = control {
        let _ = c.finish().await;
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
