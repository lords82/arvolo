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
