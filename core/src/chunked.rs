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
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::hash::Hash;
use anyhow::{anyhow, Context, Result};
use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, EndpointAddr, EndpointId,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::backfill::RelayRelease;
use crate::node::{decode_ticket, encode_ticket, local_addr_of, remote_addr_of};
use crate::source::SendSource;
use crate::transfer::{bind_endpoint, bind_endpoint_with_key, RelayChoice};

/// Chunk size: 16 MiB.
pub const CHUNK_SIZE: u32 = 16 * 1024 * 1024;

/// Read from `f` until `buf` is full or EOF (a single `read` may return less).
/// Returns the number of bytes filled (0 at EOF).
fn fill(f: &mut impl std::io::Read, buf: &mut [u8]) -> std::io::Result<usize> {
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
    /// Receiver is cancelling ON PURPOSE — not crashing, not pausing. The sender
    /// must NOT keep the tail warm for it: no relay backfill, no re-offer; the
    /// send ends. Appended last so an old sender, whose postcard decode fails on
    /// the unknown index, reads it as EOF — today's "receiver gone" behavior.
    Abort,
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

/// How long a chunk fetch may go without receiving a single byte before it is
/// treated as dead.
///
/// Not a deadline on the fetch: a chunk on a slow link can legitimately take
/// minutes, and cutting it off mid-flow would turn a slow provider into a failing
/// one. This bounds *silence*, which is a different thing and always a fault.
///
/// A fetch had no bound at all before this, and a provider that stopped sending
/// without closing hung the task holding it — forever, since QUIC keeps a
/// connection alive as long as the peer answers at the transport level. That is
/// worse than a failure, because a failure is something the scheduler knows what to
/// do with: it cools the provider down, re-queues the piece and reassigns it within
/// seconds. Silence reaches none of that machinery. Observed: a 10.7 GiB download
/// stopped at 97.4% and sat there for hours, no progress and no error, with the
/// sender still counting a connected peer.
///
/// Thirty seconds is long enough to be unambiguous — no healthy transfer pauses
/// that long between packets — and short against the cost of being wrong, which is
/// one re-fetched chunk from another source.
const CHUNK_STALL_SECS: u64 = 30;

/// How long a fetch may spend *reaching* a provider before treating it as
/// unreachable.
///
/// Deliberately not [`CHUNK_STALL_SECS`]: "I cannot reach you" and "you stopped
/// talking to me half way" are different faults with different honest durations.
/// Opening a chunk stream includes the connect, and so discovery and hole punching
/// — which the delivery loop budgets `LIVE_CONNECT_SECS` (90s) for, calling the
/// cross-internet cold start "highly variable". Bounding that at thirty seconds
/// would turn a perfectly good provider behind an awkward NAT into a failing one,
/// and where there is only one provider — the ordinary `send --to` — into a loop of
/// thirty-second attempts in place of the wait that would have worked.
const CHUNK_OPEN_SECS: u64 = 90;

/// How long a chunk connection that has *already served a request* may go quiet
/// before the sender hangs up on it.
///
/// The short bound belongs to a connection that has never asked for anything: that
/// one has shown no purpose, and it is where the hang seen in the wild lives.
/// Applying the same impatience afterwards would disconnect peers doing nothing
/// wrong — a swarm seeder between requests, a provider the receiver's rarest-first
/// ordering is not favouring at the moment — and charge them a fresh handshake for
/// it. This is long enough that idling is not punished, and still an answer to
/// "when does a connection nobody uses go away", which before was: never.
const CHUNK_IDLE_SECS: u64 = 5 * 60;

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
    /// Regenerate any chunk from the original plaintext source, on demand —
    /// positional reads, so a handed-off descriptor serves as well as a path.
    OnTheFly {
        source: SendSource,
        key: [u8; crate::crypto::CHUNK_KEY_LEN],
        index: HashMap<Hash, u32>,
        total_chunks: u32,
    },
    /// Serve stored ciphertext files, one per hash, from a directory.
    Files { dir: PathBuf },
    /// A receiver re-seeding: regenerate a chunk it has verified by re-sealing it
    /// from its own (plaintext) output file — deterministic sealing reproduces the
    /// sender's exact ciphertext/hash. `have` is a **bitfield of verified pieces**
    /// (arbitrary/disjoint, not just a prefix): only pieces whose bit is set are on
    /// disk and may be served.
    Reseal {
        path: PathBuf,
        key: [u8; crate::crypto::CHUNK_KEY_LEN],
        index: HashMap<Hash, u32>,
        total_chunks: u32,
        have: HaveBitfield,
    },
}

/// Shared bitfield of the pieces a receiver has verified and written to its output
/// file. Set bit `i` ⇒ piece `i` is on disk at offset `i*CHUNK_SIZE` and can be
/// served. Written by the receiver's commit path, read by the seeder and the swarm
/// announce. Using a bitfield (vs a prefix count) is what allows disjoint pieces.
pub type HaveBitfield = Arc<std::sync::Mutex<Vec<u8>>>;

impl ChunkBackend {
    /// Produce the full ciphertext for `hash`, or `None` if not available here.
    fn produce(&self, hash: &Hash) -> Option<Vec<u8>> {
        match self {
            ChunkBackend::OnTheFly {
                source,
                key,
                index,
                total_chunks,
            } => {
                let idx = *index.get(hash)?;
                let mut buf = vec![0u8; CHUNK_SIZE as usize];
                let n = source
                    .read_at_full(&mut buf, idx as u64 * CHUNK_SIZE as u64)
                    .ok()?;
                let ct = crate::crypto::seal_chunk(key, idx, *total_chunks, &buf[..n]).ok()?;
                Some(ct)
            }
            ChunkBackend::Files { dir } => std::fs::read(dir.join(hash.to_string())).ok(),
            ChunkBackend::Reseal {
                path,
                key,
                index,
                total_chunks,
                have,
            } => {
                let idx = *index.get(hash)?;
                if !crate::swarm::bitfield_has(&have.lock().unwrap(), idx as usize) {
                    return None; // we haven't verified/committed this piece yet
                }
                let mut file = std::fs::File::open(path).ok()?;
                file.seek(SeekFrom::Start(idx as u64 * CHUNK_SIZE as u64))
                    .ok()?;
                let mut buf = vec![0u8; CHUNK_SIZE as usize];
                let n = fill(&mut file, &mut buf).ok()?;
                let ct = crate::crypto::seal_chunk(key, idx, *total_chunks, &buf[..n]).ok()?;
                Some(ct)
            }
        }
    }
}

