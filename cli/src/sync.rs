//! Multi-device: pairing (`device pair`/`device join`) and address-book sync
//! (`sync now`/`sync status`).
//!
//! Devices share one identity (see the plan): pairing copies this device's
//! identity key + address book to a new device over the SPAKE2 rendezvous, so
//! afterwards both are the same "person" (one public id, one inbox slot) and any
//! device can open files sent to you. Address-book changes then ride the inbox as
//! encrypted CRDT snapshots.

use std::io::IsTerminal;

use anyhow::{anyhow, bail, Context, Result};
use arvolo_core::code;
use arvolo_core::crypto::Identity;
use arvolo_core::presence::{decode_sync_note, encode_sync_note, InboxSubscription};
use arvolo_core::sync::{self, PairPayload, SyncNote};

use crate::book;

/// TTL for a published sync-cell note. Long so the cell survives devices being
/// offline; refreshed on every publish. Within the relay's 30-day inbox cap.
const SYNC_NOTE_TTL_SECS: u64 = 7 * 24 * 3600;

/// How often the `listen`/`daemon` background loop runs a sync round. Each round
/// re-publishes the cell (refreshing its TTL) and merges peers' updates.
const AUTO_SYNC_SECS: u64 = 300;

/// Rendezvous value cap on the relay is 64 KiB; keep the inline pair payload
/// (identity secret + full book snapshot) comfortably under it.
const MAX_PAIR_PAYLOAD: usize = 60 * 1024;

fn resolve_relay(relay: Option<String>) -> Result<String> {
    match relay {
        Some(r) => Ok(book::normalize_relay(&r)),
        None => book::default_relay_or_builtin()
            .context("no relay configured (pass --relay or set ARVOLO_RELAY / config `relay`)"),
    }
}

/// `arvolo device pair` — on an existing device: publish a pairing code and, once
/// the new device connects, hand it this device's identity + address book.
pub async fn device_pair(relay: Option<String>, qr: bool) -> Result<()> {
    let relay = resolve_relay(relay)?;
    let me = crate::my_identity()?;
    let identity_secret: [u8; 32] = me
        .secret_bytes()
        .try_into()
        .map_err(|_| anyhow!("identity secret is not 32 bytes"))?;
    let snapshot = book::build_local_snapshot()?;
    let payload = PairPayload {
        identity_secret,
        snapshot,
    };
    let bytes = payload.encode()?;
    if bytes.len() > MAX_PAIR_PAYLOAD {
        bail!(
            "address book is too large to pair inline ({} KiB); the deposit-based \
             fallback is not implemented yet",
            bytes.len() / 1024
        );
    }

    // Whichever rendezvous the relay speaks. Pairing stays strictly one-shot
    // either way — this hands over the identity secret itself — but on a v2 relay
    // it gains key confirmation, so a joiner who mistypes the code is turned away
    // instead of being handed the sealed identity blob to fail on.
    let (code, sender) = code::publish_auto(&bytes, &relay, true)
        .await
        .context("start device pairing")?;

    println!("\nOn the new device, run:\n");
    println!("    arvolo device join {code}\n");
    if qr {
        crate::print_qr(&code);
    }
    eprintln!(
        "This shares THIS device's identity ({}) with the new one, so both act as the \
         same person. Anyone you paired before still sees a single id.",
        me.public().fingerprint()
    );
    eprintln!("Waiting for the new device… (Ctrl-C to cancel)");

    match sender {
        code::CodeSender::V1(complete) => complete.run().await.context("device pairing")?,
        code::CodeSender::V2(host) => {
            let opts = code::HostOpts {
                max_sessions: Some(1),
                ..code::HostOpts::default()
            };
            let reason = host
                .run(
                    &bytes,
                    &opts,
                    code::HostState::default(),
                    tokio_util::sync::CancellationToken::new(),
                    |_| {},
                    |_| {},
                )
                .await
                .context("device pairing")?;
            if reason != code::CloseReason::MaxSessions {
                bail!("device pairing did not complete ({reason:?})");
            }
        }
    }
    eprintln!("✓ New device linked — it now shares your identity and address book.");
    Ok(())
}

