//! Swarm (multi-peer) primitives for shared `arvc…` tickets: the content id, the
//! tracker request/response types, and bitfield helpers. See
//! `docs/SWARM-DESIGN.md`. The tracker itself lives in the relay; these types are
//! the shared wire contract between a peer and the tracker, and between peers.

use serde::{Deserialize, Serialize};

use crate::hash::Hash;

/// Deterministic content id for a swarm: BLAKE3 over the ordered piece hashes and
/// the total size. Every ticket holder derives the same id without contacting
/// anyone; it is the tracker key and reveals nothing about the plaintext (the
/// piece hashes are of ciphertext). Hex-encoded.
pub fn swarm_id(chunks: &[Hash], total_size: u64) -> String {
    let mut buf = Vec::with_capacity(chunks.len() * 32 + 8);
    for h in chunks {
        buf.extend_from_slice(h.as_bytes());
    }
    buf.extend_from_slice(&total_size.to_le_bytes());
    Hash::new(&buf).to_string()
}

/// Bytes needed for a `n_chunks`-bit have/not bitfield.
pub fn bitfield_bytes(n_chunks: usize) -> usize {
    n_chunks.div_ceil(8)
}

/// A fresh all-zero bitfield for `n_chunks` pieces.
pub fn bitfield_new(n_chunks: usize) -> Vec<u8> {
    vec![0u8; bitfield_bytes(n_chunks)]
}

/// Mark piece `i` present (no-op if out of range).
pub fn bitfield_set(bf: &mut [u8], i: usize) {
    if let Some(b) = bf.get_mut(i / 8) {
        *b |= 1 << (i % 8);
    }
}

/// Whether piece `i` is present.
pub fn bitfield_has(bf: &[u8], i: usize) -> bool {
    bf.get(i / 8).map(|b| b & (1 << (i % 8)) != 0).unwrap_or(false)
}

/// How many pieces the bitfield marks present.
pub fn bitfield_count(bf: &[u8]) -> u32 {
    bf.iter().map(|b| b.count_ones()).sum()
}

/// Announce/refresh a peer in a swarm and ask for others.
/// `POST /v1/swarm/{swarm_id}/announce`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnounceReq {
    /// This peer's encoded chunk-serving address (spoken over `CHUNK_ALPN`).
    pub node_addr: String,
    /// Which pieces this peer already has and can serve.
    pub bitfield: Vec<u8>,
    /// Total piece count — a sanity bound the tracker uses to cap the bitfield.
    pub n_chunks: u32,
    /// `"started" | "progress" | "completed" | "stopped"`.
    pub event: String,
    /// Max number of other peers to return.
    pub want: u32,
}

/// One other peer in the swarm, as returned by the tracker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub node_addr: String,
    pub bitfield: Vec<u8>,
}

/// Tracker response to an announce (or a `GET /v1/swarm/{swarm_id}/peers`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnnounceResp {
    pub peers: Vec<PeerInfo>,
}