/// Live count of *distinct* peers currently connected to a [`ChunkServer`] —
/// i.e. how many are downloading from us right now. Keyed by remote endpoint id
/// so repeated connects from the same peer count once. Cheap `Arc<Mutex<…>>`
/// clone shared between the server and its [`ChunkSender`].
#[derive(Clone, Default)]
pub(crate) struct PeerCount(Arc<Mutex<HashMap<EndpointId, usize>>>);

impl PeerCount {
    fn enter(&self, id: EndpointId) {
        *self.0.lock().unwrap().entry(id).or_insert(0) += 1;
    }
    fn leave(&self, id: EndpointId) {
        let mut m = self.0.lock().unwrap();
        if let Some(c) = m.get_mut(&id) {
            *c -= 1;
            if *c == 0 {
                m.remove(&id);
            }
        }
    }
    /// Number of distinct peers currently connected.
    pub(crate) fn distinct(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

/// Decrements the peer count when an accepted connection ends (drop-safe).
struct PeerGuard(PeerCount, EndpointId);
impl Drop for PeerGuard {
    fn drop(&mut self) {
        self.0.leave(self.1);
    }
}

/// Running total of chunk-body bytes this server has handed to QUIC.
///
/// The sender's only other progress signal is the receiver's chunk acks, which
/// arrive one whole 16 MiB piece at a time — and, since the receiver pulls
/// several pieces at once, the *first* of them lands only after four chunks'
/// worth of upload. On a home uplink that's a minute of a transfer that is
/// visibly doing nothing. This counter moves continuously instead.
///
/// It counts bytes accepted by the send stream, not bytes acknowledged by the
/// peer, so it leads the truth — but only by what QUIC flow control allows to be
/// in flight (a stream receive window per stream, the connection send window
/// overall: single-digit MB), against a 64 MiB blind spot. It is a *progress*
/// signal only: what counts as delivered stays the acks, and callers must still
/// clamp it, since a re-fetched piece (a failed fetch, an endgame duplicate)
/// sends the same bytes twice.
///
/// Counted **per receiver**. A shared ticket is served to several at once, and one
/// running total across all of them answers a question nobody asked: it reaches
/// the payload size when the *sum* of everyone's downloads does, which with two
/// receivers is halfway through the job. Per peer, [`SentBytes::best`] is the
/// furthest-along receiver — the only reading of "how far along is this send" that
/// is true with one receiver and still true with five.
#[derive(Clone, Default)]
pub(crate) struct SentBytes(Arc<Mutex<HashMap<EndpointId, Arc<AtomicU64>>>>);

impl SentBytes {
    /// This peer's counter, taken once per connection so the write loop pays an
    /// atomic add and not a map lookup.
    fn counter(&self, peer: EndpointId) -> Arc<AtomicU64> {
        self.0.lock().unwrap().entry(peer).or_default().clone()
    }
    /// Bytes taken by whichever receiver has taken the most.
    fn best(&self) -> u64 {
        self.0
            .lock()
            .unwrap()
            .values()
            .map(|c| c.load(Ordering::Relaxed))
            .max()
            .unwrap_or(0)
    }
}

/// Serves chunks over [`CHUNK_ALPN`], from either backend.
#[derive(Clone)]
pub(crate) struct ChunkServer {
    backend: Arc<ChunkBackend>,
    /// Distinct peers currently fetching from this server (see [`PeerCount`]).
    peers: PeerCount,
    /// Chunk-body bytes pushed out so far (see [`SentBytes`]).
    sent: SentBytes,
}

impl ChunkServer {
    /// A relay-side server that serves stored ciphertext files from `dir`.
    pub(crate) fn files(dir: PathBuf) -> Self {
        Self {
            backend: Arc::new(ChunkBackend::Files { dir }),
            peers: PeerCount::default(),
            sent: SentBytes::default(),
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
        // Track this peer as an active downloader for the duration of the
        // connection (dropped when it ends, even on error).
        let peer = conn.remote_id();
        self.peers.enter(peer);
        let _guard = PeerGuard(self.peers.clone(), peer);
        // Taken once for the connection: the write loop below then costs one atomic
        // add per flow-control window, exactly as it did when the counter was global.
        let sent = self.sent.counter(peer);
        // Serve one request per accepted bi-stream; a receiver may open several.
        // Every wait here is bounded, and it is the same bug the receiver's body read
        // had, seen from this end: a peer that is present and silent used to hold this
        // loop for ever, and with it a connection that goes on counting as an active
        // downloader — the sender reporting someone downloading who will never ask for
        // anything and never leave.
        //
        // The transport does not save us. Keep-alives run every second, so a
        // connection with no application data on it is never *idle* by QUIC's
        // reckoning: it answers at the transport level for as long as both processes
        // live. Only this loop can decide that nothing is being asked.
        //
        // Bounding the accept, not just the read, is what actually closes it: a stream
        // opened and never written to is invisible here — QUIC does not announce it to
        // the peer until the first byte — so a receiver that opens one and says
        // nothing leaves this waiting on `accept_bi`, never reaching the read at all.
        //
        // The first request and the ones after it get different patience, because
        // they are different situations. A connection that has never asked for
        // anything has not yet shown a purpose, and the one case seen in the wild sits
        // exactly here. A connection that has already served is entitled to a quiet
        // spell: a swarm seeder (`ARVOLO_SEED_AFTER`) or a provider the receiver's
        // rarest-first ordering is not favouring right now can easily go longer than
        // that between requests, and hanging up on it costs a fresh handshake every
        // time. Within a running download the gaps are nothing like this — the
        // receiver's own provider cooldown is three seconds — so the long branch only
        // ever covers idling, never transferring.
        let mut has_asked = false;
        loop {
            let patience = if has_asked {
                idle_after()
            } else {
                stall_after()
            };
            let accepted = match tokio::time::timeout(patience, conn.accept_bi()).await {
                Ok(Ok(pair)) => pair,
                Ok(Err(_)) => break,
                Err(_) => {
                    tracing::debug!(
                        "chunk: connection idle for {}s with nothing asked; closing it",
                        patience.as_secs()
                    );
                    break;
                }
            };
            let (mut send, mut recv) = accepted;
            let req = match tokio::time::timeout(stall_after(), read_frame::<ChunkReq>(&mut recv))
                .await
            {
                Ok(Some(req)) => req,
                Ok(None) => break,
                Err(_) => {
                    tracing::debug!("chunk: a receiver opened a stream and then asked nothing");
                    break;
                }
            };
            // It has asked for something: from here the connection has a purpose, and
            // a quiet spell on it is idling rather than the shape of the hang.
            has_asked = true;
            // Producing a chunk opens the file and re-encrypts 16 MiB (CPU-bound).
            // Run it on the blocking pool so concurrent requests (parallel receiver
            // fetches) don't stall the async runtime's worker threads.
            let backend = self.backend.clone();
            let hash = req.hash;
            let produced = tokio::task::spawn_blocking(move || backend.produce(&hash))
                .await
                .ok()
                .flatten();
            match produced {
                Some(ct) => {
                    let _ = write_frame(
                        &mut send,
                        &ChunkResp {
                            total_len: ct.len() as u64,
                        },
                    )
                    .await;
                    // `write_all` spelled out, so the bytes QUIC accepts can be
                    // counted as they go: it is exactly this loop internally
                    // (`write` returns what the flow-control window took), so
                    // the only addition is one relaxed atomic per iteration.
                    let start = (req.offset as usize).min(ct.len());
                    let mut body = &ct[start..];
                    while !body.is_empty() {
                        let Ok(n) = send.write(body).await else { break };
                        body = &body[n..];
                        sent.fetch_add(n as u64, Ordering::Relaxed);
                    }
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

/// Render an error together with its source chain.
///
/// quinn's `ReadError::ConnectionLost` Displays as the bare words "connection
/// lost" and puts *why* — timed out, reset, closed by the peer, transport error
/// — in its `source()`. A transfer tool that swallows that is a transfer tool
/// nobody can debug: every network fault reads the same in the log and in the
/// UI. Walk the chain so the reason survives.
fn with_causes(e: impl std::error::Error) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        let text = s.to_string();
        // Some layers repeat the parent's message verbatim; don't stutter.
        if !out.ends_with(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        src = s.source();
    }
    out
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
    let reach = open_after();
    let mut stream =
        match tokio::time::timeout(reach, open_chunk_stream(endpoint, addr, hash, offset)).await {
            Err(_) => anyhow::bail!("could not reach the chunk provider in {}s", reach.as_secs()),
            Ok(r) => r?,
        };
    let want = stream.total_len.saturating_sub(offset) as usize;
    let mut buf = vec![0u8; want];
    read_body_or_stall(&mut stream.recv, &mut buf, stall_after()).await?;
    Ok((stream.total_len, buf))
}

/// An opened chunk request to one provider: the response stream positioned at the
/// body, the body's total ciphertext length, and the live connection kept alive
/// while the body is read. Lets [`ChunkReceiver::fetch_to_file`] *race* providers
/// — open the request to all, then read the body only from the first that responds.
struct ChunkStream {
    _conn: Connection,
    recv: RecvStream,
    total_len: u64,
}

/// Connect to `addr`, request chunk `hash` from `offset`, and read the response
/// header — but not the body. Errors if the provider doesn't have it
/// (`total_len == 0`) or is unreachable.
async fn open_chunk_stream(
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    hash: Hash,
    offset: u64,
) -> Result<ChunkStream> {
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
    anyhow::ensure!(
        resp.total_len <= MAX_CHUNK_CT,
        "provider claims oversized chunk"
    );
    Ok(ChunkStream {
        _conn: conn,
        recv,
        total_len: resp.total_len,
    })
}

/// How long a fetch tolerates silence, from `ARVOLO_CHUNK_STALL_SECS` (seconds).
/// Overridable because the only way to exercise the bound in a test is to make it
/// small; 0 or unparseable means the default. See [`CHUNK_STALL_SECS`].
fn stall_after() -> std::time::Duration {
    secs_from_env("ARVOLO_CHUNK_STALL_SECS", CHUNK_STALL_SECS)
}

/// How long a fetch may spend reaching a provider, from `ARVOLO_CHUNK_OPEN_SECS`.
/// See [`CHUNK_OPEN_SECS`].
fn open_after() -> std::time::Duration {
    secs_from_env("ARVOLO_CHUNK_OPEN_SECS", CHUNK_OPEN_SECS)
}

/// How long a connection that has already served may go quiet, from
/// `ARVOLO_CHUNK_IDLE_SECS`. See [`CHUNK_IDLE_SECS`].
fn idle_after() -> std::time::Duration {
    secs_from_env("ARVOLO_CHUNK_IDLE_SECS", CHUNK_IDLE_SECS)
}

fn secs_from_env(var: &str, default_secs: u64) -> std::time::Duration {
    let secs = std::env::var(var)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default_secs);
    std::time::Duration::from_secs(secs)
}

/// Fill `buf` from `recv`, giving up if no byte arrives for `stall`.
///
/// Deliberately not `read_exact`: that waits for the whole body with no bound, so a
/// provider that goes quiet mid-chunk parks the task for good. Bounding each read
/// instead of the whole fetch keeps a slow-but-alive provider working — every byte
/// received starts the clock over — while turning a silent one into an ordinary
/// error, which is the only form the scheduler can act on.
///
/// A clean end-of-stream before the promised length is a fault too, and named as
/// one: the provider said how long the chunk was in its header.
async fn read_body_or_stall<R: tokio::io::AsyncRead + Unpin>(
    recv: &mut R,
    buf: &mut [u8],
    stall: std::time::Duration,
) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let want = buf.len();
    let mut filled = 0;
    while filled < want {
        let read = tokio::time::timeout(stall, recv.read(&mut buf[filled..])).await;
        let n = match read {
            Err(_) => anyhow::bail!(
                "read chunk body: nothing for {}s after {filled} of {want} bytes",
                stall.as_secs()
            ),
            Ok(r) => r.map_err(|e| anyhow!("read chunk body: {}", with_causes(e)))?,
        };
        if n == 0 {
            anyhow::bail!("read chunk body: ended after {filled} of {want} bytes");
        }
        filled += n;
    }
    Ok(())
}

// ---- sender ---------------------------------------------------------------

/// Which chunks each receiver has acked, keyed by receiver.
///
/// One shared set across receivers answers "which chunks has *somebody* got",
/// which is not the question anyone asks of it. Two receivers taking complementary
/// halves would fill a shared set completely while neither of them held the file —
/// and that set is what decides "delivered", what a departing receiver's backfill
/// tail is computed from, and (through `copies_served`) when a `--share-copies`
/// limit stops the share. All three were wrong in exactly that case: a copy counted
/// that nobody received, a tail not backfilled because someone *else* had it, and a
/// share cut off mid-transfer with both receivers incomplete.
///
/// Entries are never dropped, including when a receiver disconnects: a resumed
/// receiver must keep the acks it already gave, and "how many distinct receivers
/// took a copy" cannot be answered by a structure that forgets them. So this grows
/// with the number of *distinct* receivers a share has ever had — which is the
/// quantity the share is counting anyway, at a chunk bitset each.
type DeliveredByPeer = Arc<Mutex<HashMap<EndpointId, HashSet<u32>>>>;

#[derive(Debug, Clone)]
struct CtrlHandler {
    total: usize,
    delivered: DeliveredByPeer,
    on_relay: Arc<Mutex<HashSet<u32>>>,
    gone_tx: mpsc::UnboundedSender<(EndpointId, Vec<usize>)>,
    /// Signalled once per receiver, when its control channel connects.
    connected_tx: mpsc::UnboundedSender<()>,
    /// Signalled when a receiver says [`CtrlMsg::Abort`]: it is cancelling on
    /// purpose, and the send must end instead of keeping its tail warm.
    abort_tx: mpsc::UnboundedSender<EndpointId>,
}

impl ProtocolHandler for CtrlHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let peer = conn.remote_id();
        let Ok((mut send, mut recv)) = conn.accept_bi().await else {
            tracing::debug!("ctrl: accept_bi failed");
            return Ok(());
        };
        tracing::debug!("ctrl: receiver connected");
        let _ = self.connected_tx.send(());
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
                // An ack for a chunk this transfer doesn't have is dropped rather
                // than recorded. The index comes off the wire from the receiver, and
                // the size of this set is what "they have the whole file" is decided
                // on — so without the bound, a receiver that acked a few thousand
                // invented indices would be counted as a completed copy, and on a
                // share with a `--share-copies` limit that is enough to close the
                // share on everybody else.
                Ok(Some(CtrlMsg::Have(idx))) if (idx as usize) < self.total => {
                    self.delivered
                        .lock()
                        .unwrap()
                        .entry(peer)
                        .or_default()
                        .insert(idx);
                }
                // A deliberate cancel: end the send, don't nurse the tail.
                Ok(Some(CtrlMsg::Abort)) => break "aborted",
                Ok(Some(_)) => {}        // Ping/Hello: keepalive
                Ok(None) => break "eof", // closed cleanly
                Err(_) => break "idle",  // idle: receiver gone
            }
        };
        if reason == "aborted" {
            tracing::debug!("ctrl: receiver aborted on purpose");
            let _ = self.abort_tx.send(peer);
            conn.close(0u32.into(), b"aborted");
            return Ok(());
        }
        // Receiver gone: report the chunks *this* receiver still lacks. What another
        // receiver happens to hold is no comfort to this one, and it is this one's
        // tail the relay is about to be asked to keep.
        let map = self.delivered.lock().unwrap();
        let mine = map.get(&peer).cloned().unwrap_or_default();
        drop(map);
        let undelivered: Vec<usize> = (0..self.total)
            .filter(|i| !mine.contains(&(*i as u32)))
            .collect();
        tracing::debug!(
            "ctrl: receiver gone ({reason}); delivered={} undelivered={}",
            mine.len(),
            undelivered.len()
        );
        let _ = self.gone_tx.send((peer, undelivered));
        // Acknowledge the clean shutdown. We have read the receiver's stream to EOF,
        // so every `Have` it sent is already counted above; closing now lets its
        // `Control::finish` return at once instead of sitting out its close timeout,
        // which would otherwise stall the end of every completed download.
        conn.close(0u32.into(), b"done");
        Ok(())
    }
}

/// A running sender: serves every chunk and orchestrates lazy relay backfill.
pub struct ChunkSender {
    router: Router,
    endpoint: Endpoint,
    addr: EndpointAddr,
    chunks: Vec<Hash>,
    total_size: u64,
    key: [u8; crate::crypto::CHUNK_KEY_LEN],
    node_seed: [u8; 32],
    delivered: DeliveredByPeer,
    on_relay: Arc<Mutex<HashSet<u32>>>,
    gone_rx: AsyncMutex<mpsc::UnboundedReceiver<(EndpointId, Vec<usize>)>>,
    connected_rx: AsyncMutex<mpsc::UnboundedReceiver<()>>,
    abort_rx: AsyncMutex<mpsc::UnboundedReceiver<EndpointId>>,
    peers: PeerCount,
    sent: SentBytes,
}

/// What the hashing pass produces: the chunk digests plus the sizes derived from
/// them. A named struct rather than a 4-tuple only because the pass now returns
/// across a `spawn_blocking` boundary, where the tuple was unreadable.
///
/// Public, and cloneable, because it is worth **keeping**. Producing it means
/// reading and encrypting the entire payload — measured at ~127 MiB/s, so a
/// minute and a half for 10 GB — while what it produces is one 32-byte digest per
/// 16 MiB chunk: 22 KB for that same 10 GB file. A caller that has to serve the
/// same bytes again (a delivery loop retrying while the recipient is not yet
/// connectable) can hand it back instead of paying for the pass a second time.
#[derive(Clone)]
pub struct PreparedChunks {
    total_size: u64,
    total_chunks: u32,
    chunks: Vec<Hash>,
    index: HashMap<Hash, u32>,
}

impl PreparedChunks {
    /// Read and encrypt `source` chunk by chunk to compute the digests, keeping
    /// none of the ciphertext — chunks are regenerated on demand while serving.
    ///
    /// On the blocking pool, not the async workers. This pass has no `.await`
    /// anywhere in it, so run on the runtime it pins a worker for its whole
    /// duration — tens of seconds per gigabyte. A handful of large sends prepared
    /// at once therefore occupied every worker and the daemon stopped answering
    /// its control socket at all: `arvolo status` and the GUI both hung until the
    /// last one finished, which looks exactly like a crash and is not one.
    /// Reading is sequential — one pass over the file, which is what a disk wants
    /// — but the sealing and hashing of each chunk runs on [`PREP_WORKERS`]
    /// threads. `seal_chunk` is a pure function of `(key, index, bytes)`, so the
    /// chunks are independent by construction and the digests come out identical
    /// to the single-threaded order; only the wall clock changes. Measured at
    /// 132 MiB/s on one core, which on a 10 GB file is minutes the sender spends
    /// before anyone has been offered anything.
    pub async fn compute(
        source: SendSource,
        key: [u8; crate::crypto::CHUNK_KEY_LEN],
    ) -> Result<Self> {
        tokio::task::spawn_blocking(move || -> Result<Self> {
            let total_size = source.len()?;
            let total_chunks = (total_size as usize).div_ceil(CHUNK_SIZE as usize) as u32;
            let mut file = source
                .sequential_reader()
                .with_context(|| format!("open {}", source.label()))?;

            let workers = prep_workers();
            let (done_tx, done_rx) = std::sync::mpsc::channel::<Result<(u32, Hash)>>();
            let mut hashes: Vec<Option<Hash>> = Vec::new();

            std::thread::scope(|scope| -> Result<()> {
                // One rendezvous channel per worker: `sync_channel(0)` hands a
                // buffer over only when that worker is free, so the reader cannot
                // run ahead and pull the whole file into memory. What is in flight
                // is bounded at one chunk per worker plus the one being read.
                let mut feed = Vec::with_capacity(workers);
                for _ in 0..workers {
                    let (tx, rx) = std::sync::mpsc::sync_channel::<(u32, Vec<u8>)>(0);
                    feed.push(tx);
                    let done = done_tx.clone();
                    scope.spawn(move || {
                        for (idx, buf) in rx {
                            let out = crate::crypto::seal_chunk(&key, idx, total_chunks, &buf)
                                .map(|ct| (idx, Hash::new(&ct)));
                            // A closed receiver means the reader gave up (an I/O
                            // error, or a worker before us failed): stop, don't
                            // spend the rest of the file on an answer nobody wants.
                            if done.send(out).is_err() {
                                return;
                            }
                        }
                    });
                }
                drop(done_tx);

                let mut idx: u32 = 0;
                loop {
                    let mut buf = vec![0u8; CHUNK_SIZE as usize];
                    let n = fill(&mut file, &mut buf).context("read file")?;
                    if n == 0 {
                        break;
                    }
                    buf.truncate(n);
                    // Round-robin rather than a shared queue: the chunks are the
                    // same size and cost the same, so there is nothing for work
                    // stealing to balance, and a shared queue would need a lock
                    // held across a blocking recv — which serialises the very
                    // thing being parallelised.
                    if feed[idx as usize % workers].send((idx, buf)).is_err() {
                        break; // a worker died; the error comes back on `done_rx`
                    }
                    idx += 1;
                }
                drop(feed);
                Ok(())
            })?;

            for got in done_rx {
                let (idx, hash) = got?;
                let i = idx as usize;
                if hashes.len() <= i {
                    hashes.resize(i + 1, None);
                }
                hashes[i] = Some(hash);
            }

            // Back into file order, which is the order everything downstream
            // assumes: the ticket's chunk list is indexed by position.
            let mut chunks = Vec::with_capacity(hashes.len());
            let mut index: HashMap<Hash, u32> = HashMap::new();
            for (i, h) in hashes.into_iter().enumerate() {
                let h = h.context("a chunk went missing while hashing")?;
                chunks.push(h);
                index.insert(h, i as u32);
            }
            Ok(Self {
                total_size,
                total_chunks,
                chunks,
                index,
            })
        })
        .await
        .context("hash payload")?
    }

