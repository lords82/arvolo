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
    bf.get(i / 8)
        .map(|b| b & (1 << (i % 8)) != 0)
        .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hashes(seeds: &[&[u8]]) -> Vec<Hash> {
        seeds.iter().map(|s| Hash::new(s)).collect()
    }

    /// Every ticket holder must derive the *same* swarm id from the piece list — it
    /// is the tracker key, so a mismatch would split a swarm into two.
    #[test]
    fn swarm_id_is_deterministic() {
        let chunks = hashes(&[b"alpha", b"beta", b"gamma"]);
        assert_eq!(swarm_id(&chunks, 4096), swarm_id(&chunks, 4096));
    }

    /// The id binds the total size: two contents with identical piece hashes but a
    /// different declared size are different swarms.
    #[test]
    fn swarm_id_depends_on_total_size() {
        let chunks = hashes(&[b"alpha", b"beta"]);
        assert_ne!(swarm_id(&chunks, 100), swarm_id(&chunks, 101));
    }

    /// Piece *order* is part of the identity — reordering the hashes changes the id,
    /// so a shuffled piece list can't be mistaken for the same content.
    #[test]
    fn swarm_id_depends_on_piece_order() {
        let a = hashes(&[b"one", b"two"]);
        let b = hashes(&[b"two", b"one"]);
        assert_ne!(swarm_id(&a, 32), swarm_id(&b, 32));
    }

    /// `bitfield_bytes` rounds up to whole bytes (8 pieces per byte).
    #[test]
    fn bitfield_bytes_rounds_up() {
        assert_eq!(bitfield_bytes(0), 0);
        assert_eq!(bitfield_bytes(1), 1);
        assert_eq!(bitfield_bytes(8), 1);
        assert_eq!(bitfield_bytes(9), 2);
        assert_eq!(bitfield_bytes(17), 3);
    }

    /// A fresh bitfield has nothing set; setting a bit is visible to `has`/`count`.
    #[test]
    fn set_then_has_and_count_roundtrip() {
        let mut bf = bitfield_new(20);
        assert_eq!(bitfield_count(&bf), 0);
        assert!(!bitfield_has(&bf, 0));

        for i in [0usize, 7, 8, 15, 19] {
            bitfield_set(&mut bf, i);
        }
        for i in [0usize, 7, 8, 15, 19] {
            assert!(bitfield_has(&bf, i), "piece {i} must read back as present");
        }
        for i in [1usize, 6, 9, 18] {
            assert!(!bitfield_has(&bf, i), "piece {i} was never set");
        }
        assert_eq!(bitfield_count(&bf), 5);
    }

    /// Out-of-range indices are a no-op on set and read as absent — the swarm's
    /// disjoint (arbitrary-bit) sets rely on this staying total.
    #[test]
    fn out_of_range_index_is_a_noop() {
        let mut bf = bitfield_new(8);
        bitfield_set(&mut bf, 100); // beyond the field
        assert_eq!(bitfield_count(&bf), 0);
        assert!(!bitfield_has(&bf, 100));
        assert!(!bitfield_has(&[], 0));
    }

    /// Two disjoint halves (evens vs. odds) OR together to the full set — the basis
    /// of the disjoint-piece swarm.
    #[test]
    fn disjoint_bitfields_are_complementary() {
        let n = 4;
        let mut evens = bitfield_new(n);
        let mut odds = bitfield_new(n);
        bitfield_set(&mut evens, 0);
        bitfield_set(&mut evens, 2);
        bitfield_set(&mut odds, 1);
        bitfield_set(&mut odds, 3);
        for i in 0..n {
            assert_ne!(
                bitfield_has(&evens, i),
                bitfield_has(&odds, i),
                "each piece is in exactly one half"
            );
        }
        assert_eq!(bitfield_count(&evens) + bitfield_count(&odds), n as u32);
    }
}
