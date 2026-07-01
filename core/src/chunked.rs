//! Chunked transfer with lazy tail-backfill and anti-double-send.
//!
//! A file is split into fixed-size content-addressed chunks. The sender serves
//! them P2P. A bidirectional **control channel** lets the receiver ack chunks
//! it has (`Have`) and lets the sender tell the receiver which chunks it pushed
//! to a relay (`RelayHas`).
//!
//! Orchestration (driven by the CLI):
//! - Receiver pulls chunks directly from the sender (P2P).
//! - If the receiver drops, the sender backfills **only the undelivered chunks**
//!   to the relay (the CLI does the HTTP call when [`ChunkSender::receiver_gone`]
//!   fires) and calls [`ChunkSender::mark_on_relay`].
//! - When the receiver returns, the sender advertises the on-relay chunks; the
//!   receiver pulls **those from the relay** and the rest from the sender
//!   (anti-double-send), and releases each relay chunk as it gets it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, EndpointAddr,
};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket, BlobFormat, BlobsProtocol, Hash};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::backfill::RelayRelease;
use crate::node::{decode_ticket, encode_ticket, local_addr_of};
use crate::transfer::{bind_endpoint, fetch_into, RelayChoice};

/// Chunk size: 16 MiB.
pub const CHUNK_SIZE: u32 = 16 * 1024 * 1024;

/// Control-channel ALPN.
pub const CTRL_ALPN: &[u8] = b"arvolo/ctrl/2";

// ---- control messages -----------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
enum CtrlMsg {
    /// Receiver opens the channel.
    Hello,
    /// Receiver liveness heartbeat.
    Ping,
    /// Receiver has chunk `idx`.
    Have(u32),
    /// Sender: these chunk indices are now available on the relay.
    RelayHas(Vec<u32>),
}

/// If the sender hears nothing on the control channel for this long, it treats
/// the receiver as gone (covers abrupt crashes that don't close cleanly).
const CTRL_IDLE_SECS: u64 = 6;
const CTRL_HEARTBEAT_SECS: u64 = 2;

async fn write_msg(send: &mut SendStream, msg: &CtrlMsg) -> Result<()> {
    let bytes = postcard::to_allocvec(msg).context("encode ctrl msg")?;
    let len = bytes.len() as u32;
    send.write_all(&len.to_le_bytes())
        .await
        .map_err(|e| anyhow!("ctrl write len: {e}"))?;
    send.write_all(&bytes)
        .await
        .map_err(|e| anyhow!("ctrl write: {e}"))?;
    Ok(())
}

async fn read_msg(recv: &mut RecvStream) -> Option<CtrlMsg> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await.ok()?;
    let len = u32::from_le_bytes(len) as usize;
    if len > 64 * 1024 {
        return None;
    }
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await.ok()?;
    postcard::from_bytes(&buf).ok()
}

// ---- sender ---------------------------------------------------------------

#[derive(Debug, Clone)]
struct CtrlHandler {
    total: usize,
    delivered: Arc<Mutex<HashSet<u32>>>,
    on_relay: Arc<Mutex<HashSet<u32>>>,
    gone_tx: mpsc::UnboundedSender<Vec<usize>>,
}

impl ProtocolHandler for CtrlHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let dbg = std::env::var("ARVOLO_DEBUG").is_ok();
        let Ok((mut send, mut recv)) = conn.accept_bi().await else {
            if dbg {
                eprintln!("[ctrl] accept_bi failed");
            }
            return Ok(());
        };
        if dbg {
            eprintln!("[ctrl] receiver connected");
        }
        // On connect, tell the receiver which chunks are already on the relay.
        let snapshot: Vec<u32> = self.on_relay.lock().unwrap().iter().copied().collect();
        if !snapshot.is_empty() {
            let _ = write_msg(&mut send, &CtrlMsg::RelayHas(snapshot)).await;
        }
        let _ = send.finish();
        // Read Have acks (and Pings) until the receiver disconnects or goes
        // silent for CTRL_IDLE_SECS (abrupt crash).
        let reason = loop {
            match tokio::time::timeout(
                std::time::Duration::from_secs(CTRL_IDLE_SECS),
                read_msg(&mut recv),
            )
            .await
            {
                Ok(Some(CtrlMsg::Have(idx))) => {
                    self.delivered.lock().unwrap().insert(idx);
                }
                Ok(Some(_)) => {}        // Ping/Hello: keepalive
                Ok(None) => break "eof", // closed cleanly
                Err(_) => break "idle",  // idle: receiver gone
            }
        };
        // Receiver gone: report the still-undelivered chunk indices.
        let delivered = self.delivered.lock().unwrap();
        let undelivered: Vec<usize> = (0..self.total)
            .filter(|i| !delivered.contains(&(*i as u32)))
            .collect();
        if dbg {
            eprintln!(
                "[ctrl] receiver gone ({reason}); delivered={} undelivered={}",
                delivered.len(),
                undelivered.len()
            );
        }
        drop(delivered);
        let _ = self.gone_tx.send(undelivered);
        Ok(())
    }
}

