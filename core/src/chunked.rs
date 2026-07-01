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

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, EndpointAddr,
};
use iroh_blobs::Hash;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::backfill::RelayRelease;
use crate::node::{decode_ticket, encode_ticket, local_addr_of};
use crate::transfer::{bind_endpoint, RelayChoice};

/// Chunk size: 16 MiB.
pub const CHUNK_SIZE: u32 = 16 * 1024 * 1024;

/// Read from `f` until `buf` is full or EOF (a single `read` may return less).
/// Returns the number of bytes filled (0 at EOF).
fn fill(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match f.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

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

// ---- chunk transfer protocol ----------------------------------------------
//
// Our own content-addressed chunk protocol (ALPN `arvolo/chunk/1`) replaces
// iroh-blobs for chunk transfer. The receiver asks for a chunk by BLAKE3 hash
// (with a byte offset for intra-chunk resume); the provider streams the chunk's
// ciphertext from that offset. The sender regenerates ciphertext ON THE FLY from
// the original file (deterministic encryption) so nothing is stored; the relay
// serves from files it holds. The receiver verifies BLAKE3(ciphertext) == hash.

/// Chunk-transfer ALPN.
pub const CHUNK_ALPN: &[u8] = b"arvolo/chunk/1";

/// Maximum ciphertext length for one chunk (plaintext chunk + AEAD tag). A
/// provider that claims more than this is rejected BEFORE we allocate/download,
/// so a malicious provider can't drive the receiver to OOM or fill the disk.
const MAX_CHUNK_CT: u64 = CHUNK_SIZE as u64 + 16;

#[derive(Serialize, Deserialize)]
struct ChunkReq {
    hash: Hash,
    /// Start streaming from this byte offset of the ciphertext (resume).
    offset: u64,
}

#[derive(Serialize, Deserialize)]
struct ChunkResp {
    /// Full ciphertext length; 0 means "not available here".
    total_len: u64,
}

async fn write_frame<T: Serialize>(send: &mut SendStream, msg: &T) -> Result<()> {
    let bytes = postcard::to_allocvec(msg).context("encode frame")?;
    send.write_all(&(bytes.len() as u32).to_le_bytes())
        .await
        .map_err(|e| anyhow!("write len: {e}"))?;
    send.write_all(&bytes)
        .await
        .map_err(|e| anyhow!("write frame: {e}"))?;
    Ok(())
}

async fn read_frame<T: serde::de::DeserializeOwned>(recv: &mut RecvStream) -> Option<T> {
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

/// A provider of chunk ciphertext: regenerated on the fly from a file (sender)
/// or read from stored files (relay).
enum ChunkBackend {
    /// Regenerate any chunk from the original plaintext file, on demand.
    OnTheFly {
        path: PathBuf,
        key: [u8; crate::crypto::CHUNK_KEY_LEN],
        index: HashMap<Hash, u32>,
        total_chunks: u32,
    },
    /// Serve stored ciphertext files, one per hash, from a directory.
    Files { dir: PathBuf },
}

impl ChunkBackend {
    /// Produce the full ciphertext for `hash`, or `None` if not available here.
    fn produce(&self, hash: &Hash) -> Option<Vec<u8>> {
        match self {
            ChunkBackend::OnTheFly {
                path,
                key,
                index,
                total_chunks,
            } => {
                let idx = *index.get(hash)?;
                let mut file = std::fs::File::open(path).ok()?;
                file.seek(SeekFrom::Start(idx as u64 * CHUNK_SIZE as u64)).ok()?;
                let mut buf = vec![0u8; CHUNK_SIZE as usize];
                let n = fill(&mut file, &mut buf).ok()?;
                let ct = crate::crypto::seal_chunk(key, idx, *total_chunks, &buf[..n]).ok()?;
                Some(ct)
            }
            ChunkBackend::Files { dir } => std::fs::read(dir.join(hash.to_string())).ok(),
        }
    }
}

/// Serves chunks over [`CHUNK_ALPN`], from either backend.
#[derive(Clone)]
pub(crate) struct ChunkServer {
    backend: Arc<ChunkBackend>,
}

impl ChunkServer {
    /// A relay-side server that serves stored ciphertext files from `dir`.
    pub(crate) fn files(dir: PathBuf) -> Self {
        Self {
            backend: Arc::new(ChunkBackend::Files { dir }),
        }
    }
}

impl std::fmt::Debug for ChunkServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChunkServer")
    }
}

