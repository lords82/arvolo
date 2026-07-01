//! arvolo core — engine abstractions and shared types.
//!
//! The networking engine (iroh) lives **behind** the [`Transport`] trait so the
//! application logic (encryption, mailbox, TTL, GC) never depends on iroh
//! directly and stays swappable (e.g. quinn + libp2p) if needed.
//!
//! This is the Step 0.1 skeleton: types + trait only. iroh is wired in Step 0.2.

use anyhow::Result;
use std::future::Future;

pub mod backfill;
pub mod chunked;
pub mod code;
pub mod crypto;
pub mod flow;
pub mod hash;
pub mod node;
pub mod offline;
pub mod pairing;
pub mod transfer;

/// Re-exports of types that appear in the public API.
pub mod reexport {
    pub use crate::hash::Hash;
}

/// Crate version, surfaced by the CLI and relay.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// BLAKE3 content hash identifying a blob. Content-addressing is the backbone:
/// the hash *is* the manifest and enables source-agnostic resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlobHash(pub [u8; 32]);

/// Stable identity of a node/endpoint (a public key). "Dial keys, not IPs".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub [u8; 32]);

/// How a peer was reached — useful for telemetry and the P2P-first policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Direct connection (LAN or hole-punched).
    Direct,
    /// Relayed live-forward (no storage).
    Relay,
}

/// Abstraction over the P2P transport.
///
/// iroh is the first implementation; keeping it behind a trait hedges the
/// dependency. Methods use `async fn` in traits (stable since Rust 1.75).
pub trait Transport {
    /// This node's stable id (its public key).
    fn node_id(&self) -> NodeId;

    /// Make the blob identified by `hash` available for others to fetch.
    /// Returns once the blob is registered with the transport.
    fn provide(&self, hash: BlobHash) -> impl Future<Output = Result<()>> + Send;

    /// Fetch a blob by hash from `from`, verifying integrity end-to-end.
    fn fetch(&self, from: NodeId, hash: BlobHash) -> impl Future<Output = Result<Vec<u8>>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn ids_are_comparable() {
        assert_eq!(BlobHash([0u8; 32]), BlobHash([0u8; 32]));
        assert_ne!(NodeId([1u8; 32]), NodeId([2u8; 32]));
    }
}
