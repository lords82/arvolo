//! Multi-device contact-book synchronization (shared-identity model).
//!
//! All of a user's devices share one X25519 identity, so they share one inbox
//! slot and one derived snapshot key. A device publishes a **full** encrypted
//! snapshot of its address book as a mutable cell on that slot; the others pull
//! it and **merge**. Merge is a state-based CRDT — commutative, associative and
//! idempotent — so ordering, re-delivery and concurrent edits all converge with
//! no coordination.
//!
//! The snapshot also carries a roster of the machines themselves, so a send can
//! be aimed at one of them. It is a register like the others and rides inside the
//! same encrypted cell, so the relay learns nothing from it — but it is *self
//! reported*: each machine writes its own row and nothing outside proves that a
//! row corresponds to a computer that exists. See [`DeviceEntry`] and the
//! authorship rule in [`SyncState::merge`].
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

/// One verified/trusted/blocked mark, keyed by pubkey (a 2P-set element with a
/// clock).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkEntry {
    pub pubkey: String,
    pub clock: Lamport,
    pub deleted: bool,
    /// Unix seconds when the mark was made — "verified since", "trusted since".
    ///
    /// Wall-clock, and deliberately **not** an input to the merge: the Lamport
    /// clock decides which register wins and this rides along with it. Using it to
    /// order anything would make convergence depend on how well two machines agree
    /// about the time, which is exactly what Lamport clocks exist to avoid.
    ///
    /// `0` means unknown — a mark made before this was recorded.
    #[serde(default)]
    pub since: u64,
}

/// One TOFU seen counter. Merged by `max` (idempotent; preserves the only value
/// ever consumed — the "seen before?" boolean).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenEntry {
    pub pubkey: String,
    pub count: u64,
}

/// One advertised-name binding, keyed by pubkey (last-writer-wins, like a
/// contact). `pinned` is the name the user approved ("gave for good"); `pending`
/// is the last advertised name that differs from it, awaiting approval. A cleared
/// contact is a tombstone rather than an omission, same as [`ContactEntry`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameEntry {
    pub pubkey: String,
    pub pinned: String,
    #[serde(default)]
    pub pending: Option<String>,
    pub clock: Lamport,
    pub deleted: bool,
}

/// One machine holding this identity, as it describes itself.
///
/// The register that is **not** like the others: a contact is something you write
/// about somebody else, a device row is something a machine writes about itself.
/// Nobody adds a device — pairing hands over the seed and the new machine starts
/// announcing itself on its next sync round. See [`SyncState::merge`] for the one
/// extra rule that makes that safe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEntry {
    /// Lowercase hex of the writer's [`DeviceId`] — the key, repeated in the row
    /// so a snapshot stays a flat list.
    pub id: String,
    /// What the user calls this machine. Defaulted from its hostname, editable
    /// only on the machine itself (again: see `merge`).
    pub name: String,
    /// `std::env::consts::OS` on the machine that wrote the row. A hint for the
    /// icon and for telling two similarly-named rows apart, never a decision.
    pub platform: String,
    /// Unix seconds when that machine last published a snapshot. Self-reported —
    /// there is no neutral clock here — so a reader clamps it to "now" and shows
    /// it as elapsed time, never as a presence light.
    pub last_seen: u64,
    pub clock: Lamport,
    pub deleted: bool,
}

/// The state-side form of [`DeviceEntry`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceReg {
    pub name: String,
    pub platform: String,
    pub last_seen: u64,
    pub clock: Lamport,
    pub deleted: bool,
}

/// A [`DeviceId`] as it keys the device register: lowercase hex, so the map key
/// and the row's own `id` are the same string everywhere.
pub fn device_key(id: &DeviceId) -> String {
    data_encoding::HEXLOWER.encode(id)
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
    /// Approved/pending advertised names, keyed by pubkey. `#[serde(default)]` so
    /// a snapshot written by an older device (no `names`) still deserializes.
    #[serde(default)]
    pub names: Vec<NameEntry>,
    /// Silenced identities. Same 2P-set shape as `verified`/`trusted`, and the same
    /// `#[serde(default)]` reason: an older device sends no `blocked` at all, which
    /// merges as "no information", never as "unblock everything".
    #[serde(default)]
    pub blocked: Vec<MarkEntry>,
    /// The machines holding this identity, each row written by the machine it
    /// describes. `#[serde(default)]` for the same reason as the two above: a
    /// device that predates the register sends none, which reads as "I know of no
    /// devices" and never as "there are none".
    #[serde(default)]
    pub devices: Vec<DeviceEntry>,
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

/// A mark register (verified/trusted/blocked): clock + tombstone for a pubkey,
/// plus when the mark was made (see [`MarkEntry::since`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkReg {
    pub clock: Lamport,
    pub deleted: bool,
    pub since: u64,
}

