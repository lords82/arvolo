//! Chunked transfer with incremental relay cleanup.
//!
//! A file is split into fixed-size chunks, each an independent content-addressed
//! blob. The sender serves all chunks (and can seed them to a relay). The
//! receiver fetches chunks one by one — from the sender or, as a fallback, the
//! relay — and **releases each chunk on the relay as soon as it has it**, so the
//! relay frees storage *during* the download instead of only at the end.
//!
//! Resume is driven by the output file length (whole chunks already written).

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use iroh::{
    endpoint::{Connection, SendStream},
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, EndpointAddr,
};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket, BlobFormat, BlobsProtocol, Hash};
use serde::{Deserialize, Serialize};

use crate::backfill::RelayRelease;
use crate::node::{decode_ticket, encode_ticket};
use crate::transfer::{bind_endpoint, fetch_into, RelayChoice};

/// Chunk size: 16 MiB. Large enough to keep manifests small, small enough that
/// incremental relay cleanup is meaningful for big files.
pub const CHUNK_SIZE: u32 = 16 * 1024 * 1024;

/// Control-channel ALPN: the receiver tells the sender which chunks it has, so
/// the sender knows what's been delivered (for lazy/tail backfill).
pub const CTRL_ALPN: &[u8] = b"arvolo/ctrl/1";

/// Sender-side control handler: records the chunk indices the receiver acks.
#[derive(Debug, Clone)]
struct CtrlHandler {
    delivered: Arc<Mutex<HashSet<u32>>>,
}

impl ProtocolHandler for CtrlHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        // The receiver opens a uni stream and writes one little-endian u32 chunk
        // index per chunk it has received.
        if let Ok(mut recv) = conn.accept_uni().await {
            let mut buf = [0u8; 4];
            while recv.read_exact(&mut buf).await.is_ok() {
                self.delivered
                    .lock()
                    .unwrap()
                    .insert(u32::from_le_bytes(buf));
            }
        }
        Ok(())
    }
}

/// The receiver's end of the control channel: acks chunks to the sender.
pub struct Control {
    send: SendStream,
    _conn: Connection,
}

impl Control {
    /// Tell the sender we have chunk `idx`.
    pub async fn ack(&mut self, idx: u32) -> Result<()> {
        self.send
            .write_all(&idx.to_le_bytes())
            .await
            .map_err(|e| anyhow!("ctrl ack: {e}"))
    }

    /// Finish acking: send the stream FIN and wait for the sender to drain it
    /// (so the last acks aren't lost to an abrupt connection close).
    pub async fn finish(mut self) -> Result<()> {
        self.send
            .finish()
            .map_err(|e| anyhow!("ctrl finish: {e}"))?;
        // The sender reads to EOF then drops the connection; wait for that (the
        // connection stays open meanwhile, draining the acks). Bounded so we
        // never hang if the sender lingers.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), self._conn.closed()).await;
        Ok(())
    }
}

const TICKET_PREFIX: &str = "arvc";

#[derive(Serialize, Deserialize)]
struct TicketWire {
    total_size: u64,
    chunk_size: u32,
    chunks: Vec<Hash>,
    providers: Vec<EndpointAddr>,
    #[serde(default)]
    relay: Option<RelayRelease>,
}

/// A chunked transfer ticket (`arvc…`): the ordered chunk hashes, where to get
/// them, and how to release relay copies.
pub struct ChunkTicket {
    pub total_size: u64,
    pub chunk_size: u32,
    pub chunks: Vec<Hash>,
    pub providers: Vec<EndpointAddr>,
    pub relay: Option<RelayRelease>,
}

impl ChunkTicket {
    pub fn encode(&self) -> Result<String> {
        let bytes = postcard::to_allocvec(&TicketWire {
            total_size: self.total_size,
            chunk_size: self.chunk_size,
            chunks: self.chunks.clone(),
            providers: self.providers.clone(),
            relay: self.relay.clone(),
        })
        .context("serialize chunk ticket")?;
        Ok(format!(
            "{TICKET_PREFIX}{}",
            data_encoding::BASE32_NOPAD.encode(&bytes)
        ))
    }

    pub fn decode(s: &str) -> Result<Self> {
        let body = s
            .trim()
            .strip_prefix(TICKET_PREFIX)
            .ok_or_else(|| anyhow!("not a chunk ticket (missing {TICKET_PREFIX} prefix)"))?;
        let bytes = data_encoding::BASE32_NOPAD
            .decode(body.to_uppercase().as_bytes())
            .context("decode chunk ticket")?;
        let w: TicketWire = postcard::from_bytes(&bytes).context("deserialize chunk ticket")?;
        Ok(Self {
            total_size: w.total_size,
            chunk_size: w.chunk_size,
            chunks: w.chunks,
            providers: w.providers,
            relay: w.relay,
        })
    }

    pub fn looks_like(s: &str) -> bool {
        s.trim_start().starts_with(TICKET_PREFIX)
    }
}

/// What the sender hands the relay so it can seed the chunks: the sender's
/// address and the list of chunk hashes. Base32 `arvs…`.
#[derive(Serialize, Deserialize)]
pub struct SeedRequest {
    pub sender: EndpointAddr,
    pub chunks: Vec<Hash>,
}

