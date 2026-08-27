//! Relay-side chunk store for backfill.
//!
//! The relay holds **seeded chunks** as plain ciphertext files (one per BLAKE3
//! hash) and serves them over the custom chunk protocol ([`CHUNK_ALPN`]). It
//! pulls chunks from the sender with the same protocol. Releasing a chunk just
//! deletes its file (freeing storage *during* the download); expiry is a TTL
//! backstop in the relay's mailbox.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use crate::hash::Hash;
use anyhow::{Context, Result};
use iroh::{protocol::Router, Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};

use crate::chunked::{fetch_chunk_wire, ChunkServer, ConnPool, CHUNK_ALPN};
use crate::node::encode_ticket;
use crate::transfer::{bind_endpoint, RelayChoice};

/// Relay coordinates for chunked backfill: where the relay is (HTTP + iroh
/// address) and the per-transfer token used to seed/release/fetch its chunks.
#[derive(Clone, Serialize, Deserialize)]
pub struct RelayRelease {
    /// Relay HTTP base URL, e.g. `http://relay:8787`.
    pub http: String,
    /// The relay's iroh chunk-node address (base32), for fetching backfilled chunks.
    pub addr: String,
    /// Per-transfer token authorizing seed/release of this transfer's chunks.
    pub token: String,
}

/// A relay-side chunk provider: holds seeded ciphertext files and serves them.
pub struct BlobNode {
    endpoint: Endpoint,
    dir: PathBuf,
    /// Warm connections to the senders this node seeds from: a backfill pulls
    /// hundreds of chunks from one peer, and without this each paid a handshake.
    pool: ConnPool,
    _router: Router,
}

impl BlobNode {
    /// Start a chunk node backed by `store_dir` (one file per chunk hash),
    /// reachable over the given relay.
    pub async fn spawn(store_dir: &Path, relay: RelayChoice) -> Result<Self> {
        let dir = store_dir.to_path_buf();
        std::fs::create_dir_all(&dir).ok();
        let use_relay = !matches!(relay, RelayChoice::Disabled);
        let endpoint = bind_endpoint(relay).await?;
        let router = Router::builder(endpoint.clone())
            .accept(CHUNK_ALPN, ChunkServer::files(dir.clone()))
            .spawn();
        // Wait for a dialable address (relay home + reflexive), but never block
        // startup forever; with the relay disabled (local/tests) skip it.
        if use_relay {
            let _ = tokio::time::timeout(Duration::from_secs(10), endpoint.online()).await;
        }
        Ok(Self {
            endpoint,
            dir,
            pool: ConnPool::default(),
            _router: router,
        })
    }

    /// This node's dialable address (relay URL + direct addresses).
    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// This node's address, base32-encoded for tickets.
    pub fn addr_encoded(&self) -> Result<String> {
        encode_ticket(&self.addr())
    }

    fn chunk_path(&self, hash: &Hash) -> PathBuf {
        self.dir.join(hash.to_string())
    }

    /// Pull each chunk (by hash) from `sender` and store it as a ciphertext file,
    /// verifying `BLAKE3(ciphertext) == hash`. Returns this node's address
    /// (base32) for advertising the relay as a provider.
    pub async fn seed_chunks(&self, sender: EndpointAddr, chunks: &[Hash]) -> Result<String> {
        // Files written by this call, so a failure part-way through can take them
        // back out. The caller records the bookkeeping rows only once this returns
        // Ok, so anything left behind here is a file no row will ever name again —
        // storage that nothing can free short of the reconciliation sweep.
        let mut written: Vec<PathBuf> = Vec::new();
        for hash in chunks {
            if self.chunk_path(hash).exists() {
                continue; // already held
            }
            // Recorded before the attempt, so a half-written file is cleaned up too.
            written.push(self.chunk_path(hash));
            if let Err(e) = self.seed_one(&sender, hash).await {
                for path in &written {
                    let _ = std::fs::remove_file(path);
                }
                return Err(e);
            }
        }
        encode_ticket(&self.addr())
    }

    /// Pull one chunk and write it, verifying `BLAKE3(ciphertext) == hash`.
    async fn seed_one(&self, sender: &EndpointAddr, hash: &Hash) -> Result<()> {
        let (total_len, ct) = fetch_chunk_wire(&self.endpoint, &self.pool, sender, *hash, 0)
            .await
            .with_context(|| format!("seed chunk {hash}"))?;
        anyhow::ensure!(
            ct.len() as u64 == total_len && Hash::new(&ct) == *hash,
            "seeded chunk {hash} failed integrity check"
        );
        std::fs::write(self.chunk_path(hash), &ct)
            .with_context(|| format!("write chunk {hash}"))?;
        Ok(())
    }

