use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use arvolo_core::code;
use arvolo_core::flow::{self, ChunkSource, RecvEvent};
use arvolo_core::manager::{Direction, ManagerEvent, TransferManager};
use arvolo_core::transfer::RelayChoice;
use indicatif::{ProgressBar, ProgressStyle};

use crate::{book, history, sync};

#[cfg(unix)]
use crate::ipc;

use crate::output::{vprintln, vvprintln};

use crate::ui::*;
use crate::util::*;

#[cfg(unix)]
use crate::commands::daemon::{daemon_client, daemon_events};
use crate::commands::offline::recv_offline;
use crate::output::verbosity;

/// Ask the user y/n on stdin (blocking), defaulting to no on EOF/error.
pub(crate) async fn confirm(prompt: String) -> bool {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        eprint!("{prompt} [y/N] ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return false;
        }
        matches!(line.trim(), "y" | "Y" | "yes" | "YES")
    })
    .await
    .unwrap_or(false)
}

pub(crate) async fn listen(
    download_dir: Option<PathBuf>,
    relay: Option<String>,
    use_http: bool,
    auto_accept_contacts: bool,
    auto_accept_verified: bool,
    yes: bool,
    no_sync: bool,
) -> Result<()> {
    // If a daemon is already receiving, attach to it as a viewer/approver instead
    // of standing up a second engine (which would fight over presence/inbox).
    #[cfg(unix)]
    {
        if let Some(client) = daemon_client().await {
            // Say so. Two different things happen under one verb depending on
            // what else is running, and a user who can't tell which one they got
            // can't explain the difference in behaviour they're about to see.
            eprintln!("(a daemon is already listening — attaching to it as the approver)");
            if download_dir.is_some() {
                eprintln!(
                    "note: --download-dir is ignored when attaching to the daemon \
                     (it saves to its own configured dir)."
                );
            }
            return listen_attached(client, auto_accept_contacts, auto_accept_verified, yes).await;
        }
    }

    let relay = require_relay(relay, use_http)?;
    let me = my_identity()?;
    let my_id = encode_id(&me.public());
    let download_dir = download_dir
        .or_else(book::default_download_dir)
        .unwrap_or_else(book::default_home_downloads);

    let manager = TransferManager::new(me, Some(relay.clone()), download_dir.clone());
    let mut events = manager.subscribe();
    let inbox = manager.spawn_inbox()?;
    let auto_sync =
        (!no_sync && book::sync_enabled()).then(|| sync::spawn_auto_sync(relay.clone()));
    if auto_sync.is_some() {
        vprintln!("multi-device address-book sync enabled");
    }
    vprintln!("inbox poller started — publishing presence and watching for offers on the relay");
    if yes {
        vprintln!("auto-accepting ALL offers (--yes)");
    } else if auto_accept_verified {
        vprintln!("auto-accepting offers from verified contacts only");
    } else if auto_accept_contacts {
        vprintln!("auto-accepting offers from saved contacts");
    }

    eprintln!("Listening as {my_id}");
    eprintln!("Fingerprint: {}", manager.public_id().fingerprint());
    eprintln!("Saving accepted files to {}", download_dir.display());
    eprintln!("Relay: {relay}");
    eprintln!("Ctrl-C to stop.\n");

    let cancel = cancel_on_ctrl_c();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            ev = events.recv() => {
                let ev = match ev {
                    Ok(ev) => ev,
                    // Dropped some events under load — keep going.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                match ev {
                    ManagerEvent::OfferReceived { id, from, name, size, note, sender_name } => {
                        let from_b32 = encode_id(&from);
                        // Same as the daemon: a blocked sender never reaches the
                        // prompt, or blocking would just be a slower "no".
                        if book::is_blocked(&from_b32) {
                            manager.reject_offer(&id).await;
                            continue;
                        }
                        let status = book::sender_status(&from_b32);
                        eprintln!("\n📨 Incoming file offer:");
                        eprintln!("   from: {}{}", status.name.clone().unwrap_or_else(|| from_b32.clone()),
                                  if status.verified { " ✓ verified" } else { "" });
                        eprintln!("   fingerprint: {}", from.fingerprint());
                        eprintln!("   file: {}  ({})", sanitize_display(&name), human_size(size));
                        if !note.is_empty() {
                            eprintln!("   💬 note: {}", sanitize_display(&note));
                        }
                        note_advertised_name(&from_b32, &sender_name);

                        // `trusted` comes first and is unconditional: it is the
                        // standing "don't ask me about this person" the user set
                        // with `contacts trust`. Leaving it out here is why the
                        // same `listen` used to behave differently depending on
                        // whether a daemon happened to be up — the daemon-attached
                        // path honours it, this one didn't.
                        let accept = if status.trusted {
                            eprintln!("   ⬇ auto-downloading: trusted contact");
                            true
                        } else if yes {
                            true
                        } else if auto_accept_verified && status.verified {
                            eprintln!("   (auto-accepting: verified contact)");
                            true
                        } else if auto_accept_contacts && status.name.is_some() {
                            eprintln!("   (auto-accepting: saved contact)");
                            true
                        } else {
                            confirm(format!("   Accept from {}?", status.name.unwrap_or(from_b32))).await
                        };

                        if accept {
                            match manager.accept_offer(&id, None).await {
                                Ok(_) => eprintln!("   ✓ accepted — downloading…"),
                                Err(e) => eprintln!("   ✗ could not accept: {e:#}"),
                            }
                        } else {
                            manager.reject_offer(&id).await;
                            eprintln!("   ✗ rejected");
                        }
                    }
                    ManagerEvent::Started { direction: Direction::Recv, name, total_size, .. } => {
                        eprintln!("↓ receiving {} ({})", sanitize_display(&name), human_size(total_size));
                    }
                    ManagerEvent::Completed { id, path } => {
                        if let Some(p) = &path {
                            eprintln!("✓ saved {}", p.display());
                        }
                        record_history(&manager, id, "completed");
                    }
                    ManagerEvent::Failed { id, error } => {
                        eprintln!("✗ transfer failed: {error}");
                        record_history(&manager, id, &format!("failed: {error}"));
                    }
                    ManagerEvent::Cancelled { id } => record_history(&manager, id, "cancelled"),
                    _ => {}
                }
            }
        }
    }
    inbox.cancel();
    if let Some(h) = auto_sync {
        h.abort();
    }
    Ok(())
}

