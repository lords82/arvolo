//! The address-book sync round, and the loop that runs it.
//!
//! Devices that share an identity share an inbox slot, and the address book rides
//! it as encrypted CRDT snapshots. The *commands* around this — pairing a device,
//! printing a QR, listing what is paired — stay with the CLI, because they are
//! conversations with a person. What is here is the part with nobody watching:
//! merge what the others published, publish ours, clean up.

use anyhow::{Context, Result};
use arvolo_core::presence::{decode_sync_note, encode_sync_note, InboxSubscription};
use arvolo_core::sync::{self, SyncNote};

use crate::book;
use crate::identity::my_identity;

/// TTL for a published sync-cell note. Long so the cell survives devices being
/// offline; refreshed on every publish. Within the relay's 30-day inbox cap.
const SYNC_NOTE_TTL_SECS: u64 = 7 * 24 * 3600;

/// How often the `listen`/`daemon` background loop runs a sync round. Each round
/// re-publishes the cell (refreshing its TTL) and merges peers' updates.
const AUTO_SYNC_SECS: u64 = 300;

/// The relay to sync through: the one asked for, else the configured default.
pub fn resolve_relay(relay: Option<String>) -> Result<String> {
    match relay {
        Some(r) => Ok(book::normalize_relay(&r)),
        None => book::default_relay_or_builtin()
            .context("no relay configured (pass --relay or set ARVOLO_RELAY / config `relay`)"),
    }
}

/// Spawn a background task that runs a sync round now and then every
/// [`AUTO_SYNC_SECS`]. Used by `listen`/`daemon` for automatic sync. The first
/// round fires immediately (interval's first tick is instant). Errors are logged
/// and retried on the next tick, never fatal. Abort the handle to stop it.
pub fn spawn_auto_sync(relay: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(AUTO_SYNC_SECS));
        loop {
            tick.tick().await;
            // `sync_round` directly, NOT the CLI's `sync now`: this loop runs
            // *inside* the daemon (and inside a standalone `listen`), and that
            // command routes through the daemon's IPC when one answers — which
            // from here would be the daemon calling itself.
            if let Err(e) = sync_round(Some(relay.clone())).await {
                tracing::debug!("auto-sync round failed: {e:#}");
            }
        }
    })
}

/// One round: merge every snapshot our other devices published, then publish the
/// current state. Returns how many snapshots were merged.
///
/// Separate from the command that prints the result because the daemon runs this
/// for a GUI that reports the outcome in a panel, and a `println!` from inside a
/// daemon goes nowhere.
pub async fn sync_round(relay: Option<String>) -> Result<usize> {
    let relay = resolve_relay(relay)?;
    let me = my_identity()?;
    let key = sync::snapshot_key(&me.secret_bytes());
    let sub = InboxSubscription::new(relay, &me);

    // 1. Pull: read the cell, merge every snapshot we can decrypt.
    let items = sub.raw_items(0).await.context("read inbox for sync")?;
    let mut merged = 0usize;
    let mut to_clear: Vec<String> = Vec::new();
    for item in &items {
        let Some(note) = decode_sync_note(&item.blob) else {
            continue; // an offer or junk — not ours to touch
        };
        match note {
            SyncNote::Snapshot { blob } => match sync::decrypt_snapshot(&key, &blob) {
                Ok(snap) => {
                    book::apply_merged_state(&snap).context("merge incoming snapshot")?;
                    merged += 1;
                    to_clear.push(item.id.clone());
                }
                // Tagged as a sync note but not decryptable with our key: not from
                // our devices (or corrupt). Safe to remove so it can't fill the slot.
                Err(_) => to_clear.push(item.id.clone()),
            },
            // The deposit-based fallback isn't produced yet (snapshots are inline).
            SyncNote::SnapshotRef { .. } => {}
        }
    }

    // 2. Push: publish a fresh full snapshot reflecting the merged state.
    let snapshot = book::build_local_snapshot()?;
    let blob = sync::encrypt_snapshot(&key, &snapshot)?;
    let body = encode_sync_note(&SyncNote::Snapshot { blob })?;
    sub.post_raw(body, Some(SYNC_NOTE_TTL_SECS))
        .await
        .context("publish snapshot")?;

    // 3. Cleanup: delete the notes we merged (we are the slot owner). Best-effort.
    for id in to_clear {
        let _ = sub.ack(&id).await;
    }

    // A merge is the one path that can orphan advertised-name records (a device
    // that removed the contact synced its deletion here). Sweep them now, so no
    // separate `contacts prune` chore exists. Best-effort, like the acks.
    if merged > 0 {
        if let Ok(n) = book::prune_orphan_names() {
            if n > 0 {
                tracing::debug!("pruned {n} orphan advertised-name record(s) after merge");
            }
        }
    }

    Ok(merged)
}