    /// Rebuild from digests computed by an earlier pass — a preparation that was
    /// written down and is being picked up again — without re-reading a byte of
    /// the payload.
    ///
    /// `index` is a pure reverse map of `chunks`, so nothing is lost by not having
    /// kept it (the same rebuild happens in [`ChunkSeeder::start`]). What *is* lost
    /// is the proof the pass gives for free: that the payload still holds the bytes
    /// these digests were taken from. The caller owes that guarantee — see
    /// [`crate::flow::ReusablePrep::from_parts`], which is where it is spelled out.
    ///
    /// Refuses a digest list that does not describe `total_size`. That catches a
    /// torn record and a payload that changed size, which is the cheapest half of
    /// the guarantee the caller is otherwise carrying alone.
    pub fn from_digests(total_size: u64, chunks: Vec<Hash>) -> Result<Self> {
        let total_chunks = (total_size as usize).div_ceil(CHUNK_SIZE as usize) as u32;
        anyhow::ensure!(
            total_chunks as usize == chunks.len(),
            "{} digests do not describe {total_size} bytes (expected {total_chunks})",
            chunks.len()
        );
        let mut index: HashMap<Hash, u32> = HashMap::new();
        for (i, h) in chunks.iter().enumerate() {
            index.insert(*h, i as u32);
        }
        Ok(Self {
            total_size,
            total_chunks,
            chunks,
            index,
        })
    }

    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    /// The ordered chunk digests — what a ticket carries, and what a persisted
    /// preparation has to write down.
    pub fn chunks(&self) -> &[Hash] {
        &self.chunks
    }
}

/// How many threads seal and hash chunks at once.
///
/// Capped rather than "every core": each worker holds a whole 16 MiB chunk, so
/// this number is also a memory ceiling — and the daemon can be preparing several
/// sends at the same time, which multiplies it. Four is where the wait stops being
/// the thing you notice (a 10 GB file goes from about two minutes to about thirty
/// seconds) without the footprint becoming the thing you notice instead.
fn prep_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 4)
}

