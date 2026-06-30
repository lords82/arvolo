//! Seed-to-relay backfill: durable P2P delivery.
//!
//! The sender serves a blob P2P *and* seeds it to a relay that runs an
//! iroh-blobs store node. Once seeded, the blob lives in two places. The sender
//! can then go offline: the receiver fetches by hash and, because the transfer
//! is content-addressed, **falls back from the sender to the relay** and resumes
//! across providers without losing progress.

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

use crate::node::{decode_ticket, encode_ticket};
use crate::transfer::{bind_endpoint, export, fetch_into, recv_store_dir, RelayChoice};

const PREFIX: &str = "arvp";

/// How the receiver tells the relay it's done, so the seeded blob can be deleted
/// immediately (delete-after-delivery) instead of only at TTL.
#[derive(Clone, Serialize, Deserialize)]
pub struct RelayRelease {
    /// Relay HTTP base URL, e.g. `http://relay:8787`.
    pub http: String,
    /// One-time release token for this seeded blob.
    pub token: String,
}

#[derive(Serialize, Deserialize)]
struct Wire {
    hash: Hash,
    providers: Vec<EndpointAddr>,
    #[serde(default)]
    relay: Option<RelayRelease>,
}

/// A multi-provider ticket: a blob hash plus the addresses that can serve it
/// (the sender and any relay it was seeded to). The receiver tries them in
/// order and resumes across providers thanks to the content-addressed store.
pub struct ProviderTicket {
    pub hash: Hash,
    pub providers: Vec<EndpointAddr>,
    /// If seeded to a relay, how to release (delete) the relay copy after delivery.
    pub relay: Option<RelayRelease>,
}

impl ProviderTicket {
    /// Encode to a single pasteable string (`arvp…`).
    pub fn encode(&self) -> Result<String> {
        let bytes = postcard::to_allocvec(&Wire {
            hash: self.hash,
            providers: self.providers.clone(),
            relay: self.relay.clone(),
        })
        .context("serialize provider ticket")?;
        Ok(format!(
            "{PREFIX}{}",
            data_encoding::BASE32_NOPAD.encode(&bytes)
        ))
    }

    /// Parse a string produced by [`ProviderTicket::encode`].
    pub fn decode(s: &str) -> Result<Self> {
        let body = s
            .trim()
            .strip_prefix(PREFIX)
            .ok_or_else(|| anyhow!("not a provider ticket (missing {PREFIX} prefix)"))?;
        let bytes = data_encoding::BASE32_NOPAD
            .decode(body.to_uppercase().as_bytes())
            .context("decode provider ticket")?;
        let w: Wire = postcard::from_bytes(&bytes).context("deserialize provider ticket")?;
        Ok(Self {
            hash: w.hash,
            providers: w.providers,
            relay: w.relay,
        })
    }

    /// Is `s` a provider ticket?
    pub fn looks_like(s: &str) -> bool {
        s.trim_start().starts_with(PREFIX)
    }
}

/// A relay-side blob provider: holds seeded blobs in a persistent store, serves
/// them to receivers, and can pull a blob from a sender's ticket.
pub struct BlobNode {
    endpoint: Endpoint,
    store: FsStore,
    _router: Router,
}

impl BlobNode {
    /// Start a blob node backed by `store_dir`, reachable over the given relay.
    pub async fn spawn(store_dir: &Path, relay: RelayChoice) -> Result<Self> {
        std::fs::create_dir_all(store_dir).ok();
        let endpoint = bind_endpoint(relay).await?;
        // Enable periodic GC: blobs are kept alive by a tag while needed; once a
        // tag is removed (on release / TTL) GC deletes the blob within `interval`.
        let mut opts = Options::new(store_dir);
        opts.gc = Some(GcConfig {
            interval: Duration::from_secs(30),
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

    /// Pull the blob described by the sender's ticket string into this node's
    /// store and protect it with a tag (so GC keeps it). Returns this node's
    /// address (base32) and the blob hash (hex), so the relay can track and
    /// later release it.
    pub async fn seed_from_ticket(&self, ticket_str: &str) -> Result<(String, String)> {
        let ticket =
            BlobTicket::from_str(ticket_str.trim()).map_err(|e| anyhow!("bad blob ticket: {e}"))?;
        let hash = ticket.hash();
        fetch_into(&self.store, &self.endpoint, &ticket)
            .await
            .context("seed blob into relay")?;
        self.store
            .tags()
            .set(tag_name(&hash), hash)
            .await
            .context("tag seeded blob")?;
        Ok((encode_ticket(&self.addr())?, hash.to_string()))
    }

    /// Release a seeded blob (by hex hash): remove its protecting tag so the
    /// store's periodic GC deletes it. Idempotent.
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

/// Ask a relay (HTTP) to seed `sender_ticket`, returning the relay's provider
/// address. Used by the sender. (HTTP call lives in the CLI; this just parses.)
pub fn parse_relay_addr(encoded: &str) -> Result<EndpointAddr> {
    decode_ticket(encoded)
}

/// Receiver: fetch the blob from the first reachable provider in `ticket`,
/// resuming across providers via the persistent store. Writes to `out`.
pub async fn fetch_from_providers(
    ticket: &ProviderTicket,
    out: &Path,
    relay: RelayChoice,
) -> Result<Hash> {
    let endpoint = bind_endpoint(relay).await?;
    let dir = recv_store_dir(&ticket.hash);
    std::fs::create_dir_all(&dir).ok();
    let store = FsStore::load(&dir)
        .await
        .map_err(|e| anyhow!("open store: {e}"))?;

    let mut last_err: Option<anyhow::Error> = None;
    for addr in &ticket.providers {
        let bt = BlobTicket::new(addr.clone(), ticket.hash, BlobFormat::Raw);
        match fetch_into(&store, &endpoint, &bt).await {
            Ok(_) => {
                last_err = None;
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }

    // Export succeeds only if the blob is complete (resumed across providers).
    export(&store, ticket.hash, out)
        .await
        .map_err(|e| match &last_err {
            Some(le) => anyhow!("no provider could complete the transfer ({le}); {e}"),
            None => e,
        })?;

    endpoint.close().await;
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(ticket.hash)
}
