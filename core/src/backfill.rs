//! Relay-side blob store for chunked backfill.
//!
//! The relay runs an iroh-blobs store node that holds **seeded chunks** (each an
//! independent content-addressed blob), protected by a tag. The receiver
//! releases each chunk as it gets it; releasing removes the tag and the store's
//! periodic GC deletes the blob — so storage is freed *during* the download.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use iroh::{protocol::Router, Endpoint, EndpointAddr};
use iroh_blobs::{
    store::fs::{options::Options, FsStore},
    store::GcConfig,
    ticket::BlobTicket,
    BlobFormat, BlobsProtocol, Hash,
};
use serde::{Deserialize, Serialize};

use crate::node::encode_ticket;
use crate::transfer::{bind_endpoint, fetch_into, RelayChoice};

/// Relay coordinates for chunked backfill: where the relay is (HTTP + iroh
/// address) and the per-transfer token used to seed/release/fetch its chunks.
#[derive(Clone, Serialize, Deserialize)]
pub struct RelayRelease {
    /// Relay HTTP base URL, e.g. `http://relay:8787`.
    pub http: String,
    /// The relay's iroh blob-node address (base32), for fetching backfilled chunks.
    pub addr: String,
    /// Per-transfer token authorizing seed/release of this transfer's chunks.
    pub token: String,
}

/// A relay-side blob provider: holds seeded chunks and serves them; releasing a
/// chunk untags it so the store's GC deletes it.
pub struct BlobNode {
    endpoint: Endpoint,
    store: FsStore,
    _router: Router,
}

impl BlobNode {
    /// Start a blob node backed by `store_dir`, reachable over the given relay.
    /// Periodic GC deletes any untagged (released/expired) blob within ~15s.
    pub async fn spawn(store_dir: &Path, relay: RelayChoice) -> Result<Self> {
        std::fs::create_dir_all(store_dir).ok();
        let endpoint = bind_endpoint(relay).await?;
        let mut opts = Options::new(store_dir);
        opts.gc = Some(GcConfig {
            interval: Duration::from_secs(15),
            add_protected: None,
        });
        let store = FsStore::load_with_opts(store_dir.join("blobs.db"), opts)
            .await
            .map_err(|e| anyhow!("open blob store: {e}"))?;
        let blobs = BlobsProtocol::new(&store, None);
        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs)
            .spawn();
        endpoint.online().await;
        Ok(Self {
            endpoint,
            store,
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

    /// Pull each chunk (by hash) from `sender` into this node's store and tag it,
    /// so the chunks are served and kept until released. Returns this node's
    /// address (base32) for advertising the relay as a provider.
    pub async fn seed_chunks(&self, sender: EndpointAddr, chunks: &[Hash]) -> Result<String> {
        for hash in chunks {
            let bt = BlobTicket::new(sender.clone(), *hash, BlobFormat::Raw);
            fetch_into(&self.store, &self.endpoint, &bt)
                .await
                .with_context(|| format!("seed chunk {hash}"))?;
            self.store
                .tags()
                .set(tag_name(hash), *hash)
                .await
                .context("tag chunk")?;
        }
        encode_ticket(&self.addr())
    }

    /// Release a chunk (by hex hash): remove its tag so GC deletes it. Idempotent.
    pub async fn release_hex(&self, hash_hex: &str) -> Result<()> {
        let hash = Hash::from_str(hash_hex).context("parse hash")?;
        self.store
            .tags()
            .delete(tag_name(&hash))
            .await
            .context("delete tag")?;
        Ok(())
    }
}

fn tag_name(hash: &Hash) -> String {
    format!("seed/{hash}")
}