impl ChunkSender {
    /// Serve `source` under a fresh random per-transfer content key and node id.
    pub async fn serve(source: impl Into<SendSource>, relay: RelayChoice) -> Result<Self> {
        Self::serve_resume(
            source,
            relay,
            crate::crypto::random_chunk_key(),
            crate::node::random_node_seed(),
        )
        .await
    }

    /// Serve `source` under an explicit content `key` and transport `node_seed`.
    /// Used to *resume* a previous send: the same key over the same (unchanged)
    /// file reproduces identical chunk hashes, and the same seed reproduces the
    /// same node id — so the ticket the original session handed out stays valid.
    pub async fn serve_resume(
        source: impl Into<SendSource>,
        relay: RelayChoice,
        key: [u8; crate::crypto::CHUNK_KEY_LEN],
        node_seed: [u8; 32],
    ) -> Result<Self> {
        let source = source.into();
        let prepared = PreparedChunks::compute(source.clone(), key).await?;
        Self::serve_prepared(source, relay, key, node_seed, prepared).await
    }

    /// [`serve_resume`](Self::serve_resume) with the hashing pass already done.
    ///
    /// The split exists so a caller that serves the same bytes more than once —
    /// a delivery loop retrying against a recipient who has not connected yet —
    /// pays for the read-and-encrypt pass once instead of once per attempt. Feed
    /// it a [`PreparedChunks`] computed under this same `key`, or the digests will
    /// not match the ciphertext this sender produces.
    pub async fn serve_prepared(
        source: impl Into<SendSource>,
        relay: RelayChoice,
        key: [u8; crate::crypto::CHUNK_KEY_LEN],
        node_seed: [u8; 32],
        prepared: PreparedChunks,
    ) -> Result<Self> {
        let source = source.into();
        let PreparedChunks {
            total_size,
            total_chunks,
            chunks,
            index,
        } = prepared;
        let peers = PeerCount::default();
        let sent = SentBytes::default();
        let chunk_server = ChunkServer {
            backend: Arc::new(ChunkBackend::OnTheFly {
                source,
                key,
                index,
                total_chunks,
            }),
            peers: peers.clone(),
            sent: sent.clone(),
        };

        let use_relay = !matches!(relay, RelayChoice::Disabled);
        let endpoint =
            bind_endpoint_with_key(relay, crate::node::secret_key_from_seed(&node_seed)).await?;

        let delivered: DeliveredByPeer = Arc::new(Mutex::new(HashMap::new()));
        let on_relay = Arc::new(Mutex::new(HashSet::new()));
        let (gone_tx, gone_rx) = mpsc::unbounded_channel();
        let (connected_tx, connected_rx) = mpsc::unbounded_channel();
        let (abort_tx, abort_rx) = mpsc::unbounded_channel();
        let handler = CtrlHandler {
            total: chunks.len(),
            delivered: delivered.clone(),
            on_relay: on_relay.clone(),
            gone_tx,
            connected_tx,
            abort_tx,
        };
        let router = Router::builder(endpoint.clone())
            .accept(CHUNK_ALPN, chunk_server)
            .accept(CTRL_ALPN, handler)
            .spawn();
        let addr = if use_relay {
            endpoint.online().await;
            remote_addr_of(&endpoint)
        } else {
            local_addr_of(&endpoint)
        };
        Ok(Self {
            router,
            endpoint,
            addr,
            chunks,
            total_size,
            key,
            node_seed,
            delivered,
            on_relay,
            gone_rx: AsyncMutex::new(gone_rx),
            connected_rx: AsyncMutex::new(connected_rx),
            abort_rx: AsyncMutex::new(abort_rx),
            peers,
            sent,
        })
    }

