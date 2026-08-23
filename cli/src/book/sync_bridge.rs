use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::sync::{
    self, ContactReg, DeviceId, DeviceReg, Lamport, MarkReg, NameReg, RelayReg, SyncSnapshot,
    SyncState,
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

pub(crate) fn paired_marker_path() -> PathBuf {
    sync_dir().join("paired")
}

/// Remember that this identity now lives on more than one device. Written by both
/// halves of `device pair`/`device join`, once the exchange has actually happened.
pub(crate) fn mark_paired() {
    std::fs::create_dir_all(sync_dir()).ok();
    let _ = std::fs::write(paired_marker_path(), b"");
}

/// Does another device share this identity, as far as we can tell?
///
/// Only ever used to decide whether to *suggest* pairing, never to refuse
/// anything — it is evidence, not bookkeeping. The device register would often
/// answer it outright, but not always (a machine that has never completed a sync
/// round has no row), so this keeps reading traces instead:
///
/// 1. the marker [`mark_paired`] leaves when a pairing completes;
/// 2. failing that — for an identity paired before the marker existed — a CRDT
///    entry authored by a device id that is not ours, which only a merged
///    snapshot from another device can produce. A device row is one such entry,
///    and the most direct of them: another machine saying so itself.
///
/// Both can be false negatives (a fresh pairing that has not synced, on a build
/// that predates the marker), which is exactly why the caller only adds a hint.
pub(crate) fn paired_with_another_device() -> bool {
    if paired_marker_path().exists() {
        return true;
    }
    let (state, _, mine) = load_meta();
    state.authored_by_another_device(&mine)
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
    present: &Marks,
    lamport: &mut u64,
    device: DeviceId,
) {
    // A pubkey present in the ledger but not yet active in the sidecar → add,
    // carrying the ledger's own "marked at" so the stamp reaches other devices
    // instead of being reset to whenever this reconciliation happened to run.
    for (pk, since) in present {
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
                    since: *since,
                },
            );
        }
    }
    // Active in the sidecar but no longer in the ledger → tombstone. This is what
    // carries `contact_add`'s key-change clearing (which already removed the old
    // pubkey from the verified/trusted TOMLs) into the CRDT and out to peers.
    let stale: Vec<String> = state
        .iter()
        .filter(|(pk, r)| !r.deleted && !present.contains_key(*pk))
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
                // A tombstone has nothing to be "since": the mark is gone.
                since: 0,
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
    tombstone_marks(
        &mut state.blocked,
        &load_blocked().blocked,
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

    // Contact relays: same shape again — last-writer-wins per id, tombstoning a
    // row cleared out of band. Empty is not a value here, it is an absence: the
    // ledger only ever holds relays somebody set or a sender advertised.
    let relays = load_relays().relays;
    for (id, relay) in &relays {
        let changed = !matches!(state.relays.get(id), Some(r) if !r.deleted && &r.relay == relay);
        if changed {
            lamport += 1;
            state.relays.insert(
                id.clone(),
                RelayReg {
                    relay: relay.clone(),
                    clock: Lamport {
                        counter: lamport,
                        device,
                    },
                    deleted: false,
                },
            );
        }
    }
    let relay_removed: Vec<String> = state
        .relays
        .iter()
        .filter(|(id, r)| !r.deleted && !relays.contains_key(*id))
        .map(|(id, _)| id.clone())
        .collect();
    for id in relay_removed {
        lamport += 1;
        if let Some(r) = state.relays.get_mut(&id) {
            r.deleted = true;
            r.clock = Lamport {
                counter: lamport,
                device,
            };
        }
    }

    register_this_device(&mut state, &mut lamport, device);

    save_meta(&state, lamport, device)?;
    Ok(state.to_snapshot(lamport, device))
}

/// Forget a device: out of the ledger here, tombstoned in the sidecar so the
/// removal reaches the other machines.
///
/// Both, in one place, because they say different things and *each is useless
/// alone*. The ledger is what this machine shows; a row dropped from it and
/// nowhere else comes back on the next merge. The sidecar is what this machine
/// tells the others; a tombstone written there and nowhere else leaves the row
/// on screen until something re-projects. Two call sites did the pair by hand
/// until a test caught one of them doing half of it.
pub(crate) fn forget_device(id: &DeviceId) -> Result<()> {
    let key = sync::device_key(id);
    let mut ledger = load_devices();
    if ledger.devices.remove(&key).is_some() {
        save_devices(&ledger)?;
    }
    tombstone_device(id)
}