/// How an attached `listen` decides whether to auto-accept an incoming offer.
#[cfg(unix)]
#[derive(Clone, Copy)]
pub(crate) struct AcceptPolicy {
    pub(crate) auto_accept_contacts: bool,
    pub(crate) auto_accept_verified: bool,
    pub(crate) yes: bool,
}

/// Decide + act on one offer over the daemon IPC.
#[cfg(unix)]
pub(crate) async fn handle_attached_offer(
    client: &mut ipc::client::DaemonClient,
    offer: ipc::protocol::OfferDto,
    policy: AcceptPolicy,
) {
    let status = book::sender_status(&offer.from);
    let who = status.name.clone().unwrap_or_else(|| offer.from.clone());
    // Trusted senders are auto-downloaded by the daemon itself; don't also prompt
    // for them here (the daemon already accepted or will).
    if status.trusted {
        // Record any advertised-name change silently (it's surfaced later in
        // `contacts list`); never block or prompt for a trusted sender.
        book::observe_advertised_name(&offer.from, &offer.sender_name);
        eprintln!(
            "⬇ auto-downloading {} from trusted {who}",
            sanitize_display(&offer.name)
        );
        return;
    }
    eprintln!("\n📨 Incoming file offer:");
    eprintln!(
        "   from: {who}{}",
        if status.verified { " ✓ verified" } else { "" }
    );
    eprintln!(
        "   file: {}  ({})",
        sanitize_display(&offer.name),
        human_size(offer.size)
    );
    if !offer.note.is_empty() {
        eprintln!("   💬 note: {}", sanitize_display(&offer.note));
    }
    note_advertised_name(&offer.from, &offer.sender_name);

    let accept = if policy.yes {
        true
    } else if policy.auto_accept_verified && status.verified {
        eprintln!("   (auto-accepting: verified contact)");
        true
    } else if policy.auto_accept_contacts && status.name.is_some() {
        eprintln!("   (auto-accepting: saved contact)");
        true
    } else {
        confirm(format!("   Accept from {who}?")).await
    };

    if accept {
        match client.accept(offer.id, None).await {
            Ok(tid) => eprintln!("   ✓ accepted — downloading (transfer {tid})…"),
            Err(e) => eprintln!("   ✗ could not accept: {e:#}"),
        }
    } else {
        match client.reject(offer.id).await {
            Ok(()) => eprintln!("   ✗ rejected"),
            Err(e) => eprintln!("   ✗ could not reject: {e:#}"),
        }
    }
}