impl SeedRequest {
    pub fn encode(&self) -> Result<String> {
        let bytes = postcard::to_allocvec(self).context("serialize seed request")?;
        Ok(format!(
            "arvs{}",
            data_encoding::BASE32_NOPAD.encode(&bytes)
        ))
    }
    pub fn decode(s: &str) -> Result<Self> {
        let body = s
            .trim()
            .strip_prefix("arvs")
            .ok_or_else(|| anyhow!("not a seed request"))?;
        let bytes = data_encoding::BASE32_NOPAD
            .decode(body.to_uppercase().as_bytes())
            .context("decode seed request")?;
        postcard::from_bytes(&bytes).context("deserialize seed request")
    }
}

/// A running sender: serves every chunk of a file as its own blob, and tracks
/// which chunks the receiver has acked over the control channel.
pub struct ChunkSender {
    _router: Router,
    _store: MemStore,
    endpoint: Endpoint,
    addr: EndpointAddr,
    chunks: Vec<Hash>,
    total_size: u64,
    delivered: Arc<Mutex<HashSet<u32>>>,
}

impl ChunkSender {
    /// Split `path` into chunks, add each as a blob, and serve them.
    pub async fn serve(path: &Path, relay: RelayChoice) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let total_size = data.len() as u64;
        let store = MemStore::new();
        let mut chunks = Vec::new();
        for chunk in data.chunks(CHUNK_SIZE as usize) {
            let tag = store
                .blobs()
                .add_bytes(chunk.to_vec())
                .await
                .context("add chunk")?;
            chunks.push(tag.hash);
        }
        let blobs = BlobsProtocol::new(&store, None);
        let use_relay = !matches!(relay, RelayChoice::Disabled);
        let endpoint = bind_endpoint(relay).await?;
        let delivered = Arc::new(Mutex::new(HashSet::new()));
        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs)
            .accept(
                CTRL_ALPN,
                CtrlHandler {
                    delivered: delivered.clone(),
                },
            )
            .spawn();
        // online() waits for a relay; with the relay disabled (LAN/tests) there
        // is none, so use the direct (loopback/LAN) address instead.
        let addr = if use_relay {
            endpoint.online().await;
            endpoint.addr()
        } else {
            crate::node::local_addr_of(&endpoint)
        };
        Ok(Self {
            _router: router,
            _store: store,
            endpoint,
            addr,
            chunks,
            total_size,
            delivered,
        })
    }

    /// Number of chunks the receiver has acknowledged receiving.
    pub fn delivered_count(&self) -> usize {
        self.delivered.lock().unwrap().len()
    }

    /// The set of chunk indices the receiver has acknowledged.
    pub fn delivered(&self) -> HashSet<u32> {
        self.delivered.lock().unwrap().clone()
    }

    pub fn addr(&self) -> EndpointAddr {
        self.addr.clone()
    }
    pub fn chunks(&self) -> &[Hash] {
        &self.chunks
    }
    pub fn total_size(&self) -> u64 {
        self.total_size
    }
    pub fn chunk_size(&self) -> u32 {
        CHUNK_SIZE
    }
    pub fn seed_request(&self) -> SeedRequest {
        SeedRequest {
            sender: self.addr(),
            chunks: self.chunks.clone(),
        }
    }
    pub async fn shutdown(self) {
        self.endpoint.close().await;
    }
}

/// A receiver endpoint that fetches chunks (with provider fallback).
pub struct ChunkReceiver {
    endpoint: Endpoint,
}

impl ChunkReceiver {
    pub async fn open(relay: RelayChoice) -> Result<Self> {
        Ok(Self {
            endpoint: bind_endpoint(relay).await?,
        })
    }

    /// Open a control channel to the sender, to ack chunks as we receive them.
    pub async fn open_control(&self, sender: &EndpointAddr) -> Result<Control> {
        let conn = self
            .endpoint
            .connect(sender.clone(), CTRL_ALPN)
            .await
            .map_err(|e| anyhow!("control connect: {e}"))?;
        let send = conn
            .open_uni()
            .await
            .map_err(|e| anyhow!("control stream: {e}"))?;
        Ok(Control { send, _conn: conn })
    }

    /// Fetch a single chunk by hash, trying each provider in turn. Returns the
    /// chunk bytes. Memory is bounded to one chunk (transient store).
    pub async fn fetch_chunk(&self, providers: &[EndpointAddr], hash: Hash) -> Result<Vec<u8>> {
        let store = MemStore::new();
        let mut last_err = None;
        for addr in providers {
            let bt = BlobTicket::new(addr.clone(), hash, BlobFormat::Raw);
            match fetch_into(&store, &self.endpoint, &bt).await {
                Ok(_) => {
                    let bytes = store.blobs().get_bytes(hash).await.context("read chunk")?;
                    return Ok(bytes.to_vec());
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("no providers for chunk {hash}")))
    }

    pub async fn close(self) {
        self.endpoint.close().await;
    }
}

/// Encode a relay provider address (for advertising the relay as a provider).
pub fn encode_addr(addr: &EndpointAddr) -> Result<String> {
    encode_ticket(addr)
}

/// Decode a relay provider address.
pub fn decode_addr(s: &str) -> Result<EndpointAddr> {
    decode_ticket(s)
}