/// A name register: the approved (`pinned`) and awaiting-approval (`pending`)
/// advertised names for a pubkey, plus clock and tombstone. Merged last-writer-wins
/// by [`Lamport`], exactly like [`ContactReg`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameReg {
    pub pinned: String,
    pub pending: Option<String>,
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
    /// pubkey → advertised-name register.
    pub names: BTreeMap<String, NameReg>,
    /// pubkey → blocked mark.
    pub blocked: BTreeMap<String, MarkReg>,
    /// [`device_key`] → what that machine says about itself.
    pub devices: BTreeMap<String, DeviceReg>,
}

impl SyncState {
    /// Does any register here carry a clock stamped by a device other than `mine`?
    ///
    /// A register only gets a foreign device id by winning a merge against a
    /// snapshot another device published, so this answering `true` is proof that
    /// this identity has been on at least two machines. It is *not* proof of the
    /// reverse: a device paired a minute ago, or one that has only ever received,
    /// has nothing of anyone else's to show. Only good enough to decide whether to
    /// suggest `device pair`, which is all it is used for.
    pub fn authored_by_another_device(&self, mine: &DeviceId) -> bool {
        let contacts = self.contacts.values().map(|r| &r.clock);
        let names = self.names.values().map(|r| &r.clock);
        let marks = self
            .verified
            .values()
            .chain(self.trusted.values())
            .chain(self.blocked.values())
            .map(|r| &r.clock);
        // A device row authored by somebody else is the least circumstantial
        // evidence of the lot: it is another machine saying so itself.
        let devices = self.devices.values().map(|r| &r.clock);
        contacts
            .chain(names)
            .chain(marks)
            .chain(devices)
            .any(|c| &c.device != mine)
    }

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
                    since: m.since,
                },
            );
        }
        for m in &s.trusted {
            state.trusted.insert(
                m.pubkey.clone(),
                MarkReg {
                    clock: m.clock,
                    deleted: m.deleted,
                    since: m.since,
                },
            );
        }
        for m in &s.blocked {
            state.blocked.insert(
                m.pubkey.clone(),
                MarkReg {
                    clock: m.clock,
                    deleted: m.deleted,
                    since: m.since,
                },
            );
        }
        for e in &s.seen {
            state.seen.insert(e.pubkey.clone(), e.count);
        }
        for n in &s.names {
            state.names.insert(
                n.pubkey.clone(),
                NameReg {
                    pinned: n.pinned.clone(),
                    pending: n.pending.clone(),
                    clock: n.clock,
                    deleted: n.deleted,
                },
            );
        }
        for d in &s.devices {
            state.devices.insert(
                d.id.clone(),
                DeviceReg {
                    name: d.name.clone(),
                    platform: d.platform.clone(),
                    last_seen: d.last_seen,
                    clock: d.clock,
                    deleted: d.deleted,
                },
            );
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
                    since: r.since,
                })
                .collect(),
            trusted: self
                .trusted
                .iter()
                .map(|(pubkey, r)| MarkEntry {
                    pubkey: pubkey.clone(),
                    clock: r.clock,
                    deleted: r.deleted,
                    since: r.since,
                })
                .collect(),
            blocked: self
                .blocked
                .iter()
                .map(|(pubkey, r)| MarkEntry {
                    pubkey: pubkey.clone(),
                    clock: r.clock,
                    deleted: r.deleted,
                    since: r.since,
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
            names: self
                .names
                .iter()
                .map(|(pubkey, r)| NameEntry {
                    pubkey: pubkey.clone(),
                    pinned: r.pinned.clone(),
                    pending: r.pending.clone(),
                    clock: r.clock,
                    deleted: r.deleted,
                })
                .collect(),
            devices: self
                .devices
                .iter()
                .map(|(id, r)| DeviceEntry {
                    id: id.clone(),
                    name: r.name.clone(),
                    platform: r.platform.clone(),
                    last_seen: r.last_seen,
                    clock: r.clock,
                    deleted: r.deleted,
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
        merge_marks(&mut self.blocked, &other.blocked);
        for (pubkey, count) in &other.seen {
            let e = self.seen.entry(pubkey.clone()).or_insert(0);
            *e = (*e).max(*count);
        }
        for (pubkey, incoming) in &other.names {
            match self.names.get(pubkey) {
                Some(cur) if cur.clock >= incoming.clock => {}
                _ => {
                    self.names.insert(pubkey.clone(), incoming.clone());
                }
            }
        }
        merge_devices(&mut self.devices, &other.devices);
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
        for r in self.names.values() {
            m = m.max(r.clock.counter);
        }
        for r in self.blocked.values() {
            m = m.max(r.clock.counter);
        }
        for r in self.devices.values() {
            m = m.max(r.clock.counter);
        }
        m
    }
}

/// Merge the device register: last-writer-wins by [`Lamport`], like the contacts,
/// **plus one rule the contacts do not have** — a live row is only accepted from
/// the machine it describes.
///
/// Without it the register is an ordinary LWW map, so any device could rewrite
/// any other device's row and win by holding the higher counter. That matters
/// here in a way it does not for a contact: the rows are what a targeted send
/// picks from, so "send it to the MacBook" would come to mean "send it to
/// whichever machine last called itself the MacBook". Requiring `clock.device` to
/// be the row's own id makes a device's description of itself the only one there
/// is.
///
/// Tombstones are exempt, and have to be: forgetting a machine is something you
/// do *from another machine*, usually because the one being forgotten is gone. So
/// anyone may delete a row and nobody may write one but its owner. (What stops a
/// forgotten-but-still-running device from simply re-announcing itself is not
/// here — it is the retirement check on the writing side, which declines to
/// republish a row its own identity has tombstoned.)
fn merge_devices(into: &mut BTreeMap<String, DeviceReg>, other: &BTreeMap<String, DeviceReg>) {
    for (id, incoming) in other {
        // Self-authorship, checked against the key rather than against whoever
        // handed us the snapshot: snapshots are relayed between devices, so the
        // machine we merged this from is very often not the one that wrote it.
        if !incoming.deleted && device_key(&incoming.clock.device) != *id {
            continue;
        }
        match into.get(id) {
            Some(cur) if cur.clock > incoming.clock => {}
            Some(cur) if cur.clock == incoming.clock => {
                // Exact-clock tie, as for a mark: delete wins.
                if incoming.deleted && !cur.deleted {
                    into.insert(id.clone(), incoming.clone());
                }
            }
            _ => {
                into.insert(id.clone(), incoming.clone());
            }
        }
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
            blocked: vec![],
            seen: vec![],
            names: vec![],
            devices: vec![],
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
            blocked: vec![],
            seen: vec![SeenEntry {
                pubkey: "aaaa".into(),
                count: 2,
            }],
            names: vec![],
            devices: vec![],
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
            blocked: vec![],
            seen: vec![],
            names: vec![],
            devices: vec![],
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
                since: 0,
            }],
            trusted: vec![],
            blocked: vec![],
            seen: vec![SeenEntry {
                pubkey: "k".into(),
                count: 3,
            }],
            names: vec![],
            devices: vec![],
        });
        let b = SyncState::from_snapshot(&SyncSnapshot {
            lamport: 0,
            device: [0u8; 16],
            contacts: vec![],
            verified: vec![MarkEntry {
                pubkey: "k".into(),
                clock: lam(2, 2),
                deleted: true,
                since: 0,
            }],
            trusted: vec![],
            blocked: vec![],
            seen: vec![SeenEntry {
                pubkey: "k".into(),
                count: 5,
            }],
            names: vec![],
            devices: vec![],
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

    fn name_state(pubkey: &str, pinned: &str, pending: Option<&str>, clock: Lamport) -> SyncState {
        SyncState::from_snapshot(&SyncSnapshot {
            lamport: 0,
            device: [0u8; 16],
            contacts: vec![],
            verified: vec![],
            trusted: vec![],
            blocked: vec![],
            seen: vec![],
            names: vec![NameEntry {
                pubkey: pubkey.into(),
                pinned: pinned.into(),
                pending: pending.map(Into::into),
                clock,
                deleted: false,
            }],
            devices: vec![],
        })
    }

    #[test]
    fn names_last_writer_wins_and_commute() {
        // An approval of the new name (higher clock, pending cleared) wins over the
        // old pinned+pending, regardless of merge order.
        let a = name_state("k", "Lorenzo", Some("Lore"), lam(1, 1));
        let b = name_state("k", "Lore", None, lam(2, 2));

        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);

        assert_eq!(ab, ba, "name merge is commutative");
        assert_eq!(ab.names["k"].pinned, "Lore");
        assert_eq!(
            ab.names["k"].pending, None,
            "approval cleared the pending name"
        );
    }

    /// A helper that builds a name state carrying an explicit tombstone.
    fn name_tombstone(pubkey: &str, clock: Lamport) -> SyncState {
        SyncState::from_snapshot(&SyncSnapshot {
            lamport: 0,
            device: [0u8; 16],
            contacts: vec![],
            verified: vec![],
            trusted: vec![],
            blocked: vec![],
            seen: vec![],
            names: vec![NameEntry {
                pubkey: pubkey.into(),
                pinned: "Lorenzo".into(),
                pending: None,
                clock,
                deleted: true,
            }],
            devices: vec![],
        })
    }

    #[test]
    fn names_merge_is_idempotent() {
        let base = name_state("k", "Lorenzo", Some("Lore"), lam(1, 1));
        let other = name_state("k", "Lore", None, lam(2, 2));
        let mut once = base.clone();
        once.merge(&other);
        let mut twice = once.clone();
        twice.merge(&other);
        assert_eq!(once, twice, "re-merging the same name snapshot is a no-op");
    }

    #[test]
    fn names_converge_regardless_of_order() {
        // Three concurrent writers for the same id; the highest clock must win from
        // any application order (device id breaks the counter tie deterministically).
        let s1 = name_state("k", "One", None, lam(1, 1));
        let s2 = name_state("k", "Two", None, lam(2, 2));
        let s3 = name_state("k", "Three", None, lam(2, 3));

        let mut forward = s1.clone();
        forward.merge(&s2);
        forward.merge(&s3);
        let mut backward = s3.clone();
        backward.merge(&s2);
        backward.merge(&s1);

        assert_eq!(forward, backward, "any order → same state");
        // lam(2,3) is the max (counter tie broken by the larger device id).
        assert_eq!(forward.names["k"].pinned, "Three");
    }

    #[test]
    fn name_tombstone_wins_by_clock_and_can_be_superseded() {
        // A tombstone at a higher clock removes the name...
        let mut has = name_state("k", "Lorenzo", None, lam(1, 1));
        has.merge(&name_tombstone("k", lam(2, 1)));
        assert!(has.names["k"].deleted, "higher-clock tombstone wins");

        // ...but a still-higher re-observation revives it (last intent wins).
        has.merge(&name_state("k", "Lorenzo", Some("Lore"), lam(3, 1)));
        assert!(!has.names["k"].deleted, "later re-add beats the tombstone");
        assert_eq!(has.names["k"].pending.as_deref(), Some("Lore"));
    }

    #[test]
    fn names_are_keyed_independently_per_id() {
        // Two identities with different names must never bleed into each other.
        let mut a = name_state("k1", "Alice", None, lam(1, 1));
        let b = name_state("k2", "Bob", None, lam(1, 2));
        a.merge(&b);
        assert_eq!(a.names["k1"].pinned, "Alice");
        assert_eq!(a.names["k2"].pinned, "Bob");
    }

    fn device_state(rows: Vec<DeviceEntry>) -> SyncState {
        SyncState::from_snapshot(&SyncSnapshot {
            lamport: 0,
            device: [0u8; 16],
            contacts: vec![],
            verified: vec![],
            trusted: vec![],
            blocked: vec![],
            seen: vec![],
            names: vec![],
            devices: rows,
        })
    }

    /// A device row carries its own id, so the key and the clock's device agree.
    fn device_row(d: u8, name: &str, last_seen: u64, counter: u64, deleted: bool) -> DeviceEntry {
        DeviceEntry {
            id: device_key(&[d; 16]),
            name: name.into(),
            platform: "linux".into(),
            last_seen,
            clock: lam(counter, d),
            deleted,
        }
    }

    #[test]
    fn a_device_row_is_last_writer_wins_from_its_own_machine() {
        let mut a = device_state(vec![device_row(1, "laptop", 100, 1, false)]);
        a.merge(&device_state(vec![device_row(
            1,
            "the laptop",
            400,
            2,
            false,
        )]));
        assert_eq!(a.devices[&device_key(&[1u8; 16])].name, "the laptop");
        assert_eq!(a.devices[&device_key(&[1u8; 16])].last_seen, 400);

        // Two machines describing themselves are two rows, not a contest.
        a.merge(&device_state(vec![device_row(2, "desktop", 50, 1, false)]));
        assert_eq!(a.devices.len(), 2);
    }

    #[test]
    fn only_a_machine_may_describe_itself() {
        // The whole reason this register is not an ordinary LWW map. Device 2
        // writes a *live* row for device 1's id, with a clock high enough to win
        // every ordinary comparison — and is ignored, because a send aimed at
        // "laptop" must not be aimed at whoever last claimed the name.
        let forged = DeviceEntry {
            id: device_key(&[1u8; 16]),
            clock: lam(99, 2),
            name: "laptop".into(),
            platform: "linux".into(),
            last_seen: 999,
            deleted: false,
        };
        let mut a = device_state(vec![device_row(1, "laptop", 100, 1, false)]);
        a.merge(&device_state(vec![forged]));
        let row = &a.devices[&device_key(&[1u8; 16])];
        assert_eq!(row.last_seen, 100, "a foreign live row must not land");
        assert_eq!(row.clock, lam(1, 1));

        // A row nobody has ever seen is not created either.
        let mut empty = device_state(vec![]);
        empty.merge(&device_state(vec![DeviceEntry {
            id: device_key(&[7u8; 16]),
            clock: lam(1, 8),
            name: "invented".into(),
            platform: "linux".into(),
            last_seen: 1,
            deleted: false,
        }]));
        assert!(empty.devices.is_empty());
    }

    #[test]
    fn anyone_may_forget_a_device() {
        // The exemption that makes `forget` possible at all: you retire a machine
        // *from another machine*, usually because that one is gone for good.
        let mut a = device_state(vec![device_row(1, "laptop", 100, 1, false)]);
        a.merge(&device_state(vec![DeviceEntry {
            id: device_key(&[1u8; 16]),
            clock: lam(2, 2),
            name: String::new(),
            platform: String::new(),
            last_seen: 0,
            deleted: true,
        }]));
        assert!(
            a.devices[&device_key(&[1u8; 16])].deleted,
            "tombstone lands"
        );

        // And the machine itself can come back, because a tombstone is
        // bookkeeping and not a revocation — which is why the writing side has
        // to decline to republish (see the retirement check in the CLI).
        a.merge(&device_state(vec![device_row(1, "laptop", 500, 3, false)]));
        assert!(!a.devices[&device_key(&[1u8; 16])].deleted);
    }

    #[test]
    fn devices_converge_in_any_order() {
        let rows = [
            device_row(1, "laptop", 100, 1, false),
            device_row(1, "laptop renamed", 300, 4, false),
            device_row(2, "desktop", 200, 2, false),
        ];
        let mut forward = device_state(vec![]);
        for r in &rows {
            forward.merge(&device_state(vec![r.clone()]));
        }
        let mut backward = device_state(vec![]);
        for r in rows.iter().rev() {
            backward.merge(&device_state(vec![r.clone()]));
        }
        assert_eq!(forward, backward, "any order → same state");
        // Idempotent too.
        let once = forward.clone();
        forward.merge(&once);
        assert_eq!(forward, once);
    }
}