    pub fn addr(&self) -> EndpointAddr {
        self.addr.clone()
    }
    /// The transport secret seed this sender is bound under. Persisted by the CLI
    /// so a resumed send can rebind the *same* node id (see `flow::resume_send`).
    pub fn node_seed(&self) -> [u8; 32] {
        self.node_seed
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
    /// Chunks acked by whichever receiver has acked the most — "how far along is
    /// the best-served receiver", not "how many distinct chunks left the machine".
    pub fn delivered_count(&self) -> usize {
        self.delivered
            .lock()
            .unwrap()
            .values()
            .map(|s| s.len())
            .max()
            .unwrap_or(0)
    }

    /// Receivers that have acked every chunk — one entry per receiver holding a
    /// complete copy. Empty for a zero-chunk payload, which no ack can ever satisfy
    /// (the disconnect with an empty tail is what proves that one; see
    /// [`SendSession::serve`](crate::flow::SendSession::serve)).
    pub fn completed_peers(&self) -> Vec<EndpointId> {
        let total = self.chunks.len();
        if total == 0 {
            return Vec::new();
        }
        self.delivered
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, got)| got.len() >= total)
            .map(|(peer, _)| *peer)
            .collect()
    }

    /// Chunk bytes pushed out to receivers so far — the fine-grained progress
    /// signal the ack count can't give (see [`SentBytes`] for what it does and
    /// doesn't mean). Sums every receiver and every re-send, so the caller must
    /// clamp it to the payload size.
    pub fn sent_bytes(&self) -> u64 {
        self.sent.best()
    }

    /// How many distinct peers are currently downloading from this sender.
    pub fn active_peers(&self) -> usize {
        self.peers.distinct()
    }

    /// Resolves when a receiver's control channel connects (once per receiver).
    /// Never fires if no receiver shows up — used to distinguish "nobody home"
    /// from an in-progress transfer.
    pub async fn receiver_connected(&self) {
        let mut rx = self.connected_rx.lock().await;
        let _ = rx.recv().await;
    }

    /// Resolves when a connected receiver disconnects, yielding *which* receiver it
    /// was and the chunk indices it still lacked (the tail to backfill). Never fires
    /// if no receiver has connected.
    ///
    /// The identity matters to the caller: an empty tail means that receiver has a
    /// complete copy, and reporting it as one more delivery has to be done once per
    /// receiver rather than once per session.
    ///
    /// Once the channel closes — the handler is gone, i.e. we are shutting down —
    /// this never resolves again. It is awaited inside a `select!`, so "never" is the
    /// right answer there: the other arms keep working and the loop exits on its
    /// cancel. The alternatives were both wrong. Resolving with an empty tail (what
    /// the pre-tuple version did) reads as "a receiver took the whole file" and
    /// reports a delivery that never happened; resolving with a fabricated id needs
    /// an id to fabricate, and the all-zero one had to be parsed back into a curve
    /// point behind an `expect` — a panic in a shutdown path, guarding a value that
    /// meant nothing anyway.
    pub async fn receiver_gone(&self) -> (EndpointId, Vec<usize>) {
        let mut rx = self.gone_rx.lock().await;
        match rx.recv().await {
            Some(v) => v,
            None => std::future::pending().await,
        }
    }

    /// Resolves when a receiver cancels ON PURPOSE ([`CtrlMsg::Abort`]): unlike
    /// [`Self::receiver_gone`] there is no tail to keep warm — the send should
    /// end. Never fires for crashes or network drops.
    pub async fn receiver_aborted(&self) -> EndpointId {
        let mut rx = self.abort_rx.lock().await;
        match rx.recv().await {
            Some(v) => v,
            None => std::future::pending().await,
        }
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
        // Stop serving *now*, not after the current download drains. This runs on
        // `cancel`, which must actually interrupt an in-flight transfer — so close
        // the endpoint FIRST: that tears down active receiver connections, so their
        // chunk-serving handler tasks error out and end. Only then shut the router's
        // accept loop. The old order (`router.shutdown().await` first) waited for the
        // handlers to finish, so a receiver mid-download of a large file kept pulling
        // to completion after `cancel`, leaving the transfer "active" the whole time.
        //
        // Closing the endpoint (rather than dropping the sender) avoids the
        // "task was cancelled" panic the `AbortOnDropHandle` drop would raise: the
        // handlers observe a closed connection and return cleanly. A JoinError from
        // `router.shutdown` only means a handler panicked at teardown — ignore it.
        self.endpoint.close().await;
        let _ = self.router.shutdown().await;
    }
}