/// A running sender: serves every chunk and orchestrates lazy relay backfill.
pub struct ChunkSender {
    _router: Router,
    _store: MemStore,
    endpoint: Endpoint,
    addr: EndpointAddr,
    chunks: Vec<Hash>,
    total_size: u64,
    key: [u8; crate::crypto::CHUNK_KEY_LEN],
    delivered: Arc<Mutex<HashSet<u32>>>,
    on_relay: Arc<Mutex<HashSet<u32>>>,
    gone_rx: AsyncMutex<mpsc::UnboundedReceiver<Vec<usize>>>,
}

impl ChunkSender {
    pub async fn serve(path: &Path, relay: RelayChoice) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let total_size = data.len() as u64;
        let store = MemStore::new();
        // Encrypt every chunk under a per-transfer random content key before it
        // enters the blob store, so the relay (and any other holder of a chunk)
        // only ever sees ciphertext. The key travels in the ticket, out-of-band.
        let key = crate::crypto::random_chunk_key();
        let total_chunks = data.len().div_ceil(CHUNK_SIZE as usize) as u32;
        let mut chunks = Vec::new();
        for (idx, chunk) in data.chunks(CHUNK_SIZE as usize).enumerate() {
            let ct = crate::crypto::seal_chunk(&key, idx as u32, total_chunks, chunk)?;
            let tag = store.blobs().add_bytes(ct).await.context("add chunk")?;
            chunks.push(tag.hash);
        }
        let blobs = BlobsProtocol::new(&store, None);
        let use_relay = !matches!(relay, RelayChoice::Disabled);
        let endpoint = bind_endpoint(relay).await?;

        let delivered = Arc::new(Mutex::new(HashSet::new()));
        let on_relay = Arc::new(Mutex::new(HashSet::new()));
        let (gone_tx, gone_rx) = mpsc::unbounded_channel();
        let handler = CtrlHandler {
            total: chunks.len(),
            delivered: delivered.clone(),
            on_relay: on_relay.clone(),
            gone_tx,
        };
        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs)
            .accept(CTRL_ALPN, handler)
            .spawn();
        let addr = if use_relay {
            endpoint.online().await;
            endpoint.addr()
        } else {
            local_addr_of(&endpoint)
        };
        Ok(Self {
            _router: router,
            _store: store,
            endpoint,
            addr,
            chunks,
            total_size,
            key,
            delivered,
            on_relay,
            gone_rx: AsyncMutex::new(gone_rx),
        })
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
    /// The per-transfer content key; the CLI puts this in the ticket so the
    /// receiver can decrypt. Whoever holds the ticket can decrypt.
    pub fn key(&self) -> [u8; crate::crypto::CHUNK_KEY_LEN] {
        self.key
    }
    pub fn delivered_count(&self) -> usize {
        self.delivered.lock().unwrap().len()
    }

    /// Resolves when a connected receiver disconnects, yielding the chunk
    /// indices not yet delivered (the tail to backfill). Never fires if no
    /// receiver has connected.
    pub async fn receiver_gone(&self) -> Vec<usize> {
        let mut rx = self.gone_rx.lock().await;
        rx.recv().await.unwrap_or_default()
    }

    /// Record that `indices` are now on the relay (advertised to receivers on
    /// their next control connection).
    pub fn mark_on_relay(&self, indices: &[usize]) {
        let mut set = self.on_relay.lock().unwrap();
        for &i in indices {
            set.insert(i as u32);
        }
    }

    pub async fn shutdown(self) {
        self.endpoint.close().await;
    }
}

// ---- receiver -------------------------------------------------------------

/// The receiver's control channel: acks chunks (and heartbeats); learns which
/// chunks are on the relay. Dropping it closes the connection so the sender
/// promptly detects the receiver is gone.
pub struct Control {
    send: Arc<AsyncMutex<SendStream>>,
    heartbeat: tokio::task::JoinHandle<()>,
    reader: tokio::task::JoinHandle<()>,
    conn: Connection,
}

