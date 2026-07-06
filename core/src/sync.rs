//! Multi-device contact-book synchronization (shared-identity model).
//!
//! All of a user's devices share one X25519 identity, so they share one inbox
//! slot and one derived snapshot key. A device publishes a **full** encrypted
//! snapshot of its address book as a mutable cell on that slot; the others pull
//! it and **merge**. Merge is a state-based CRDT — commutative, associative and
//! idempotent — so ordering, re-delivery and concurrent edits all converge with
//! no coordination and no device roster.
//!
//! This module is the pure engine: wire types, snapshot encryption (under a key
//! derived from the identity secret, so every device derives it independently),
//! and the merge algorithm. It performs no I/O and no networking — the CLI side
//! (`cli/src/sync.rs`, `cli/src/book.rs`) projects the merged state into the four
//! TOML ledgers, and the transport rides the existing inbox/pairing primitives.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::crypto;

/// Context string for deriving the snapshot-encryption key from the identity
/// secret. Domain-separated from every other `derive_key` use in the codebase so
/// the sync key can never collide with another purpose. Every device holding the
/// same identity secret derives the identical key — that is what lets any device
/// decrypt any other device's snapshot with no key distribution.
pub const SNAPSHOT_KEY_CONTEXT: &str = "arvolo/contacts-sync/key/v1";

/// Derive the 32-byte snapshot-encryption key from the raw identity secret bytes.
pub fn snapshot_key(identity_secret: &[u8]) -> [u8; crypto::CHUNK_KEY_LEN] {
    blake3::derive_key(SNAPSHOT_KEY_CONTEXT, identity_secret)
}

/// A local, per-device random id. **Not** the identity (which is shared across
/// devices); this exists only as a deterministic tiebreak in the Lamport clock so
/// two devices that pick the same counter still order deterministically. Minted
/// once per device and persisted alongside the sync state.
pub type DeviceId = [u8; 16];

/// A fresh random device id.
pub fn random_device_id() -> DeviceId {
    use rand::RngCore;
    let mut id = [0u8; 16];
    rand::rng().fill_bytes(&mut id);
    id
}

/// A Lamport timestamp giving every ledger entry a **total order**: compare by
/// `counter`, breaking ties with `device`. Because the tiebreak is a random
/// per-device id, two independent edits never compare equal unless they are the
/// literally same op, which makes last-writer-wins deterministic on every device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lamport {
    pub counter: u64,
    pub device: DeviceId,
}

impl PartialOrd for Lamport {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Lamport {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.counter
            .cmp(&other.counter)
            .then_with(|| self.device.cmp(&other.device))
    }
}

/// One contact entry in a snapshot: a `name → pubkey` binding carrying its clock
/// and a tombstone flag. A removal is a tombstone (`deleted = true`) rather than
/// an omission — otherwise a peer that still holds the name would re-add it on
/// merge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactEntry {
    pub name: String,
    pub pubkey: String,
    pub clock: Lamport,
    pub deleted: bool,
}

/// One verified/trusted mark, keyed by pubkey (a 2P-set element with a clock).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkEntry {
    pub pubkey: String,
    pub clock: Lamport,
    pub deleted: bool,
}

/// One TOFU seen counter. Merged by `max` (idempotent; preserves the only value
/// ever consumed — the "seen before?" boolean).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenEntry {
    pub pubkey: String,
    pub count: u64,
}

/// A full, self-contained snapshot of the address book — the on-the-wire form of
/// [`SyncState`]. It is *state*, not a delta: it always carries the complete
/// current book (plus tombstones), so a device that missed earlier snapshots
/// still converges from the latest one alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSnapshot {
    /// The writer's Lamport counter at publish time (for clock advancement).
    pub lamport: u64,
    /// The writer's device id.
    pub device: DeviceId,
    pub contacts: Vec<ContactEntry>,
    pub verified: Vec<MarkEntry>,
    pub trusted: Vec<MarkEntry>,
    pub seen: Vec<SeenEntry>,
}

/// The sealed inbox payload carrying a snapshot to the user's other devices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncNote {
    /// The default carrier: the encrypted snapshot inline (fits the 512 KiB inbox
    /// value limit for any realistic personal address book).
    Snapshot { blob: Vec<u8> },
    /// Fallback for an oversized snapshot: a relay blob claim to fetch instead.
    SnapshotRef { claim: String, relay: String },
}

/// What a newly paired device receives, transferred over the SPAKE2 rendezvous
/// (already encrypted under the pairing key): the shared identity secret plus the
/// current address-book snapshot inline. Inline keeps pairing to a single
/// rendezvous round-trip; a personal book fits the 64 KiB rendezvous value limit.
/// (A huge book would need a deposit+claim fallback — not yet implemented.)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairPayload {
    pub identity_secret: [u8; 32],
    pub snapshot: SyncSnapshot,
}