impl ProtocolHandler for ChunkServer {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        // Serve one request per accepted bi-stream; a receiver may open several.
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let Some(req) = read_frame::<ChunkReq>(&mut recv).await else {
                break;
            };
            match self.backend.produce(&req.hash) {
                Some(ct) => {
                    let _ = write_frame(&mut send, &ChunkResp { total_len: ct.len() as u64 }).await;
                    let start = (req.offset as usize).min(ct.len());
                    let _ = send.write_all(&ct[start..]).await;
                }
                None => {
                    let _ = write_frame(&mut send, &ChunkResp { total_len: 0 }).await;
                }
            }
            let _ = send.finish();
        }
        Ok(())
    }
}

/// Fetch `ct[offset..]` of the chunk `hash` from one provider. Returns the full
/// ciphertext length and the received tail bytes (unverified — the caller
/// combines with any staged prefix and checks BLAKE3).
pub(crate) async fn fetch_chunk_wire(
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    hash: Hash,
    offset: u64,
) -> Result<(u64, Vec<u8>)> {
    let conn = endpoint
        .connect(addr.clone(), CHUNK_ALPN)
        .await
        .map_err(|e| anyhow!("connect chunk provider: {e}"))?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| anyhow!("open_bi: {e}"))?;
    write_frame(&mut send, &ChunkReq { hash, offset }).await?;
    send.finish().map_err(|e| anyhow!("finish req: {e}"))?;
    let resp: ChunkResp = read_frame(&mut recv)
        .await
        .ok_or_else(|| anyhow!("no chunk response"))?;
    if resp.total_len == 0 {
        anyhow::bail!("chunk not available from this provider");
    }
    anyhow::ensure!(resp.total_len <= MAX_CHUNK_CT, "provider claims oversized chunk");
    let want = resp.total_len.saturating_sub(offset) as usize;
    let mut buf = vec![0u8; want];
    recv.read_exact(&mut buf)
        .await
        .map_err(|e| anyhow!("read chunk body: {e}"))?;
    Ok((resp.total_len, buf))
}