impl Control {
    pub async fn ack(&mut self, idx: u32) -> Result<()> {
        let mut send = self.send.lock().await;
        write_msg(&mut send, &CtrlMsg::Have(idx)).await
    }
    pub async fn finish(self) -> Result<()> {
        self.heartbeat.abort();
        {
            let mut send = self.send.lock().await;
            send.finish().map_err(|e| anyhow!("ctrl finish: {e}"))?;
        }
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), self.conn.closed()).await;
        Ok(())
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.heartbeat.abort();
        self.reader.abort();
        self.conn.close(0u32.into(), b"bye");
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

    /// Open the control channel to the sender. RelayHas updates from the sender
    /// are written into `on_relay`. Returns the ack side, or None if the sender
    /// is unreachable.
    pub async fn open_control(
        &self,
        sender: &EndpointAddr,
        on_relay: Arc<Mutex<HashSet<u32>>>,
    ) -> Option<Control> {
        let conn = self
            .endpoint
            .connect(sender.clone(), CTRL_ALPN)
            .await
            .ok()?;
        let (mut send, mut recv) = conn.open_bi().await.ok()?;
        // Open the stream on the wire so the sender's accept_bi returns.
        write_msg(&mut send, &CtrlMsg::Hello).await.ok()?;
        let send = Arc::new(AsyncMutex::new(send));
        // Read RelayHas updates from the sender.
        let reader = tokio::spawn(async move {
            while let Some(msg) = read_msg(&mut recv).await {
                if let CtrlMsg::RelayHas(indices) = msg {
                    let mut set = on_relay.lock().unwrap();
                    for i in indices {
                        set.insert(i);
                    }
                }
            }
        });
        // Heartbeat so the sender can tell we're alive (vs. an abrupt crash).
        let hb_send = send.clone();
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(CTRL_HEARTBEAT_SECS)).await;
                let mut s = hb_send.lock().await;
                if write_msg(&mut s, &CtrlMsg::Ping).await.is_err() {
                    break;
                }
            }
        });
        Some(Control {
            send,
            heartbeat,
            reader,
            conn,
        })
    }

    /// Fetch a single chunk by hash, trying each provider in order. Bounded to
    /// one chunk in memory.
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

// ---- tickets --------------------------------------------------------------

const TICKET_PREFIX: &str = "arvc";

#[derive(Serialize, Deserialize)]
struct TicketWire {
    total_size: u64,
    chunk_size: u32,
    chunks: Vec<Hash>,
    providers: Vec<EndpointAddr>,
    relay: Option<RelayRelease>,
    /// Per-transfer content key to decrypt the chunks (32 bytes).
    key: Vec<u8>,
    /// Suggested output name (original filename, or archive/bundle name).
    name: String,
    /// The payload is a tar archive to unpack (folder / multiple files).
    archive: bool,
}

/// A chunked transfer ticket (`arvc…`). Carries the content key that decrypts
/// the chunks — whoever holds the ticket can receive and decrypt.
pub struct ChunkTicket {
    pub total_size: u64,
    pub chunk_size: u32,
    pub chunks: Vec<Hash>,
    pub providers: Vec<EndpointAddr>,
    pub relay: Option<RelayRelease>,
    pub key: Vec<u8>,
    /// Suggested output name (original filename, or archive/bundle name).
    pub name: String,
    /// The payload is a tar archive to unpack (folder / multiple files).
    pub archive: bool,
}

impl ChunkTicket {
    pub fn encode(&self) -> Result<String> {
        let bytes = postcard::to_allocvec(&TicketWire {
            total_size: self.total_size,
            chunk_size: self.chunk_size,
            chunks: self.chunks.clone(),
            providers: self.providers.clone(),
            relay: self.relay.clone(),
            key: self.key.clone(),
            name: self.name.clone(),
            archive: self.archive,
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
            key: w.key,
            name: w.name,
            archive: w.archive,
        })
    }

    pub fn looks_like(s: &str) -> bool {
        s.trim_start().starts_with(TICKET_PREFIX)
    }
}

/// What the sender hands the relay to backfill chunks: the sender's address, the
/// chunk hashes to fetch, and the transfer token. Base32 `arvs…`.
#[derive(Serialize, Deserialize)]
pub struct SeedRequest {
    pub sender: EndpointAddr,
    pub chunks: Vec<Hash>,
    pub token: String,
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

/// Encode/decode a relay provider address (its iroh address).
pub fn encode_addr(addr: &EndpointAddr) -> Result<String> {
    encode_ticket(addr)
}
pub fn decode_addr(s: &str) -> Result<EndpointAddr> {
    decode_ticket(s)
}
