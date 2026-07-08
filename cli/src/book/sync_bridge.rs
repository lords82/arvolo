use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::sync::{
    self, ContactReg, DeviceId, Lamport, MarkReg, NameReg, SyncSnapshot, SyncState,
};

use super::*;

pub(crate) fn sync_dir() -> PathBuf {
    config_dir().join("sync")
}

pub(crate) fn meta_path() -> PathBuf {
    sync_dir().join("meta.bin")
}

pub(crate) fn device_path() -> PathBuf {
    sync_dir().join("device.bin")
}

/// This device's local id (a random tiebreak for the Lamport clock, **not** the
/// shared identity). Minted and persisted on first use.
pub(crate) fn load_or_init_device() -> DeviceId {
    if let Ok(bytes) = std::fs::read(device_path()) {
        if bytes.len() == 16 {
            let mut id = [0u8; 16];
            id.copy_from_slice(&bytes);
            return id;
        }
    }
    let id = sync::random_device_id();
    std::fs::create_dir_all(sync_dir()).ok();
    let _ = write_private_bytes(&device_path(), &id);
    id
}

/// Load the CRDT sidecar: `(state, lamport, device)`. Missing/corrupt → empty.
pub(crate) fn load_meta() -> (SyncState, u64, DeviceId) {
    let device = load_or_init_device();
    match std::fs::read(meta_path())
        .ok()
        .and_then(|b| sync::decode_snapshot(&b).ok())
    {
        Some(snap) => (SyncState::from_snapshot(&snap), snap.lamport, device),
        None => (SyncState::default(), 0, device),
    }
}

pub(crate) fn save_meta(state: &SyncState, lamport: u64, device: DeviceId) -> Result<()> {
    std::fs::create_dir_all(sync_dir()).ok();
    let snap = state.to_snapshot(lamport, device);
    let bytes = sync::encode_snapshot(&snap).context("serialize sync meta")?;
    write_private_bytes(&meta_path(), &bytes).context("write sync meta")
}

pub(crate) fn tombstone_marks(
    state: &mut BTreeMap<String, MarkReg>,
    present: &BTreeSet<String>,
    lamport: &mut u64,
    device: DeviceId,
) {
    // A pubkey present in the ledger but not yet active in the sidecar → add.
    for pk in present {
        let need = !matches!(state.get(pk), Some(r) if !r.deleted);
        if need {
            *lamport += 1;
            state.insert(
                pk.clone(),
                MarkReg {
                    clock: Lamport {
                        counter: *lamport,
                        device,
                    },
                    deleted: false,
                },
            );
        }
    }
    // Active in the sidecar but no longer in the ledger → tombstone. This is what
    // carries `contact_add`'s key-change clearing (which already removed the old
    // pubkey from the verified/trusted TOMLs) into the CRDT and out to peers.
    let stale: Vec<String> = state
        .iter()
        .filter(|(pk, r)| !r.deleted && !present.contains(*pk))
        .map(|(pk, _)| pk.clone())
        .collect();
    for pk in stale {
        *lamport += 1;
        state.insert(
            pk.clone(),
            MarkReg {
                clock: Lamport {
                    counter: *lamport,
                    device,
                },
                deleted: true,
            },
        );
    }
}

