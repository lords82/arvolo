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
use arvolo_core::sync::{self, PairPayload};

use crate::book;

// The round itself, the loop that runs it and the relay it runs through moved to
// `arvolo-engine`: they happen with nobody watching, which is exactly what a
// phone needs and what this file — full of prompts and QR codes — is not.
pub use arvolo_engine::sync::{resolve_relay, spawn_auto_sync, sync_round};

/// Rendezvous value cap on the relay is 64 KiB; keep the inline pair payload
/// (identity secret + full book snapshot) comfortably under it.
const MAX_PAIR_PAYLOAD: usize = 60 * 1024;

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
    let (code, sender) = code::publish_auto(&bytes, &relay)
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
    // The exchange happened: from here on this identity lives on more than one
    // device, and `send --to me` must stop offering to pair what is already paired.
    book::mark_paired();
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
    // Both halves mark it, and this half is the one that would otherwise take
    // longest to find out: a device that has just joined has nothing of anyone
    // else's authorship in its sidecar until the first sync round lands.
    book::mark_paired();

    println!(
        "✓ Linked. This device now shares identity {} and {} contact(s).",
        new_id.public().fingerprint(),
        book::contact_list().len()
    );
    Ok(())
}

/// `arvolo device status` — a quick read-only summary of the sync state.
///
/// With a daemon running, its own record is the one that means something: it is
/// the process actually running the rounds, so it knows when the last one
/// happened and whether it failed. Without one, the local files are all there is.
pub async fn sync_status(json: bool) -> Result<()> {
    if let Some(mut client) = crate::commands::daemon::daemon_client().await {
        let s = client.sync_status().await?;
        if json {
            println!("{}", serde_json::to_string_pretty(&s)?);
            return Ok(());
        }
        println!("identity:  {}", s.fingerprint);
        println!("contacts:  {}", s.contacts);
        if !s.enabled {
            println!("sync:      off (config `sync = false`)");
        } else if s.last_sync == 0 {
            println!("sync:      on — no round since the daemon started");
        } else {
            let ago = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().saturating_sub(s.last_sync))
                .unwrap_or(0);
            println!(
                "sync:      on — last round {} ago, {} update(s) merged",
                crate::human_duration(ago),
                s.last_merged
            );
        }
        if !s.last_error.is_empty() {
            eprintln!("           last error: {}", s.last_error);
        }
        return Ok(());
    }
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

/// `arvolo device list` — the machines this identity knows about.
///
/// Goes through the daemon when one is running, for the same reason `sync now`
/// does: the register is projected by the sync round, and the daemon is the one
/// running those. Without a daemon it reads the ledger directly, which is the
/// same file, just possibly a round out of date.
pub async fn device_list(json: bool) -> Result<()> {
    let mine = sync::device_key(&book::load_or_init_device());
    let rows: Vec<(String, book::DeviceRow)> = match crate::commands::daemon::daemon_client().await
    {
        Some(mut client) => client
            .list_devices()
            .await?
            .into_iter()
            .map(|d| {
                (
                    d.id,
                    book::DeviceRow {
                        name: d.name,
                        platform: d.platform,
                        last_seen: d.last_seen,
                    },
                )
            })
            .collect(),
        None => book::load_devices().devices.into_iter().collect(),
    };

    if json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, r)| {
                serde_json::json!({
                    "id": id,
                    "name": r.name,
                    "platform": r.platform,
                    "last_seen": r.last_seen,
                    "this_device": *id == mine,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!(
            "No devices recorded yet. Each machine adds itself on its first sync round, \n\
             so this fills in once `arvolo device pair` has been through one."
        );
        return Ok(());
    }
    for (id, r) in &rows {
        let short = &id[..8.min(id.len())];
        let here = if *id == mine { "  (this device)" } else { "" };
        // "seen", not "online": it is that machine's own clock, written once per
        // sync round, so it lags by up to five minutes and cannot tell a machine
        // that has just gone to sleep from one that is still awake.
        let seen = if r.last_seen == 0 {
            "never synced".to_string()
        } else {
            // Clamped to now: the stamp is that machine's own clock, and two
            // machines that disagree about the time would otherwise print a
            // sighting in the future.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!(
                "seen {} ago",
                crate::human_duration(now.saturating_sub(r.last_seen))
            )
        };
        println!("{:<20} {short}  {:<8} {seen}{here}", r.name, r.platform);
    }
    Ok(())
}

/// `arvolo device rename` — name this machine. Only this one; see the command.
pub async fn device_rename(name: String) -> Result<()> {
    if let Some(mut client) = crate::commands::daemon::daemon_client().await {
        client.rename_this_device(name.clone()).await?;
    } else {
        let key = sync::device_key(&book::load_or_init_device());
        let mut devices = book::load_devices();
        let row = devices.devices.entry(key).or_default();
        row.name = crate::ui::sanitize_display(name.trim());
        if row.platform.is_empty() {
            row.platform = std::env::consts::OS.to_string();
        }
        book::save_devices(&devices)?;
    }
    println!("✓ This device is now called {name}. Your other devices see it after the next sync.");
    Ok(())
}

/// `arvolo device forget` — retire a machine from the register.
pub async fn device_forget(who: String) -> Result<()> {
    if let Some(mut client) = crate::commands::daemon::daemon_client().await {
        client.forget_device(who.clone()).await?;
    } else {
        let id = book::resolve_device(&who)?;
        anyhow::ensure!(
            id != book::load_or_init_device(),
            "this is the device you are on — forgetting it here would only make it \
             announce itself again"
        );
        book::forget_device(&id)?;
    }
    println!(
        "✓ {who} is out of the register. It still holds this identity — forgetting is \n\
         bookkeeping, not revocation — but if it is running it will stop announcing itself."
    );
    Ok(())
}

/// `arvolo sync now` — one round of the mutable-cell protocol on our shared inbox
/// slot: pull and merge any snapshots our other devices published, publish a fresh
/// full snapshot of the merged book, then delete the notes we merged (writer-side
/// cleanup, so the slot tends to a single current snapshot).
pub async fn sync_now(relay: Option<String>, quiet: bool) -> Result<()> {
    // Through the daemon when one runs: its auto-sync loop shares the same inbox
    // cell, and two processes running rounds concurrently race on the slot. The
    // daemon also records the outcome, so the GUI's panel and `device status`
    // agree on when the last round happened.
    if let Some(mut client) = crate::commands::daemon::daemon_client().await {
        let s = client.sync_now().await?;
        if !quiet {
            println!(
                "✓ Synced via the daemon ({} update(s) merged from your other devices).",
                s.last_merged
            );
        }
        return Ok(());
    }
    let merged = sync_round(relay).await?;
    if !quiet {
        println!("✓ Synced ({merged} update(s) merged from your other devices).");
    }
    Ok(())
}