/// A receiver-side seeder: serves, over `CHUNK_ALPN`, the pieces this receiver has
/// already verified — re-sealed from its own output file, so the ciphertext is
/// byte-identical to the sender's. Lets a downloading peer also *upload* to other
/// swarm peers. One endpoint + router; keep it alive for the transfer.
pub struct ChunkSeeder {
    router: Router,
    endpoint: Endpoint,
    addr: EndpointAddr,
}

impl ChunkSeeder {
    /// Start seeding the pieces of `chunks` (the ticket's ordered hash list) from
    /// the plaintext at `path` under `key`. `have` is a live count of contiguous
    /// committed pieces — indices `< have` are served, and the caller bumps it as
    /// it commits. Resolves once the seeder is reachable (its address is known).
    pub async fn start(
        path: PathBuf,
        key: [u8; crate::crypto::CHUNK_KEY_LEN],
        chunks: &[Hash],
        total_chunks: u32,
        have: HaveBitfield,
        relay: RelayChoice,
    ) -> Result<Self> {
        let mut index: HashMap<Hash, u32> = HashMap::new();
        for (i, h) in chunks.iter().enumerate() {
            index.insert(*h, i as u32);
        }
        let chunk_server = ChunkServer {
            backend: Arc::new(ChunkBackend::Reseal {
                path,
                key,
                index,
                total_chunks,
                have,
            }),
            peers: PeerCount::default(),
            // A seeder uploads to swarm peers, not to "the" recipient, so its
            // byte count is nobody's progress bar — nothing reads this one.
            sent: SentBytes::default(),
        };
        // Mirror `ChunkSender::serve`: only wait to come "online" via a relay when
        // one is configured. With `RelayChoice::Disabled` there is no relay to
        // become reachable through, so `online()` would block forever — use the
        // local direct address instead (LAN / relay-less swarming).
        let use_relay = !matches!(&relay, RelayChoice::Disabled);
        let endpoint = bind_endpoint(relay).await?;
        let router = Router::builder(endpoint.clone())
            .accept(CHUNK_ALPN, chunk_server)
            .spawn();
        let addr = if use_relay {
            endpoint.online().await;
            remote_addr_of(&endpoint)
        } else {
            local_addr_of(&endpoint)
        };
        Ok(Self {
            router,
            endpoint,
            addr,
        })
    }