/// Stream `ct[have..]` of chunk `hash` from one provider, appending to `out`
/// (which already holds `have` verified ciphertext bytes). Writes incrementally
/// so a partial download survives an interruption. Returns the full ciphertext
/// length.
async fn fetch_chunk_wire_to_file(
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    hash: Hash,
    out: &mut std::fs::File,
    have: u64,
) -> Result<u64> {
    use std::io::Write;
    let conn = endpoint
        .connect(addr.clone(), CHUNK_ALPN)
        .await
        .map_err(|e| anyhow!("connect chunk provider: {e}"))?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| anyhow!("open_bi: {e}"))?;
    write_frame(&mut send, &ChunkReq { hash, offset: have }).await?;
    send.finish().map_err(|e| anyhow!("finish req: {e}"))?;
    let resp: ChunkResp = read_frame(&mut recv)
        .await
        .ok_or_else(|| anyhow!("no chunk response"))?;
    if resp.total_len == 0 {
        anyhow::bail!("chunk not available from this provider");
    }
    anyhow::ensure!(resp.total_len <= MAX_CHUNK_CT, "provider claims oversized chunk");
    let mut remaining = resp.total_len.saturating_sub(have);
    let mut buf = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let want = (remaining as usize).min(buf.len());
        match recv.read(&mut buf[..want]).await {
            Ok(Some(n)) if n > 0 => {
                out.write_all(&buf[..n]).map_err(|e| anyhow!("stage chunk: {e}"))?;
                remaining -= n as u64;
            }
            _ => break,
        }
    }
    Ok(resp.total_len)
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
        // Compute the chunk hashes by streaming the file (bounded memory), WITHOUT
        // storing any ciphertext — chunks are regenerated on demand while serving.
        let total_size = std::fs::metadata(path)
            .with_context(|| format!("stat {}", path.display()))?
            .len();
        let total_chunks = (total_size as usize).div_ceil(CHUNK_SIZE as usize) as u32;
        let key = crate::crypto::random_chunk_key();
        let mut chunks = Vec::new();
        let mut index: HashMap<Hash, u32> = HashMap::new();
        {
            let mut file = std::fs::File::open(path)
                .with_context(|| format!("open {}", path.display()))?;
            let mut buf = vec![0u8; CHUNK_SIZE as usize];
            let mut idx: u32 = 0;
            loop {
                let n = fill(&mut file, &mut buf).context("read file")?;
                if n == 0 {
                    break;
                }
                let ct = crate::crypto::seal_chunk(&key, idx, total_chunks, &buf[..n])?;
                let hash = Hash::new(&ct);
                chunks.push(hash);
                index.insert(hash, idx);
                idx += 1;
            }
        }
        let chunk_server = ChunkServer {
            backend: Arc::new(ChunkBackend::OnTheFly {
                path: path.to_path_buf(),
                key,
                index,
                total_chunks,
            }),
        };

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
            .accept(CHUNK_ALPN, chunk_server)
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

    /// Fetch a whole chunk by hash, trying each provider in order, and verify
    /// `BLAKE3(ciphertext) == hash`. Bounded to one chunk in memory.
    pub async fn fetch_chunk(&self, providers: &[EndpointAddr], hash: Hash) -> Result<Vec<u8>> {
        let (ct, _rest) = self.fetch_from(providers, hash, &[]).await?;
        Ok(ct)
    }

    /// Fetch the chunk `hash`, resuming from `prefix` (already-downloaded
    /// ciphertext bytes). Returns `(full_ciphertext, newly_downloaded_tail)` once
    /// the full ciphertext is present and BLAKE3-verified. Tries providers in
    /// order for fallback.
    pub async fn fetch_from(
        &self,
        providers: &[EndpointAddr],
        hash: Hash,
        prefix: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut last_err = None;
        for addr in providers {
            match fetch_chunk_wire(&self.endpoint, addr, hash, prefix.len() as u64).await {
                Ok((total_len, tail)) => {
                    let mut ct = Vec::with_capacity(total_len as usize);
                    ct.extend_from_slice(prefix);
                    ct.extend_from_slice(&tail);
                    if ct.len() as u64 != total_len || Hash::new(&ct) != hash {
                        last_err = Some(anyhow!("chunk {hash} failed integrity check"));
                        continue;
                    }
                    return Ok((ct, tail));
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("no providers for chunk {hash}")))
    }

    /// Fetch chunk `hash` into `out`, resuming from whatever `out` already holds
    /// (intra-chunk resume). Tries providers in order; on success `out` contains
    /// the full BLAKE3-verified ciphertext. A partial download persists in `out`
    /// for a later resume.
    pub async fn fetch_to_file(
        &self,
        providers: &[EndpointAddr],
        hash: Hash,
        out: &mut std::fs::File,
    ) -> Result<()> {
        let mut last_err = None;
        for _round in 0..2 {
            for addr in providers {
                let have = out.metadata()?.len();
                out.seek(SeekFrom::Start(have))?;
                match fetch_chunk_wire_to_file(&self.endpoint, addr, hash, out, have).await {
                    Ok(total) if out.metadata()?.len() == total => {
                        out.seek(SeekFrom::Start(0))?;
                        let mut ct = Vec::with_capacity(total as usize);
                        out.read_to_end(&mut ct)?;
                        if Hash::new(&ct) == hash {
                            return Ok(());
                        }
                        out.set_len(0)?; // bad bytes: start this chunk over
                        last_err = Some(anyhow!("chunk {hash} failed integrity check"));
                    }
                    Ok(_) => last_err = Some(anyhow!("incomplete chunk {hash}")),
                    Err(e) => last_err = Some(e),
                }
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

/// How the per-transfer content key reaches the receiver.
#[derive(Clone, Serialize, Deserialize)]
pub enum KeyDelivery {
    /// In the clear — whoever holds the ticket can decrypt (ephemeral send).
    Plain(Vec<u8>),
    /// HPKE-sealed to a specific recipient and authenticated by the sender, so
    /// only that recipient decrypts and they learn who sent it (`--to`).
    Sealed {
        encapped_key: Vec<u8>,
        ciphertext: Vec<u8>,
        /// Sender's public id (for auth-mode verification on open).
        sender: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize)]
struct TicketWire {
    total_size: u64,
    chunk_size: u32,
    chunks: Vec<Hash>,
    providers: Vec<EndpointAddr>,
    relay: Option<RelayRelease>,
    /// Per-transfer content key delivery.
    key: KeyDelivery,
    /// Suggested output name (original filename, or archive/bundle name).
    name: String,
    /// The payload is a tar archive to unpack (folder / multiple files).
    archive: bool,
}

/// A chunked transfer ticket (`arvc…`). Carries the content key delivery — either
/// in the clear (anyone with the ticket) or sealed to a specific recipient.
pub struct ChunkTicket {
    pub total_size: u64,
    pub chunk_size: u32,
    pub chunks: Vec<Hash>,
    pub providers: Vec<EndpointAddr>,
    pub relay: Option<RelayRelease>,
    pub key: KeyDelivery,
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