impl PairPayload {
    /// Serialize to postcard bytes for the pairing transfer.
    pub fn encode(&self) -> Result<Vec<u8>> {
        postcard::to_allocvec(self).context("serialize pair payload")
    }

    /// Inverse of [`PairPayload::encode`].
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        postcard::from_bytes(bytes).context("deserialize pair payload")
    }
}

// ---- canonical CRDT state -------------------------------------------------

/// A contact register: the winning `pubkey` for a name, its clock and tombstone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContactReg {
    pub pubkey: String,
    pub clock: Lamport,
    pub deleted: bool,
}

/// A mark register (verified/trusted): clock + tombstone for a pubkey.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkReg {
    pub clock: Lamport,
    pub deleted: bool,
}

/// The canonical, in-memory CRDT state — the merge target and the source of the
/// projection into the four TOML ledgers. Kept as maps for O(log n) merge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncState {
    /// name → winning binding.
    pub contacts: BTreeMap<String, ContactReg>,
    /// pubkey → verified mark.
    pub verified: BTreeMap<String, MarkReg>,
    /// pubkey → trusted mark.
    pub trusted: BTreeMap<String, MarkReg>,
    /// pubkey → seen counter.
    pub seen: BTreeMap<String, u64>,
}

impl SyncState {
    /// Build canonical state from a wire snapshot.
    pub fn from_snapshot(s: &SyncSnapshot) -> Self {
        let mut state = SyncState::default();
        for c in &s.contacts {
            state.contacts.insert(
                c.name.clone(),
                ContactReg {
                    pubkey: c.pubkey.clone(),
                    clock: c.clock,
                    deleted: c.deleted,
                },
            );
        }
        for m in &s.verified {
            state.verified.insert(
                m.pubkey.clone(),
                MarkReg {
                    clock: m.clock,
                    deleted: m.deleted,
                },
            );
        }
        for m in &s.trusted {
            state.trusted.insert(
                m.pubkey.clone(),
                MarkReg {
                    clock: m.clock,
                    deleted: m.deleted,
                },
            );
        }
        for e in &s.seen {
            state.seen.insert(e.pubkey.clone(), e.count);
        }
        state
    }

    /// Serialize to a wire snapshot, sorted for a deterministic byte layout (so
    /// two devices in the same state produce byte-identical, dedup-friendly
    /// snapshots). `lamport`/`device` stamp the writer's clock.
    pub fn to_snapshot(&self, lamport: u64, device: DeviceId) -> SyncSnapshot {
        SyncSnapshot {
            lamport,
            device,
            contacts: self
                .contacts
                .iter()
                .map(|(name, r)| ContactEntry {
                    name: name.clone(),
                    pubkey: r.pubkey.clone(),
                    clock: r.clock,
                    deleted: r.deleted,
                })
                .collect(),
            verified: self
                .verified
                .iter()
                .map(|(pubkey, r)| MarkEntry {
                    pubkey: pubkey.clone(),
                    clock: r.clock,
                    deleted: r.deleted,
                })
                .collect(),
            trusted: self
                .trusted
                .iter()
                .map(|(pubkey, r)| MarkEntry {
                    pubkey: pubkey.clone(),
                    clock: r.clock,
                    deleted: r.deleted,
                })
                .collect(),
            seen: self
                .seen
                .iter()
                .map(|(pubkey, count)| SeenEntry {
                    pubkey: pubkey.clone(),
                    count: *count,
                })
                .collect(),
        }
    }

    /// Merge `other` into `self`. Commutative, associative, idempotent:
    /// - contacts: last-writer-wins by [`Lamport`] (tombstones ordered the same),
    /// - verified/trusted: 2P-set — higher clock wins; on the (practically
    ///   impossible) exact clock tie, delete wins for safety,
    /// - seen: `max`.
    pub fn merge(&mut self, other: &SyncState) {
        for (name, incoming) in &other.contacts {
            match self.contacts.get(name) {
                Some(cur) if cur.clock >= incoming.clock => {}
                _ => {
                    self.contacts.insert(name.clone(), incoming.clone());
                }
            }
        }
        merge_marks(&mut self.verified, &other.verified);
        merge_marks(&mut self.trusted, &other.trusted);
        for (pubkey, count) in &other.seen {
            let e = self.seen.entry(pubkey.clone()).or_insert(0);
            *e = (*e).max(*count);
        }
    }

    /// The highest Lamport counter observed anywhere in this state — used to
    /// advance a device's local clock past everything it has merged.
    pub fn max_counter(&self) -> u64 {
        let mut m = 0u64;
        for r in self.contacts.values() {
            m = m.max(r.clock.counter);
        }
        for r in self.verified.values() {
            m = m.max(r.clock.counter);
        }
        for r in self.trusted.values() {
            m = m.max(r.clock.counter);
        }
        m
    }
}

