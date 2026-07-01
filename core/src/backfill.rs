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

use crate::chunked::{fetch_chunk_wire, ChunkServer, CHUNK_ALPN};
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
        for hash in chunks {
            if self.chunk_path(hash).exists() {
                continue; // already held
            }
            let (total_len, ct) = fetch_chunk_wire(&self.endpoint, &sender, *hash, 0)
                .await
                .with_context(|| format!("seed chunk {hash}"))?;
            anyhow::ensure!(
                ct.len() as u64 == total_len && Hash::new(&ct) == *hash,
                "seeded chunk {hash} failed integrity check"
            );
            std::fs::write(self.chunk_path(hash), &ct)
                .with_context(|| format!("write chunk {hash}"))?;
        }
        encode_ticket(&self.addr())
    }

    /// Release a chunk (by hex hash): delete its file. Idempotent.
    pub async fn release_hex(&self, hash_hex: &str) -> Result<()> {
        let hash = Hash::from_str(hash_hex).context("parse hash")?;
        let _ = std::fs::remove_file(self.chunk_path(&hash));
        Ok(())
    }
}