/// `arvolo device join <code>` — on the new device: fetch the shared identity +
/// address book and adopt them (replacing this device's own identity).
pub async fn device_join(code: String, yes: bool) -> Result<()> {
    let default_relay = book::default_relay_or_builtin();
    eprintln!("Pairing… (waiting for the other device)");
    let bytes = code::resolve_bytes(&code, default_relay.as_deref())
        .await
        .context("device pairing")?;
    let payload = PairPayload::decode(&bytes)?;
    let new_id = Identity::from_secret_bytes(&payload.identity_secret)
        .context("received identity is invalid")?;
    let path = crate::identity_path();

    // Overwriting the local identity is destructive: confirm unless it already
    // matches (idempotent re-join) or --yes is given.
    if path.exists() {
        let existing = Identity::load(&path).ok();
        let differs = existing
            .as_ref()
            .map(|e| e.public().to_bytes() != new_id.public().to_bytes())
            .unwrap_or(true);
        if differs {
            let old_fp = existing
                .as_ref()
                .map(|e| e.public().fingerprint())
                .unwrap_or_else(|| "unreadable".into());
            eprintln!("This device already has an identity: {old_fp}");
            eprintln!(
                "Joining REPLACES it with the shared identity {}. Files still sealed to the \
                 old identity would no longer be openable here.",
                new_id.public().fingerprint()
            );
            if !yes {
                if !std::io::stderr().is_terminal() {
                    bail!("refusing to overwrite the identity without --yes in a non-interactive shell");
                }
                if !crate::ui::confirm_blocking("Replace this device's identity?") {
                    eprintln!("Aborted — nothing changed.");
                    return Ok(());
                }
            }
        }
    }

    new_id.save(&path).context("save shared identity")?;
    book::apply_merged_state(&payload.snapshot).context("import address book")?;

    println!(
        "✓ Linked. This device now shares identity {} and {} contact(s).",
        new_id.public().fingerprint(),
        book::contact_list().len()
    );
    Ok(())
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
            if let Err(e) = sync_now(Some(relay.clone()), true).await {
                tracing::debug!("auto-sync round failed: {e:#}");
            }
        }
    })
}

/// `arvolo device status` — a quick read-only summary of the sync state.
pub async fn sync_status(json: bool) -> Result<()> {
    let me = crate::my_identity()?;
    if json {
        let v = serde_json::json!({
            "fingerprint": me.public().fingerprint(),
            "contacts": book::contact_list().len(),
            "sync_enabled": book::sync_enabled(),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    println!("identity:  {}", me.public().fingerprint());
    println!("contacts:  {}", book::contact_list().len());
    println!(
        "sync:      {}",
        if book::sync_enabled() {
            "on (rides listen/daemon; run one round now with `arvolo device sync`)"
        } else {
            "off (config `sync = false`)"
        }
    );
    Ok(())
}

/// `arvolo sync now` — one round of the mutable-cell protocol on our shared inbox
/// slot: pull and merge any snapshots our other devices published, publish a fresh
/// full snapshot of the merged book, then delete the notes we merged (writer-side
/// cleanup, so the slot tends to a single current snapshot).
pub async fn sync_now(relay: Option<String>, quiet: bool) -> Result<()> {
    let merged = sync_round(relay).await?;
    if !quiet {
        println!("✓ Synced ({merged} update(s) merged from your other devices).");
    }
    Ok(())
}

/// The round itself, without a word of output: pull and merge every snapshot our
/// other devices published, publish a fresh full snapshot of the merged book, then
/// delete the notes we merged (writer-side cleanup, so the slot tends to a single
/// current snapshot). Returns how many snapshots were merged.
///
/// Split out from [`sync_now`] because the daemon runs this for a GUI that reports
/// the outcome in a panel, and a `println!` from inside a daemon goes nowhere.
pub async fn sync_round(relay: Option<String>) -> Result<usize> {
    let relay = resolve_relay(relay)?;
    let me = crate::my_identity()?;
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