fn merge_marks(into: &mut BTreeMap<String, MarkReg>, other: &BTreeMap<String, MarkReg>) {
    for (pubkey, incoming) in other {
        match into.get(pubkey) {
            Some(cur) if cur.clock > incoming.clock => {}
            Some(cur) if cur.clock == incoming.clock => {
                // Exact-clock tie (same counter+device): delete wins.
                if incoming.deleted && !cur.deleted {
                    into.insert(pubkey.clone(), incoming.clone());
                }
            }
            _ => {
                into.insert(pubkey.clone(), incoming.clone());
            }
        }
    }
}

// ---- plaintext (local sidecar) serialization ------------------------------

/// Serialize a snapshot to postcard bytes for the **local** unencrypted sidecar
/// (`<config>/sync/meta.bin`). For the on-the-wire form use [`encrypt_snapshot`].
pub fn encode_snapshot(snap: &SyncSnapshot) -> Result<Vec<u8>> {
    postcard::to_allocvec(snap).context("serialize sync snapshot")
}

/// Inverse of [`encode_snapshot`].
pub fn decode_snapshot(bytes: &[u8]) -> Result<SyncSnapshot> {
    postcard::from_bytes(bytes).context("deserialize sync snapshot")
}

// ---- snapshot encryption --------------------------------------------------

/// Encrypt a snapshot under the identity-derived key. Reuses the AES-256-GCM
/// chunk cipher (index 0 / total 1); the key's domain-separated derivation is
/// what isolates this ciphertext from the chunked-transfer path.
pub fn encrypt_snapshot(key: &[u8; crypto::CHUNK_KEY_LEN], snap: &SyncSnapshot) -> Result<Vec<u8>> {
    let plain = postcard::to_allocvec(snap).context("serialize sync snapshot")?;
    crypto::seal_chunk(key, 0, 1, &plain)
}

/// Decrypt a snapshot produced by [`encrypt_snapshot`]. Fails on the wrong key or
/// tampering (AEAD).
pub fn decrypt_snapshot(key: &[u8; crypto::CHUNK_KEY_LEN], blob: &[u8]) -> Result<SyncSnapshot> {
    let plain = crypto::open_chunk(key, 0, 1, blob).context("decrypt sync snapshot")?;
    postcard::from_bytes(&plain).context("deserialize sync snapshot")
}

/// A stable id for a sync note, used to dedup already-merged notes. Derived from
/// the encrypted blob so identical snapshots share an id.
pub fn note_id(blob: &[u8]) -> [u8; 16] {
    let h = blake3::hash(blob);
    let mut id = [0u8; 16];
    id.copy_from_slice(&h.as_bytes()[..16]);
    id
}

// ---- device-join authorization seam ---------------------------------------

/// The decision a [`DeviceJoinAuthorizer`] returns for a pairing applicant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinDecision {
    /// Release the shared identity + snapshot to the applicant.
    Admit,
    /// Refuse the join.
    Deny,
}

/// Pluggable boundary for "who may join this identity's device set". The
/// open-core implementation treats a completed SPAKE2 short-code handshake as the
/// authorization (holding the code proves physical possession). A future
/// Enterprise implementation can call OIDC/SAML against a directory (e.g. Active
/// Directory) here and return [`JoinDecision::Deny`] for unauthorized applicants.
/// Keeping this a trait means no credential logic lives in the open core.
pub trait DeviceJoinAuthorizer {
    /// Decide whether the applicant identified by `applicant_label` may join.
    fn authorize(&self, applicant_label: &str) -> JoinDecision;
}

/// Open-core authorizer: the SPAKE2 code match *is* the authorization, so every
/// applicant that completed the handshake is admitted.
pub struct Spake2ShortCodeAuthorizer;

