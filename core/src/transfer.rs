//! File transfer over iroh-blobs (Steps 0.3–0.5).
//!
//! Content-addressed, BLAKE3 verified streaming. The provider serves a blob by
//! hash; the receiver fetches it by hash from a [`BlobTicket`] and verifies
//! integrity end-to-end. Resume comes for free from the content-addressed store
//! (re-fetching a blob already present transfers nothing); relay fallback comes
//! from iroh's addressing (a ticket carrying only a relay URL routes via relay).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use iroh::{protocol::Router, Endpoint, EndpointAddr, RelayMode};
use iroh_blobs::{
    api::Store, get::Stats, store::fs::FsStore, store::mem::MemStore, ticket::BlobTicket,
    BlobFormat, BlobsProtocol, Hash,
};

use crate::node::{generate_secret_key, local_addr_of};

/// A running provider that serves a single blob to anyone holding its ticket.
pub struct Provider {
    _router: Router,
    _store: MemStore,
    endpoint: Endpoint,
    hash: Hash,
}

impl Provider {
    /// Serve `path` on a pre-built `endpoint` (caller controls relay/discovery).
    pub async fn serve_on(endpoint: Endpoint, path: &Path) -> Result<Self> {
        let store = MemStore::new();
        let tag = store
            .blobs()
            .add_path(path)
            .await
            .with_context(|| format!("add file {}", path.display()))?;
        let blobs = BlobsProtocol::new(&store, None);
        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs)
            .spawn();
        Ok(Self {
            _router: router,
            _store: store,
            endpoint,
            hash: tag.hash,
        })
    }

    /// Serve `path` using the relay from the environment (n0 or self-hosted).
    pub async fn from_path(path: &Path) -> Result<Self> {
        Self::serve_on(bind_endpoint(RelayChoice::from_env()).await?, path).await
    }

    /// Serve `path` for local/direct use only (relay disabled). Used by tests/LAN.
    pub async fn from_path_local(path: &Path) -> Result<Self> {
        Self::serve_on(bind_endpoint(RelayChoice::Disabled).await?, path).await
    }

    /// Shareable ticket using this host's direct (loopback/LAN) addresses.
    pub fn ticket(&self) -> BlobTicket {
        self.ticket_via(local_addr_of(&self.endpoint))
    }

    /// Shareable ticket using an explicit address (e.g. relay-only).
    pub fn ticket_via(&self, addr: EndpointAddr) -> BlobTicket {
        BlobTicket::new(addr, self.hash, BlobFormat::Raw)
    }

    /// Wait until the endpoint is online (relay reachable) and return a ticket
    /// dialable over the internet (relay URL + discovered direct addresses).
    pub async fn ticket_online(&self) -> BlobTicket {
        self.endpoint.online().await;
        BlobTicket::new(self.endpoint.addr(), self.hash, BlobFormat::Raw)
    }

    /// The BLAKE3 hash of the served blob.
    pub fn hash(&self) -> Hash {
        self.hash
    }

    /// The underlying endpoint (id, addr, online()...).
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Shut the provider down.
    pub async fn shutdown(self) {
        self.endpoint.close().await;
    }
}

/// Fetch the blob in `ticket` into `store` over `endpoint`, returning transfer
/// [`Stats`]. Re-fetching a blob already present in `store` transfers nothing
/// (this is how resume works: a partially-filled store continues where it left off).
pub async fn fetch_into(store: &Store, endpoint: &Endpoint, ticket: &BlobTicket) -> Result<Stats> {
    let conn = endpoint
        .connect(ticket.addr().clone(), iroh_blobs::ALPN)
        .await
        .map_err(|e| anyhow::anyhow!("connect to provider: {e}"))?;
    store
        .remote()
        .fetch(conn, ticket.hash_and_format())
        .await
        .map_err(|e| anyhow::anyhow!("fetch blob: {e}"))
}

/// Export a complete blob from `store` to `out`.
pub async fn export(store: &Store, hash: Hash, out: &Path) -> Result<()> {
    store
        .blobs()
        .export(hash, out)
        .await
        .with_context(|| format!("export to {}", out.display()))?;
    Ok(())
}

/// Convenience: fetch the blob described by `ticket`, write it to `out`, and
/// return its hash, using the given relay choice for NAT traversal.
pub async fn fetch_to_path(ticket: &BlobTicket, out: &Path, relay: RelayChoice) -> Result<Hash> {
    let endpoint = bind_endpoint(relay).await?;
    // Persistent store keyed by hash: if a previous attempt was interrupted, the
    // partial data is still here and the fetch resumes from where it left off.
    let store_dir = recv_store_dir(&ticket.hash());
    std::fs::create_dir_all(&store_dir).ok();
    let store = FsStore::load(&store_dir)
        .await
        .map_err(|e| anyhow!("open resume store: {e}"))?;

    fetch_into(&store, &endpoint, ticket).await?;
    export(&store, ticket.hash(), out).await?;

    endpoint.close().await;
    drop(store);
    // Transfer complete: drop the resume cache.
    let _ = std::fs::remove_dir_all(&store_dir);
    Ok(ticket.hash())
}

/// Directory holding the resumable receive store for a given blob hash.
/// Override the base with `ARVOLO_CACHE`.
pub(crate) fn recv_store_dir(hash: &Hash) -> PathBuf {
    let base = std::env::var("ARVOLO_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("arvolo-recv"));
    base.join(hash.to_string())
}

/// Which iroh relays to use for NAT traversal / fallback. The relay only ever
/// carries encrypted QUIC traffic; it never sees plaintext.
#[derive(Clone, Debug)]
pub enum RelayChoice {
    /// n0's public relays (shared; suitable for dev/test).
    N0Default,
    /// No relay — direct only (LAN / tests).
    Disabled,
    /// A self-hosted iroh relay URL (production / full sovereignty).
    Custom(String),
}

impl RelayChoice {
    /// Read from `ARVOLO_IROH_RELAY`: a self-hosted relay URL if set, else n0 defaults.
    pub fn from_env() -> Self {
        match std::env::var("ARVOLO_IROH_RELAY") {
            Ok(u) if !u.trim().is_empty() => RelayChoice::Custom(u.trim().to_string()),
            _ => RelayChoice::N0Default,
        }
    }
}

/// Bind an endpoint for blob transfer with the given relay choice.
pub async fn bind_endpoint(relay: RelayChoice) -> Result<Endpoint> {
    let builder = match relay {
        RelayChoice::N0Default => Endpoint::builder(),
        RelayChoice::Disabled => Endpoint::empty_builder(RelayMode::Disabled),
        RelayChoice::Custom(url) => {
            let parsed: iroh::RelayUrl = url
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid ARVOLO_IROH_RELAY url {url:?}: {e}"))?;
            Endpoint::empty_builder(RelayMode::Custom(iroh::RelayMap::from(parsed)))
        }
    };
    builder
        .secret_key(generate_secret_key())
        .alpns(vec![iroh_blobs::ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| anyhow::anyhow!("bind endpoint: {e}"))
}