    /// Which of `chunks` this node already holds, in the order asked.
    ///
    /// Answered from the files, not from the relay's `seeded` table: the files are
    /// what can actually be served, and the two can disagree (a row whose write
    /// died, a file the reconciliation sweep has yet to reap). A caller acts on
    /// this by *adding* the relay as a candidate provider, so an over-optimistic
    /// answer costs one failed fetch and a cooldown, never a wrong byte.
    pub fn has_chunks(&self, chunks: &[Hash]) -> Vec<bool> {
        chunks.iter().map(|h| self.chunk_path(h).exists()).collect()
    }

    /// Where this node keeps its chunk files. The relay reconciles this directory
    /// against its `seeded` table to catch files no row covers any more.
    pub fn store_dir(&self) -> &Path {
        &self.dir
    }

    /// Release a chunk (by hex hash): delete its file. Idempotent.
    pub async fn release_hex(&self, hash_hex: &str) -> Result<()> {
        let hash = Hash::from_str(hash_hex).context("parse hash")?;
        let _ = std::fs::remove_file(self.chunk_path(&hash));
        Ok(())
    }
}

/// How many hashes one `/v1/have` question may carry. Matches the relay's own
/// cap; a longer list is split by [`relay_have`] rather than refused, so a caller
/// never has to know the limit.
pub const HAVE_BATCH: usize = 4096;

/// Take a parked send back off the relay: forget every chunk seeded under
/// `token` and delete the files. Returns how many were dropped.
///
/// The withdrawal half of parking. Without it a sender that changed its mind
/// could stop pointing at the pieces but not remove them, which is a worse
/// promise than the mailbox makes — there, a cancel really does take the file
/// back. Idempotent: a token with nothing left is a success, because what the
/// caller wanted (nothing of theirs on this relay) already holds.
pub async fn revoke_seeded(client: &reqwest::Client, http: &str, token: &str) -> Result<usize> {
    let url = format!("{}/v1/seeded/{}", http.trim_end_matches('/'), token);
    let resp = client
        .delete(&url)
        .send()
        .await
        .context("relay /v1/seeded")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        // A relay too old to know the route. Nothing was withdrawn, and saying so
        // is better than reporting a withdrawal that did not happen: the pieces
        // lapse at their TTL instead.
        anyhow::bail!("this relay cannot withdraw parked chunks (too old)");
    }
    let body = resp
        .error_for_status()
        .context("relay refused the withdrawal")?
        .text()
        .await
        .unwrap_or_default();
    Ok(body.trim().parse().unwrap_or(0))
}

/// Ask the relay which of `chunks` it currently holds, as indices into `chunks`.
///
/// This is the discovery both ends need and neither could do before: the set of
/// backfilled pieces used to travel *only* from the sender to the receiver over
/// the control channel ([`crate::chunked::CtrlMsg::RelayHas`]), so it was
/// unknowable precisely when the sender was away — which is the whole situation
/// the relay's copy exists for. The receiver uses it to find a source with nobody
/// to ask; the sender uses it to avoid re-seeding what is already there.
///
/// A relay too old to know the route answers 404 and we report an empty set: the
/// caller then behaves exactly as it did before this existed (the receiver waits
/// for `RelayHas`, the sender seeds the lot), which is the right way to be wrong.
pub async fn relay_have(
    client: &reqwest::Client,
    http: &str,
    chunks: &[Hash],
) -> Result<std::collections::HashSet<u32>> {
    use crate::chunked::{decode_have_bitfield, HaveRequest};
    let base = http.trim_end_matches('/');
    let mut held = std::collections::HashSet::new();
    for (batch, offset) in chunks.chunks(HAVE_BATCH).zip((0..).step_by(HAVE_BATCH)) {
        let req = HaveRequest {
            chunks: batch.to_vec(),
        };
        let resp = client
            .post(format!("{base}/v1/have"))
            .body(req.encode()?)
            .send()
            .await
            .context("relay /v1/have")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(std::collections::HashSet::new());
        }
        let body = resp
            .error_for_status()
            .context("relay rejected have query")?
            .text()
            .await
            .context("read have answer")?;
        let bits = decode_have_bitfield(&body, batch.len())?;
        for i in 0..batch.len() {
            if crate::swarm::bitfield_has(&bits, i) {
                held.insert((offset + i) as u32);
            }
        }
    }
    Ok(held)
}