    /// This seeder's address, to announce to the swarm tracker.
    pub fn addr(&self) -> EndpointAddr {
        self.addr.clone()
    }

    pub async fn shutdown(self) {
        // Close the endpoint first so active peer connections are torn down at once
        // (their handlers end), then shut the router — same reasoning as
        // `ChunkSender::shutdown`: stop seeding immediately on cancel, don't drain.
        self.endpoint.close().await;
        let _ = self.router.shutdown().await;
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
    /// A cheap clone of the underlying connection, so a supervisor can await
    /// `.closed()` (to detect a drop and reconnect) without holding a borrow on
    /// `self` that would block `ack`.
    pub fn connection(&self) -> Connection {
        self.conn.clone()
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

    /// Tell the sender this receiver is cancelling ON PURPOSE, then close. The
    /// sender ends the transfer instead of backfilling the tail to the relay
    /// and re-offering — see [`CtrlMsg::Abort`]. Best-effort: an old sender
    /// reads it as a plain disconnect.
    pub async fn abort(self) -> Result<()> {
        self.heartbeat.abort();
        {
            let mut send = self.send.lock().await;
            write_msg(&mut send, &CtrlMsg::Abort).await?;
            send.finish().map_err(|e| anyhow!("ctrl abort: {e}"))?;
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

/// A receiver endpoint that fetches chunks (with provider fallback). Cheap to
/// `clone` (the inner `Endpoint` is `Arc`-backed), so parallel fetch tasks each
/// hold their own handle.
#[derive(Clone)]
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

    /// Fetch chunk `hash` from a **single** provider `addr` into `out`,
    /// BLAKE3-verified. The scheduler chooses which provider (least-in-flight, for
    /// load balancing) and re-queues on failure, so this does no provider fallback
    /// or retry itself. On an integrity failure the provider is banned. Fetches the
    /// whole chunk fresh (a chunk is <= one CHUNK_SIZE).
    pub async fn fetch_one(
        &self,
        addr: &EndpointAddr,
        hash: Hash,
        out: &mut std::fs::File,
        banned: &Mutex<HashSet<String>>,
    ) -> Result<()> {
        use std::io::Write;
        // The open is bounded for the same reason the body is — an unbounded await
        // there parks the task exactly as an unbounded read did — but on its own
        // clock: reaching a provider is a different fault from one that goes quiet
        // mid-chunk, and a far slower thing to do.
        let reach = open_after();
        let mut stream =
            match tokio::time::timeout(reach, open_chunk_stream(&self.endpoint, addr, hash, 0))
                .await
            {
                Err(_) => {
                    anyhow::bail!("could not reach the chunk provider in {}s", reach.as_secs())
                }
                Ok(r) => r?,
            };
        let mut buf = vec![0u8; stream.total_len as usize];
        read_body_or_stall(&mut stream.recv, &mut buf, stall_after()).await?;
        if Hash::new(&buf) != hash {
            banned.lock().unwrap().insert(addr.id.to_string());
            anyhow::bail!("chunk {hash} failed integrity check");
        }
        out.set_len(0)?;
        out.seek(SeekFrom::Start(0))?;
        out.write_all(&buf)?;
        Ok(())
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

    /// What this ticket *serves*, as opposed to how to reach it:
    /// [`crate::swarm::swarm_id`] over the digests and the size.
    ///
    /// Two tickets for the same send agree on it and nothing else has to: the
    /// provider address changes with every socket bind, and the sealed key blob is
    /// randomised by HPKE on every seal, so comparing ticket strings answers a
    /// question nobody asked. It is also a strong claim about *which* send: the
    /// digests are of ciphertext under a random 32-byte content key, so producing
    /// the same id means holding that key.
    pub fn content_id(&self) -> String {
        crate::swarm::swarm_id(&self.chunks, self.total_size)
    }
}

/// What the sender hands the relay to backfill chunks: the sender's address, the
/// chunk hashes to fetch, the transfer token, and the content-derived swarm id.
/// Base32 `arvs…`.
///
/// `swarm_id` is [`crate::swarm::swarm_id`] over the *whole* file (not just the
/// chunks in this request): it is the durable, content-derived key the relay uses
/// to meter how many bytes a single transfer offloads onto it — stable across the
/// sender suspending, resuming, or restarting.
#[derive(Serialize, Deserialize)]
pub struct SeedRequest {
    pub sender: EndpointAddr,
    pub chunks: Vec<Hash>,
    pub token: String,
    pub swarm_id: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> EndpointId {
        crate::node::generate_secret_key().public()
    }

    #[test]
    fn sent_bytes_reports_the_furthest_receiver_not_the_sum() {
        let sent = SentBytes::default();
        let (a, b) = (peer(), peer());
        sent.counter(a).fetch_add(300, Ordering::Relaxed);
        sent.counter(b).fetch_add(700, Ordering::Relaxed);
        // Summed, this would read 1000 — and on a 700-byte payload a caller would
        // clamp it to "done" while `a` still had over half to go.
        assert_eq!(sent.best(), 700);

        // The same peer coming back adds to its own total, not to a new one.
        sent.counter(a).fetch_add(500, Ordering::Relaxed);
        assert_eq!(sent.best(), 800);
        assert_eq!(sent.0.lock().unwrap().len(), 2, "two peers, two counters");
    }

    #[test]
    fn nothing_sent_yet_is_zero_not_a_panic() {
        assert_eq!(SentBytes::default().best(), 0);
    }
}

#[cfg(test)]
mod prepared_chunks_tests {
    use super::*;

    /// The digests are computed on several threads and collected out of order, so
    /// the one thing that can break is the thing everything downstream depends on:
    /// `chunks[i]` must be the hash of the i-th chunk of the file, and the reverse
    /// index must agree. Checked against the definition itself — sealing each chunk
    /// in place, sequentially — rather than against a previous run.
    #[tokio::test]
    async fn the_digests_come_back_in_file_order() {
        // Two chunks and a half, so the tail (a short final chunk) is covered too.
        let len = (2 * CHUNK_SIZE as usize) + (CHUNK_SIZE as usize / 2);
        let body: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload.bin");
        std::fs::write(&path, &body).unwrap();

        let key = crate::crypto::random_chunk_key();
        let prepared = PreparedChunks::compute(path.as_path().into(), key)
            .await
            .expect("hash the payload");

        let total_chunks = body.len().div_ceil(CHUNK_SIZE as usize) as u32;
        assert_eq!(prepared.total_chunks, total_chunks);
        assert_eq!(prepared.total_size, body.len() as u64);
        assert_eq!(prepared.chunks.len(), total_chunks as usize);

        for i in 0..total_chunks {
            let start = i as usize * CHUNK_SIZE as usize;
            let end = (start + CHUNK_SIZE as usize).min(body.len());
            let ct = crate::crypto::seal_chunk(&key, i, total_chunks, &body[start..end]).unwrap();
            assert_eq!(
                prepared.chunks[i as usize],
                Hash::new(&ct),
                "chunk {i} is not the hash of the bytes at that position"
            );
            assert_eq!(
                prepared.index.get(&Hash::new(&ct)).copied(),
                Some(i),
                "the reverse index disagrees about where chunk {i} is"
            );
        }
    }

    /// A provider that goes quiet mid-chunk must become an error, not a wait.
    ///
    /// This is the whole point of the bound. The scheduler already knows how to
    /// recover from a failed fetch — cool the provider down, re-queue the piece,
    /// reassign it seconds later — and reaches none of that for a fetch that never
    /// returns. A 10.7 GiB download once sat at 97.4% for hours on this.
    #[tokio::test]
    async fn a_silent_provider_fails_instead_of_hanging_for_ever() {
        // Sends a few bytes, then holds the stream open saying nothing.
        let (mut w, mut r) = tokio::io::duplex(64);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            w.write_all(b"half").await.unwrap();
            // Never write the rest, never close: the shape of the hang.
            std::future::pending::<()>().await;
        });

        let mut buf = [0u8; 16];
        let started = std::time::Instant::now();
        let out = read_body_or_stall(&mut r, &mut buf, std::time::Duration::from_millis(200)).await;
        let waited = started.elapsed();

        let err = out.expect_err("a provider that stops sending is a failed fetch");
        assert!(
            err.to_string().contains("nothing for"),
            "the error should say it went quiet, got: {err}"
        );
        assert!(
            err.to_string().contains("4 of 16"),
            "and how far it got, so a log line is worth reading, got: {err}"
        );
        assert!(
            waited < std::time::Duration::from_secs(5),
            "took {waited:?}"
        );
    }

    /// The bound is on silence, not on duration: a provider that keeps sending, even
    /// slowly, must not be cut off. Every byte starts the clock over.
    #[tokio::test]
    async fn a_slow_but_talking_provider_is_not_cut_off() {
        let (mut w, mut r) = tokio::io::duplex(64);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            // Eight writes, each well inside the window but together far past it.
            for _ in 0..8 {
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                w.write_all(b"ab").await.unwrap();
            }
        });

        let mut buf = [0u8; 16];
        read_body_or_stall(&mut r, &mut buf, std::time::Duration::from_millis(200))
            .await
            .expect("a slow provider is still a working provider");
        assert_eq!(&buf, b"abababababababab");
    }

    /// A provider that closes early is a fault too — it promised a length in the
    /// header — and must not leave the caller with a half-filled buffer it believes.
    #[tokio::test]
    async fn a_truncated_body_is_an_error_not_a_short_chunk() {
        let (mut w, mut r) = tokio::io::duplex(64);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            w.write_all(b"short").await.unwrap();
            w.shutdown().await.unwrap();
        });

        let mut buf = [0u8; 16];
        let err = read_body_or_stall(&mut r, &mut buf, std::time::Duration::from_secs(5))
            .await
            .expect_err("a truncated body must not pass for a whole chunk");
        assert!(
            err.to_string().contains("ended after 5 of 16"),
            "got: {err}"
        );
    }

    /// An empty payload has no chunks — and must not hang waiting for one.
    #[tokio::test]
    async fn an_empty_payload_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.bin");
        std::fs::write(&path, b"").unwrap();
        let prepared =
            PreparedChunks::compute(path.as_path().into(), crate::crypto::random_chunk_key())
                .await
                .expect("hash an empty payload");
        assert_eq!(prepared.total_size, 0);
        assert!(prepared.chunks.is_empty());
    }
}