/// Reconcile the sidecar with the current TOML ledgers and return a full snapshot
/// to publish to the user's other devices. Any TOML edit made without touching
/// the sidecar (e.g. via `contact_add`/`contact_remove`) is captured here as a
/// fresh-clock add or tombstone, so the published snapshot always reflects the
/// authoritative ledgers.
pub fn build_local_snapshot() -> Result<SyncSnapshot> {
    let (mut state, mut lamport, device) = load_meta();

    let contacts = load_contacts().contacts;
    // Adds / key-changes.
    for (name, pubkey) in &contacts {
        let changed =
            !matches!(state.contacts.get(name), Some(r) if !r.deleted && &r.pubkey == pubkey);
        if changed {
            lamport += 1;
            state.contacts.insert(
                name.clone(),
                ContactReg {
                    pubkey: pubkey.clone(),
                    clock: Lamport {
                        counter: lamport,
                        device,
                    },
                    deleted: false,
                },
            );
        }
    }
    // Contacts removed out-of-band → tombstone.
    let removed: Vec<(String, String)> = state
        .contacts
        .iter()
        .filter(|(name, r)| !r.deleted && !contacts.contains_key(*name))
        .map(|(name, r)| (name.clone(), r.pubkey.clone()))
        .collect();
    for (name, pubkey) in removed {
        lamport += 1;
        state.contacts.insert(
            name,
            ContactReg {
                pubkey,
                clock: Lamport {
                    counter: lamport,
                    device,
                },
                deleted: true,
            },
        );
    }

    // Marks: set-membership reconciliation (carries key-change clearing too).
    tombstone_marks(
        &mut state.verified,
        &load_verified().verified,
        &mut lamport,
        device,
    );
    tombstone_marks(
        &mut state.trusted,
        &load_trusted().trusted,
        &mut lamport,
        device,
    );

    // Seen counters: monotone max into the sidecar.
    for (pk, cnt) in load_seen().seen {
        let e = state.seen.entry(pk).or_insert(0);
        *e = (*e).max(cnt);
    }

    // Advertised names: last-writer-wins per id, tombstoning removals (same shape
    // as contacts). A row counts as present when it has a pinned or pending name.
    let names = load_names().names;
    for (id, row) in &names {
        let present = !row.pinned.is_empty() || row.pending.is_some();
        let changed = !matches!(state.names.get(id), Some(r)
            if !r.deleted && r.pinned == row.pinned && r.pending == row.pending);
        if present && changed {
            lamport += 1;
            state.names.insert(
                id.clone(),
                NameReg {
                    pinned: row.pinned.clone(),
                    pending: row.pending.clone(),
                    clock: Lamport {
                        counter: lamport,
                        device,
                    },
                    deleted: false,
                },
            );
        }
    }
    // Names removed out-of-band → tombstone.
    let name_removed: Vec<String> = state
        .names
        .iter()
        .filter(|(id, r)| {
            !r.deleted
                && !names
                    .get(*id)
                    .is_some_and(|row| !row.pinned.is_empty() || row.pending.is_some())
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in name_removed {
        lamport += 1;
        if let Some(r) = state.names.get_mut(&id) {
            r.deleted = true;
            r.clock = Lamport {
                counter: lamport,
                device,
            };
        }
    }

    save_meta(&state, lamport, device)?;
    Ok(state.to_snapshot(lamport, device))
}

/// Merge an incoming snapshot into the sidecar and re-project the merged state
/// into the four TOML ledgers. Idempotent and order-independent (CRDT).
pub fn apply_merged_state(incoming: &SyncSnapshot) -> Result<()> {
    // Fold local out-of-band edits in first so a merge never silently reverts an
    // un-published local change.
    build_local_snapshot()?;

    let (mut state, mut lamport, device) = load_meta();
    let incoming_state = SyncState::from_snapshot(incoming);
    state.merge(&incoming_state);
    lamport = lamport.max(incoming.lamport).max(state.max_counter());
    save_meta(&state, lamport, device)?;

    project_to_ledgers(&state)?;
    Ok(())
}

/// Project the merged CRDT state into the authoritative TOML ledgers: a
/// non-deleted register becomes a live row, a tombstone becomes absence.
pub(crate) fn project_to_ledgers(state: &SyncState) -> Result<()> {
    let mut c = Contacts::default();
    for (name, r) in &state.contacts {
        if !r.deleted {
            c.contacts.insert(name.clone(), r.pubkey.clone());
        }
    }
    save_contacts(&c)?;

    let mut v = Verified::default();
    for (pk, r) in &state.verified {
        if !r.deleted {
            v.verified.insert(pk.clone());
        }
    }
    save_verified(&v)?;

    let mut t = Trusted::default();
    for (pk, r) in &state.trusted {
        if !r.deleted {
            t.trusted.insert(pk.clone());
        }
    }
    save_trusted(&t)?;

    let mut s = Seen::default();
    for (pk, cnt) in &state.seen {
        s.seen.insert(pk.clone(), *cnt);
    }
    save_seen(&s)?;

    let mut n = Names::default();
    for (id, r) in &state.names {
        if !r.deleted {
            n.names.insert(
                id.clone(),
                NameRow {
                    pinned: r.pinned.clone(),
                    pending: r.pending.clone(),
                },
            );
        }
    }
    save_names(&n)?;
    Ok(())
}