impl DeviceJoinAuthorizer for Spake2ShortCodeAuthorizer {
    fn authorize(&self, _applicant_label: &str) -> JoinDecision {
        JoinDecision::Admit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lam(counter: u64, d: u8) -> Lamport {
        Lamport {
            counter,
            device: [d; 16],
        }
    }

    fn contact(name: &str, pubkey: &str, clock: Lamport, deleted: bool) -> ContactEntry {
        ContactEntry {
            name: name.into(),
            pubkey: pubkey.into(),
            clock,
            deleted,
        }
    }

    fn state_with(contacts: Vec<ContactEntry>) -> SyncState {
        SyncState::from_snapshot(&SyncSnapshot {
            lamport: 0,
            device: [0u8; 16],
            contacts,
            verified: vec![],
            trusted: vec![],
            seen: vec![],
        })
    }

    #[test]
    fn snapshot_crypto_roundtrip() {
        let key = snapshot_key(b"an identity secret 32 bytes long!");
        let snap = SyncSnapshot {
            lamport: 7,
            device: [9u8; 16],
            contacts: vec![contact("alice", "aaaa", lam(3, 1), false)],
            verified: vec![],
            trusted: vec![],
            seen: vec![SeenEntry {
                pubkey: "aaaa".into(),
                count: 2,
            }],
        };
        let blob = encrypt_snapshot(&key, &snap).unwrap();
        assert_ne!(blob, postcard::to_allocvec(&snap).unwrap());
        assert_eq!(decrypt_snapshot(&key, &blob).unwrap(), snap);
    }

    #[test]
    fn wrong_key_cannot_decrypt() {
        let snap = SyncSnapshot {
            lamport: 0,
            device: [0u8; 16],
            contacts: vec![],
            verified: vec![],
            trusted: vec![],
            seen: vec![],
        };
        let blob = encrypt_snapshot(&snapshot_key(b"secret-one"), &snap).unwrap();
        assert!(decrypt_snapshot(&snapshot_key(b"secret-two"), &blob).is_err());
    }

    #[test]
    fn contacts_last_writer_wins() {
        // Higher clock wins regardless of merge order.
        let a = state_with(vec![contact("bob", "key-old", lam(1, 1), false)]);
        let b = state_with(vec![contact("bob", "key-new", lam(2, 2), false)]);

        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);

        assert_eq!(ab, ba, "merge is commutative");
        assert_eq!(ab.contacts["bob"].pubkey, "key-new");
    }

    #[test]
    fn deletion_propagates_as_tombstone() {
        // A tombstone at a higher clock wins over an older add → contact removed.
        let mut has = state_with(vec![contact("carol", "ck", lam(1, 1), false)]);
        let tombstone = state_with(vec![contact("carol", "ck", lam(2, 1), true)]);
        has.merge(&tombstone);
        assert!(has.contacts["carol"].deleted, "tombstone wins");
    }

    #[test]
    fn re_add_beats_stale_tombstone() {
        // Offline re-add at an even higher clock beats a delete — last intent wins.
        let mut deleted = state_with(vec![contact("dave", "dk", lam(2, 1), true)]);
        let readd = state_with(vec![contact("dave", "dk2", lam(3, 2), false)]);
        deleted.merge(&readd);
        assert!(!deleted.contacts["dave"].deleted);
        assert_eq!(deleted.contacts["dave"].pubkey, "dk2");
    }

    #[test]
    fn marks_2p_set_and_seen_max() {
        let mut a = SyncState::from_snapshot(&SyncSnapshot {
            lamport: 0,
            device: [0u8; 16],
            contacts: vec![],
            verified: vec![MarkEntry {
                pubkey: "k".into(),
                clock: lam(1, 1),
                deleted: false,
            }],
            trusted: vec![],
            seen: vec![SeenEntry {
                pubkey: "k".into(),
                count: 3,
            }],
        });
        let b = SyncState::from_snapshot(&SyncSnapshot {
            lamport: 0,
            device: [0u8; 16],
            contacts: vec![],
            verified: vec![MarkEntry {
                pubkey: "k".into(),
                clock: lam(2, 2),
                deleted: true,
            }],
            trusted: vec![],
            seen: vec![SeenEntry {
                pubkey: "k".into(),
                count: 5,
            }],
        });
        a.merge(&b);
        assert!(a.verified["k"].deleted, "later remove wins the 2P-set");
        assert_eq!(a.seen["k"], 5, "seen merges by max");
    }

    #[test]
    fn merge_is_idempotent() {
        let base = state_with(vec![
            contact("a", "ka", lam(1, 1), false),
            contact("b", "kb", lam(2, 2), true),
        ]);
        let other = state_with(vec![contact("a", "ka2", lam(3, 1), false)]);

        let mut once = base.clone();
        once.merge(&other);
        let mut twice = once.clone();
        twice.merge(&other);
        assert_eq!(once, twice, "re-merging the same snapshot changes nothing");
    }

    #[test]
    fn order_independent_convergence() {
        let s1 = state_with(vec![contact("x", "1", lam(1, 1), false)]);
        let s2 = state_with(vec![contact("x", "2", lam(2, 2), false)]);
        let s3 = state_with(vec![contact("x", "3", lam(2, 3), false)]);

        let mut forward = s1.clone();
        forward.merge(&s2);
        forward.merge(&s3);

        let mut backward = s3.clone();
        backward.merge(&s2);
        backward.merge(&s1);

        assert_eq!(forward, backward, "any order → same state");
    }
}