#[cfg(unix)]
pub(crate) async fn listen_attached(
    mut client: ipc::client::DaemonClient,
    auto_accept_contacts: bool,
    auto_accept_verified: bool,
    yes: bool,
) -> Result<()> {
    use ipc::protocol::{EventDto, OfferDto};

    let policy = AcceptPolicy {
        auto_accept_contacts,
        auto_accept_verified,
        yes,
    };

    let st = client.status().await?;
    eprintln!("Attached to daemon {}", st.public_id);
    eprintln!("Relay: {}", st.relay.as_deref().unwrap_or("-"));
    eprintln!("Ctrl-C to detach (the daemon keeps receiving).\n");

    // Drain any offers already parked before we attached.
    for o in client.list_pending().await? {
        handle_attached_offer(&mut client, o, policy).await;
    }

    let mut events = daemon_events().await?;
    let cancel = cancel_on_ctrl_c();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            ev = events.next() => {
                match ev {
                    Ok(Some(EventDto::OfferReceived { id, from, name, size, note, sender_name })) => {
                        handle_attached_offer(
                            &mut client,
                            OfferDto { id, from, name, size, note, sender_name },
                            policy,
                        ).await;
                    }
                    Ok(Some(EventDto::Completed { path: Some(p), .. })) => {
                        eprintln!("✓ saved {p}");
                    }
                    Ok(Some(EventDto::Failed { error, .. })) => {
                        eprintln!("✗ transfer failed: {error}");
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        eprintln!("(daemon closed the connection)");
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(())
}

/// Persist a finished transfer to the history store (best-effort).
pub(crate) fn record_history(manager: &TransferManager, id: u64, status: &str) {
    let Some(t) = manager.get(id) else { return };
    let direction = match t.direction {
        Direction::Send => "send",
        Direction::Recv => "recv",
    };
    let peer_id = t.peer.as_ref().map(encode_id);
    let _ = history::record(
        direction,
        peer_id,
        &t.name,
        t.total_size,
        t.transferred,
        status,
    );
}

pub(crate) async fn recv(
    ticket: String,
    out: Option<PathBuf>,
    password: Option<String>,
) -> Result<()> {
    // An offline/mailbox ticket (arvm…) is fetched + decrypted from the relay;
    // pairing codes and P2P tickets (arvc…) take the live chunked route. Detect
    // by trying to decode it as an offline ticket (codes never do).
    if !code::looks_like_code(&ticket)
        && arvolo_core::offline::OfflineTicket::decode(&ticket).is_ok()
    {
        return recv_offline(ticket, out, password).await;
    }
    if password.is_some() {
        eprintln!("note: --password applies only to an offline (arvm…) ticket — ignoring it here.");
    }

    // A short pairing code is resolved to the real ticket over a rendezvous first.
    let ticket = if code::looks_like_code(&ticket) {
        eprintln!("Pairing… (waiting for the sender)");
        let default_relay = book::default_relay_or_builtin();
        vprintln!(
            "input looks like a pairing code — resolving to a ticket over rendezvous relay {}",
            default_relay.as_deref().unwrap_or("(embedded in code)")
        );
        code::resolve_code(&ticket, default_relay.as_deref())
            .await
            .context("pairing")?
    } else {
        vprintln!(
            "input is a full ticket — connecting to the sender P2P (relay-assisted if needed)"
        );
        ticket
    };
    // Our identity is needed to open a ticket sealed to us (--to); harmless
    // otherwise (created on first use).
    let me = my_identity()?;

    let cancel = cancel_on_ctrl_c();
    let tty = std::io::stderr().is_terminal();
    let bar: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
    let b = bar.clone();
    // Last chunk source we narrated at -vv (0 = none, 1 = sender, 2 = relay), so
    // we log only when delivery flips between P2P and the relay, not every chunk.
    let last_src = Arc::new(AtomicU8::new(0));
    flow::recv_chunked(
        &ticket,
        out,
        Some(&me),
        RelayChoice::from_env(),
        cancel,
        move |ev| {
            let mut slot = b.lock().unwrap();
            match ev {
                RecvEvent::Sender { id } => {
                    print_sender_banner(id.as_deref());
                }
                RecvEvent::Started {
                    total,
                    resuming_from,
                    total_size,
                    resumed_bytes,
                } => {
                    let head = if resuming_from > 0 {
                        format!("resuming from chunk {resuming_from}/{total}")
                    } else {
                        format!("fetching {total} chunks")
                    };
                    if tty {
                        let pb = ProgressBar::new(total_size);
                        pb.set_style(
                        ProgressStyle::with_template(
                            "{spinner} {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta}) {msg}",
                        )
                        .unwrap(),
                    );
                        pb.set_position(resumed_bytes);
                        pb.set_message(head);
                        pb.enable_steady_tick(Duration::from_millis(120));
                        *slot = Some(pb);
                    } else {
                        eprintln!("{head}…");
                    }
                }
                RecvEvent::Control { connected } => {
                    let msg = format!(
                        "control channel to sender: {}",
                        if connected {
                            "connected"
                        } else {
                            "unavailable"
                        }
                    );
                    match slot.as_ref() {
                        Some(pb) => pb.println(msg),
                        None => eprintln!("{msg}"),
                    }
                }
                RecvEvent::Chunk {
                    index,
                    total,
                    source,
                    bytes,
                } => {
                    let src = match source {
                        ChunkSource::Relay => "relay",
                        ChunkSource::Sender => "sender",
                    };
                    // At -vv, announce only when the source flips (P2P ↔ relay).
                    let code = if matches!(source, ChunkSource::Relay) {
                        2
                    } else {
                        1
                    };
                    if last_src.swap(code, Ordering::Relaxed) != code {
                        let line =
                            format!("chunk {}/{total}: now pulling from the {src}", index + 1);
                        match slot.as_ref() {
                            Some(pb) if verbosity() >= 2 => pb.println(format!("·· {line}")),
                            _ => vvprintln!("{line}"),
                        }
                    }
                    if let Some(pb) = slot.as_ref() {
                        pb.inc(bytes);
                        pb.set_message(format!("chunk {}/{total} from {src}", index + 1));
                    }
                }
                RecvEvent::Warning { message } => match slot.as_ref() {
                    Some(pb) => pb.println(message),
                    None => eprintln!("{message}"),
                },
                RecvEvent::Paused { reason } => {
                    // Not a failure: the partial download + sidecar are kept, so
                    // re-running `receive` resumes. Clear the bar and say why.
                    if let Some(pb) = slot.take() {
                        pb.finish_and_clear();
                    }
                    eprintln!("Paused: {reason}");
                }
                RecvEvent::Saved { path } => {
                    if let Some(pb) = slot.take() {
                        pb.finish_and_clear();
                    }
                    println!("Saved to {}", path.display());
                }
                RecvEvent::Swarm {
                    peers,
                    pieces_from_peers,
                } => {
                    if let Some(pb) = slot.as_ref() {
                        pb.set_message(format!(
                            "swarm: {peers} peers, {pieces_from_peers} pieces from peers"
                        ));
                    }
                }
            }
        },
    )
    .await?;
    Ok(())
}

// ---- offline mailbox ------------------------------------------------------
