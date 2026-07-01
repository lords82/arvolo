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
use crate::chunked::{ChunkReceiver, ChunkSender, ChunkTicket, SeedRequest};
use crate::crypto::{open, open_chunk, seal, Identity, PublicId, Sealed};
use crate::offline::OfflineTicket;
use crate::transfer::RelayChoice;

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

/// Split and serve `path`; with `seed_relay` set, learn the relay's address +
/// token so the tail can be backfilled if the receiver drops (nothing is
/// uploaded yet — that's lazy, in [`SendSession::serve`]).
pub async fn prepare_send(
    path: &Path,
    name: &str,
    archive: bool,
    seed_relay: Option<String>,
    relay: RelayChoice,
) -> Result<SendSession> {
    anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
    let sender = ChunkSender::serve(path, relay).await.context("start sender")?;
    let client = reqwest::Client::new();

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
        relay = Some(RelayRelease { http: url, addr, token });
    }

    let ticket = ChunkTicket {
        total_size: sender.total_size(),
        chunk_size: sender.chunk_size(),
        chunks: sender.chunks().to_vec(),
        providers: vec![sender.addr()],
        relay: relay.clone(),
        key: sender.key().to_vec(),
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

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                undelivered = self.sender.receiver_gone() => {
                    let Some(r) = &self.relay else { continue };
                    if undelivered.is_empty() { continue; }
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

/// Fetch the file described by `ticket` into `out` (default derived from the
/// ticket). Resumes a partial output, prefers P2P, falls back to the relay, and
/// releases relay chunks as they're taken. Returns the output path. If `cancel`
/// fires mid-transfer it returns early with the partial (resumable) output.
pub async fn recv_chunked(
    ticket: &str,
    out: Option<PathBuf>,
    relay: RelayChoice,
    cancel: CancellationToken,
    on: impl Fn(RecvEvent) + Send + Sync,
) -> Result<PathBuf> {
    use std::collections::HashSet;
    use std::io::{Seek, SeekFrom, Write};

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
    let key: [u8; crate::crypto::CHUNK_KEY_LEN] = t
        .key
        .as_slice()
        .try_into()
        .context("ticket has no/invalid content key")?;
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
                    message: format!("control channel attempt {attempt}/{attempts} failed; retrying…"),
                }),
                _ => {}
            }
        }
    }
    on(RecvEvent::Control { connected: control.is_some() });
    if control.is_some() {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    // No control channel => the sender is most likely offline; prefer the relay.
    let sender_offline = control.is_none();

    for i in start..t.chunks.len() {
        if cancel.is_cancelled() {
            // Partial output is left on disk and can be resumed later.
            receiver.close().await;
            return Ok(download);
        }
        let on_relay_chunk = on_relay.lock().unwrap().contains(&(i as u32));
        // Anti-double-send: chunks the sender pushed to the relay are pulled from
        // the relay; everything else is pulled from the sender (relay fallback).
        let (source, providers) = {
            let mut providers = Vec::new();
            let relay_first = on_relay_chunk || sender_offline;
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

        let bytes = receiver
            .fetch_chunk(&providers, t.chunks[i])
            .await
            .with_context(|| format!("fetch chunk {}", i + 1))?;
        // Fetched bytes are ciphertext (providers never see plaintext).
        let plain = open_chunk(&key, i as u32, total_chunks, &bytes)
            .with_context(|| format!("decrypt chunk {}", i + 1))?;
        file.seek(SeekFrom::Start(i as u64 * t.chunk_size as u64))?;
        file.write_all(&plain)?;
        on(RecvEvent::Chunk {
            index: i,
            total: t.chunks.len(),
            source,
            bytes: plain.len() as u64,
        });

        if let Some(c) = control.as_mut() {
            let _ = c.ack(i as u32).await;
        }
        // Free relay-backfilled chunks as we take them. Attempt release for every
        // chunk: with the sender offline there's no control channel to learn
        // `on_relay`, yet those are the chunks the relay holds. The relay's
        // (token, hash) guard makes it a no-op for anything not seeded.
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
    if let Some(c) = control {
        let _ = c.finish().await;
    }
    file.set_len(t.total_size)?;
    drop(file);
    receiver.close().await;

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
        on(RecvEvent::Saved { path: download.clone() });
        Ok(download)
    }
}

// ---- offline mailbox ------------------------------------------------------

/// Encrypt `path` for `recipient` (authenticated as `me`) and deposit the
/// ciphertext on the relay. Returns the offline ticket to hand to the recipient.
pub async fn deposit_offline(
    path: &Path,
    recipient: &PublicId,
    me: &Identity,
    relay: &str,
    ttl: u64,
    max: u32,
) -> Result<OfflineTicket> {
    anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
    let plaintext = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let sealed = seal(&plaintext, recipient, me, b"").context("encrypt")?;

    let relay = relay.trim_end_matches('/').to_string();
    let url = format!("{relay}/v1/deposit?ttl={ttl}&max={max}");
    let claim = reqwest::Client::new()
        .post(&url)
        .header(
            "x-arvolo-encapped-key",
            data_encoding::BASE32_NOPAD.encode(&sealed.encapped_key),
        )
        .body(sealed.ciphertext)
        .send()
        .await
        .context("deposit request")?
        .error_for_status()
        .context("relay rejected deposit")?
        .text()
        .await
        .context("read claim")?;

    Ok(OfflineTicket {
        relay,
        claim: claim.trim().to_string(),
        sender: me.public().to_bytes(),
    })
}

/// Fetch and decrypt an offline ticket into `out` (default derived from the
/// claim). Returns the output path and the number of plaintext bytes written.
pub async fn fetch_offline(
    ticket: &str,
    out: Option<PathBuf>,
    me: &Identity,
) -> Result<(PathBuf, usize)> {
    let t = OfflineTicket::decode(ticket)?;
    let sender = PublicId::from_bytes(&t.sender).context("invalid sender in ticket")?;

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
    let ciphertext = resp.bytes().await.context("read ciphertext")?.to_vec();

    let plaintext = open(
        &Sealed { encapped_key: encapped, ciphertext },
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
pub fn pack_tar(paths: &[PathBuf], dest: &Path) -> Result<()> {
    let file = std::fs::File::create(dest)
        .with_context(|| format!("create archive {}", dest.display()))?;
    let mut builder = tar::Builder::new(file);
    for p in paths {
        let base = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        if p.is_dir() {
            builder
                .append_dir_all(&base, p)
                .with_context(|| format!("archive dir {}", p.display()))?;
        } else {
            builder
                .append_path_with_name(p, &base)
                .with_context(|| format!("archive file {}", p.display()))?;
        }
    }
    builder.finish().context("finish archive")?;
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