/// The travelling half of [`forget_device`]. Separate only so the doc above can
/// say why it is never the whole job.
fn tombstone_device(id: &DeviceId) -> Result<()> {
    let (mut state, mut lamport, device) = load_meta();
    let key = sync::device_key(id);
    let prev = state.devices.get(&key).cloned();
    lamport += 1;
    state.devices.insert(
        key,
        DeviceReg {
            // Kept rather than blanked: a tombstone that also erased the name
            // would leave "forgotten" with nothing to say it was ever a machine
            // called anything, and the row is what a later "it came back" is
            // compared against.
            name: prev.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
            platform: prev
                .as_ref()
                .map(|r| r.platform.clone())
                .unwrap_or_default(),
            last_seen: prev.as_ref().map(|r| r.last_seen).unwrap_or(0),
            clock: Lamport {
                counter: lamport,
                device,
            },
            deleted: true,
        },
    );
    save_meta(&state, lamport, device)
}

/// Announce this machine in the device register, or decline to.
///
/// Two halves, and the second is the one worth reading. The first is ordinary:
/// write our own row when it is missing, when the name in `devices.toml` has been
/// edited here, or when `last_seen` has moved by more than a minute.
///
/// The second is what makes `arvolo device forget` mean anything. A tombstone is
/// bookkeeping, not revocation — the forgotten machine still holds the seed and
/// still syncs — so on its next round it would find its row deleted, write a
/// fresh one with a higher clock, and reappear within five minutes of being
/// removed. Finding *our own* row tombstoned, we take it as being retired and
/// stop announcing ourselves rather than argue with it.
///
/// That works because these are the user's own machines running the user's own
/// software, and it is worth being plain that this is the only reason it works:
/// anything holding the seed can ignore this and re-register. It is not a
/// security boundary and must never be described as one — see `docs/PROTOCOL.md`.
/// It is enough for what it is for, which is tidying up after a machine you no
/// longer use.
fn register_this_device(state: &mut SyncState, lamport: &mut u64, device: DeviceId) {
    let key = sync::device_key(&device);
    if matches!(state.devices.get(&key), Some(r) if r.deleted) {
        return;
    }

    let now = quantize_last_seen(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    // The name is the one thing here a user edits, and they edit it in the
    // ledger — so the ledger wins over the sidecar, exactly as it does for a
    // contact renamed with `arvolo contacts`.
    let ledger = load_devices();
    let name = ledger
        .devices
        .get(&key)
        .map(|r| r.name.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(default_device_name);
    let platform = std::env::consts::OS.to_string();

    let changed = !matches!(state.devices.get(&key), Some(r)
        if !r.deleted && r.name == name && r.platform == platform && r.last_seen >= now);
    if !changed {
        return;
    }
    *lamport += 1;
    state.devices.insert(
        key.clone(),
        DeviceReg {
            name: name.clone(),
            platform: platform.clone(),
            last_seen: now,
            clock: Lamport {
                counter: *lamport,
                device,
            },
            deleted: false,
        },
    );

    // Into the ledger as well, not only the sidecar. The ledger is what every
    // reader of the register uses — `device list`, the daemon's `ListDevices`,
    // the screen — and it is otherwise only written by a *merge*. A machine on
    // its own never merges anything, so without this it would not appear in its
    // own device list until its second sync round pulled back the note its first
    // one published: five minutes of a screen saying no machine had announced
    // itself, on the machine doing the announcing.
    let mut ledger = ledger;
    let row = ledger.devices.entry(key).or_default();
    row.name = name;
    row.platform = platform;
    row.last_seen = now;
    let _ = save_devices(&ledger);
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
            v.verified.insert(pk.clone(), r.since);
        }
    }
    save_verified(&v)?;

    let mut t = Trusted::default();
    for (pk, r) in &state.trusted {
        if !r.deleted {
            t.trusted.insert(pk.clone(), r.since);
        }
    }
    save_trusted(&t)?;

    let mut b = Blocked::default();
    for (pk, r) in &state.blocked {
        if !r.deleted {
            b.blocked.insert(pk.clone(), r.since);
        }
    }
    save_blocked(&b)?;

    // The register, minus the tombstones: a forgotten machine leaves the ledger
    // entirely, and the tombstone survives only in the sidecar, where it has to
    // stay so the removal keeps propagating.
    let mut d = Devices::default();
    for (id, r) in &state.devices {
        if !r.deleted {
            d.devices.insert(
                id.clone(),
                DeviceRow {
                    name: r.name.clone(),
                    platform: r.platform.clone(),
                    last_seen: r.last_seen,
                },
            );
        }
    }
    save_devices(&d)?;

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

    let mut rl = Relays::default();
    for (id, r) in &state.relays {
        if !r.deleted {
            rl.relays.insert(id.clone(), r.relay.clone());
        }
    }
    save_relays(&rl)?;
    Ok(())
}
