use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use arvolo_core::chunked::{ChunkTicket, KeyDelivery};
use arvolo_core::code;
use arvolo_core::crypto::{Identity, PublicId};
use arvolo_core::flow::{self, SendEvent};
use arvolo_core::manager::{ManagerEvent, TransferManager};
use arvolo_core::transfer::RelayChoice;

use crate::{book, sessions};

use crate::output::vprintln;

use crate::ui::*;
use crate::util::*;

#[cfg(unix)]
use crate::commands::daemon::{
    daemon_client, push_via_daemon, serve_code_via_daemon, serve_ticket_via_daemon,
};
use crate::commands::offline::send_offline;
use crate::commands::receive::record_history;
use crate::output::verbosity;

pub(crate) async fn push(
    paths: Vec<PathBuf>,
    to: String,
    relay: Option<String>,
    use_http: bool,
    note: &str,
) -> Result<()> {
    anyhow::ensure!(
        !paths.is_empty(),
        "provide at least one file or folder to push"
    );

    // If a daemon is running, hand the send off to it (concurrent, survives our
    // exit); otherwise fall back to a one-shot in-process send.
    #[cfg(unix)]
    {
        if let Some(client) = daemon_client().await {
            return push_via_daemon(client, paths, to, note.to_string()).await;
        }
    }

    let relay = require_relay(relay, use_http)?;
    let recipient = book::resolve_recipient(&to)?;
    vprintln!(
        "recipient {to} resolved (fingerprint {})",
        recipient.fingerprint()
    );
    let me = my_identity()?;
    let (payload, name, archive, temp) = resolve_payload(&paths)?;
    if archive {
        eprintln!("Packing {} item(s) into an archive…", paths.len());
    }
    vprintln!(
        "payload: {} ({}){}",
        name,
        human_size(std::fs::metadata(&payload).map(|m| m.len()).unwrap_or(0)),
        if archive { ", packed archive" } else { "" }
    );

    let manager = TransferManager::new(me, Some(relay.clone()), PathBuf::from("."));
    manager.set_display_name(book::my_display_name());
    let mut events = manager.subscribe();

    // `send_to` decides live-vs-mailbox itself (with a presence grace window and a
    // watchdog); the up-front check is only a hint for the opening line.
    vprintln!("checking {to}'s presence on the relay…");
    if manager.is_online(&recipient).await {
        eprintln!("{to} looks online — trying a direct transfer…");
    } else {
        eprintln!("{to} looks offline — will deposit to the mailbox…");
    }
    eprintln!("Sending offer to {to}… (Ctrl-C to abort)\n");
    let id = manager
        .send_to(&recipient, payload, name.clone(), archive, note.to_string())
        .await
        .inspect_err(|_| {
            if let Some(t) = &temp {
                let _ = std::fs::remove_file(t);
            }
        })?;

    let cancel = cancel_on_ctrl_c();
    let mut last_pct = u64::MAX;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                manager.cancel(id);
                break;
            }
            ev = events.recv() => {
                match ev {
                    Ok(ManagerEvent::Progress { id: eid, transferred, total_size }) if eid == id && total_size > 0 => {
                        let pct = transferred * 100 / total_size;
                        if pct != last_pct {
                            last_pct = pct;
                            eprint!("\r  {pct}% ({}/{})   ", human_size(transferred), human_size(total_size));
                            use std::io::Write;
                            let _ = std::io::stderr().flush();
                        }
                    }
                    Ok(ManagerEvent::Completed { id: eid, .. }) if eid == id => {
                        // The offline path emits Deposited first (handled below); a
                        // Completed here is a live P2P delivery.
                        eprintln!("\n✓ delivered.");
                        record_history(&manager, id, "completed");
                        break;
                    }
                    Ok(ManagerEvent::Deposited { id: eid, info }) if eid == id => {
                        eprintln!("✓ deposited to the mailbox (delivered when they return).");
                        record_history(&manager, id, "deposited");
                        // This manager has no state_dir, so the engine keeps no record
                        // of its own: without this the revoke token would die with the
                        // process and the file would sit on the relay until its TTL,
                        // unlistable and unwithdrawable. No engine row to cancel through
                        // either — this send ends with the command.
                        crate::deposits::record_from_event(None, &info);
                        eprintln!(
                            "   Take it back any time with `arvolo cancel {}`.",
                            crate::deposits::id_for(&info.claim)
                        );
                        break;
                    }
                    Ok(ManagerEvent::Waiting { id: eid, reason }) if eid == id => {
                        eprintln!("\n⏳ held: {reason}");
                        eprintln!(
                            "   The daemon keeps trying in the background — see `arvolo status`."
                        );
                        record_history(&manager, id, &format!("waiting: {reason}"));
                        break;
                    }
                    Ok(ManagerEvent::Paused { id: eid, reason }) if eid == id => {
                        eprintln!("\n⏸  paused: {reason}");
                        eprintln!("   `arvolo resume {id}` to continue, or `arvolo cancel {id}`.");
                        record_history(&manager, id, &format!("paused: {reason}"));
                        break;
                    }
                    Ok(ManagerEvent::Failed { id: eid, error }) if eid == id => {
                        eprintln!("\n✗ failed: {error}");
                        record_history(&manager, id, &format!("failed: {error}"));
                        break;
                    }
                    Ok(ManagerEvent::Cancelled { id: eid }) if eid == id => {
                        record_history(&manager, id, "cancelled");
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }
    }
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    Ok(())
}

/// [`resolve_payload`] plus the two lines every sender wants to see: that we're
/// packing, and what ended up being sent. Shared by `code` and `ticket`, which
/// both hand the result to the chunked-send path.
fn announce_payload(paths: &[PathBuf]) -> Result<(PathBuf, String, bool, Option<PathBuf>)> {
    let (payload, name, archive, temp) = resolve_payload(paths)?;
    if archive {
        eprintln!("Packing {} item(s) into an archive…", paths.len());
    }
    vprintln!(
        "payload: {} ({}){}",
        name,
        human_size(std::fs::metadata(&payload).map(|m| m.len()).unwrap_or(0)),
        if archive { ", packed archive" } else { "" }
    );
    Ok((payload, name, archive, temp))
}

/// `arvolo send <who> <paths…>` — deliver to a known recipient.
///
/// Online (their daemon is reachable) → delivered live; offline, or `--deposit`
/// → left on the mailbox as an inbox offer, with an `arvm…` ticket printed so
/// the sender can hand it over by another route too.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_to(
    who: String,
    paths: Vec<PathBuf>,
    deposit: bool,
    note: Option<String>,
    relay: Option<String>,
    use_http: bool,
    ttl: u64,
    max: Option<u32>,
    password: Option<String>,
    qr: bool,
) -> Result<()> {
    // The note rides inside the sealed offer; cap it so it fits comfortably.
    let note = note.unwrap_or_default();
    anyhow::ensure!(note.len() <= 4096, "--note is too long (max 4096 bytes)");

    let relay_url = require_relay(relay, use_http)?;
    let recipient = book::resolve_recipient(&who)?;
    book::warn_if_unverified(&who, &encode_id(&recipient));
    let online = if deposit {
        false
    } else {
        vprintln!("checking {who}'s presence on the relay…");
        arvolo_core::presence::check_online(&reqwest::Client::new(), &relay_url, &recipient)
            .await
            .unwrap_or(false)
    };
    if online {
        // Not a flag conflict — which of the two paths runs is decided by the
        // presence probe, so this can only be found out here.
        if max.is_some() || password.is_some() {
            eprintln!(
                "note: --max/--password apply to a deposited send; ignored for a live delivery."
            );
        }
        return push(paths, who, Some(relay_url), use_http, &note).await;
    }
    send_offline(
        paths,
        Some(who),
        false,
        Some(relay_url),
        use_http,
        ttl,
        max,
        password,
        qr,
        true,
        &note,
    )
    .await
}

/// `arvolo link <paths…>` — a public, browser-openable download URL.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn link(
    paths: Vec<PathBuf>,
    relay: Option<String>,
    use_http: bool,
    ttl: u64,
    max: Option<u32>,
    password: Option<String>,
    qr: bool,
) -> Result<()> {
    send_offline(
        paths, None, true, relay, use_http, ttl, max, password, qr, false, "",
    )
    .await
}

/// `arvolo code <paths…>` — hand the ticket over as a short pairing code.
pub(crate) async fn code_cmd(
    paths: Vec<PathBuf>,
    relay: Option<String>,
    use_http: bool,
    keep: bool,
    foreground: bool,
    qr: bool,
) -> Result<()> {
    // A bare relay host gets a scheme (https by default, http with --use-http).
    let relay = relay.map(|r| book::normalize_relay(&r, use_http));

    // By default hand the code to a running daemon: it hosts the rendezvous in
    // the background, survives this terminal *and* a daemon restart, and shows up
    // in `arvolo status`. `--foreground` keeps it inline. Mirrors `ticket_cmd`.
    #[cfg(unix)]
    {
        if !foreground {
            if let Some(client) = daemon_client().await {
                match serve_code_via_daemon(client, paths.clone(), relay.clone(), keep, qr).await {
                    Ok(()) => return Ok(()),
                    // An older daemon doesn't know `ServeCode`, and a relay that
                    // doesn't speak rendezvous v2 can't host one. Neither is worth
                    // failing over — serve it here instead, and say so.
                    Err(e) => eprintln!("Serving it here instead ({e:#})."),
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = foreground;

    if keep {
        eprintln!(
            "note: --keep needs the daemon to host the code; in this terminal it \
             serves one receiver and stops."
        );
    }
    let (payload, name, archive, temp) = announce_payload(&paths)?;
    // The code carries its own rendezvous relay, so no swarm seed is embedded.
    send_with_code(payload, name, archive, temp, relay, None, qr).await
}

/// `arvolo ticket <paths…>` — a self-contained `arvc…` ticket to share.
pub(crate) async fn ticket_cmd(
    paths: Vec<PathBuf>,
    relay: Option<String>,
    use_http: bool,
    foreground: bool,
    qr: bool,
) -> Result<()> {
    // A bare relay host gets a scheme (https by default, http with --use-http).
    let relay = relay.map(|r| book::normalize_relay(&r, use_http));
    // Swarm is the norm: embed the configured relay in every arvc… ticket so the
    // recipient can backfill from it AND the relay acts as the swarm tracker
    // (peers seed to each other). Best-effort in the core — if the relay is
    // unreachable at send time, the ticket falls back to pure P2P.
    let seed_relay = relay.or_else(book::default_relay);
    if let Some(r) = &seed_relay {
        vprintln!("swarm relay (embedded in ticket): {r}");
    }

    // By default hand the send to a running daemon: it serves in the background,
    // observable via `arvolo status` and surviving this terminal. `--foreground`
    // keeps it inline in this process.
    #[cfg(unix)]
    {
        if !foreground {
            if let Some(client) = daemon_client().await {
                return serve_ticket_via_daemon(client, paths, seed_relay, qr).await;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = foreground;

    let (payload, name, archive, temp) = announce_payload(&paths)?;
    vprintln!("plain ticket: the ticket itself is the capability (anonymous, unauthenticated)");

    eprintln!("Splitting and serving chunks…");
    let session = flow::prepare_send(
        &payload,
        &name,
        archive,
        None,
        seed_relay,
        RelayChoice::from_env(),
    )
    .await?;
    // Persist a resumable session (best-effort: a save failure must not abort the
    // send). The content key is stored so an interrupted send can be recovered —
    // listed by `arvolo status` and recovered with `arvolo resume <id>`.
    if let Err(e) = sessions::save(
        session.content_key(),
        session.node_seed(),
        &paths,
        &name,
        archive,
        session.total_size,
        session.chunks,
        &session.ticket,
    ) {
        eprintln!("(note: could not save resumable session: {e:#})");
    }
    let result = serve_session(session, qr).await;
    // The payload (a packed archive) had to stay readable for the whole session,
    // since chunks are produced on the fly; clean it up now that serving ended.
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    result
}

/// Drive a prepared/resumed [`flow::SendSession`] to completion, printing the
/// ticket when it's ready. Shared by fresh sends and both resume paths.
pub(crate) async fn serve_session(session: flow::SendSession, qr: bool) -> Result<()> {
    let cancel = cancel_on_ctrl_c();
    // Last progress percent we narrated, so `-v` shows a few milestones instead
    // of one line per ack. Shared because `serve`'s callback is `Fn`, not `FnMut`.
    let last_pct = Arc::new(AtomicU8::new(u8::MAX));
    session
        .serve(cancel, move |ev| match ev {
            SendEvent::Ready {
                chunks,
                total_size,
                ticket,
                has_relay,
            } => {
                println!("\nFile ready ({chunks} chunks). On the other device:\n");
                println!("    arvolo recv {ticket}\n");
                if qr {
                    print_qr(&ticket);
                }
                if has_relay {
                    println!("P2P-first; if the receiver drops, only the missing chunks are backfilled to the relay.");
                }
                vprintln!(
                    "serving {chunks} chunk(s), {} total; waiting for a receiver to connect",
                    human_size(total_size)
                );
                println!("Ctrl-C to stop.");
            }
            SendEvent::ReceiverConnected => vprintln!("receiver connected — chunk pull started"),
            SendEvent::Progress { transferred, total } if total > 0 => {
                let pct = (transferred * 100 / total) as u8;
                // Narrate every ~10% step, once each.
                if verbosity() >= 1 && pct / 10 != last_pct.load(Ordering::Relaxed) / 10 {
                    last_pct.store(pct, Ordering::Relaxed);
                    vprintln!(
                        "acked {pct}% ({}/{})",
                        human_size(transferred),
                        human_size(total)
                    );
                }
            }
            SendEvent::Progress { .. } => {}
            SendEvent::Delivered => eprintln!("✓ A receiver got the whole file."),
            SendEvent::ReceiverDropped { missing } => {
                eprintln!("Receiver dropped — backfilling {missing} missing chunks to the relay…")
            }
            SendEvent::Backfilled => {
                eprintln!("Backfilled. You can close this; the relay can finish the delivery.")
            }
            SendEvent::BackfillFailed { reason } => eprintln!("Relay backfill failed: {reason}"),
            SendEvent::RelayCapped { limit_bytes } => eprintln!("{}", relay_capped_line(limit_bytes)),
            SendEvent::Peers { count } => {
                vprintln!("{count} peer(s) downloading");
            }
        })
        .await
}

/// `arvolo resume <arvc…> <file>`: re-serve `path` under an existing *plain*
/// ticket so it stays valid after the sender restarted. The key rides in the
/// ticket, so no saved session is needed. Tickets sealed to a recipient resume
/// by session id instead (`arvolo resume <id>`).
pub(crate) async fn resume_by_ticket(ticket: &str, path: &Path, qr: bool) -> Result<()> {
    let expected = ChunkTicket::decode(ticket).context("parse ticket")?;
    let key: [u8; 32] = match &expected.key {
        KeyDelivery::Plain(bytes) => bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("ticket content key has the wrong size"))?,
        KeyDelivery::Sealed { .. } => anyhow::bail!(
            "this ticket is sealed to a recipient, so its key isn't in the ticket — \
             resume it by session id instead: `arvolo resume <id>` (see `arvolo status`)"
        ),
    };
    // A plain ticket carries no transport secret, so we can't rebind the original
    // node id — the sender comes back under a fresh id and the receiver must use
    // the reprinted ticket (their partial download still resumes: same chunks).
    eprintln!(
        "Resuming send — re-serving the file. NOTE: use the reprinted ticket below on the receiver \
         (the original ticket's address is stale after a restart). For full old-ticket recovery, \
         start sends normally and resume by session id: `arvolo resume <id>`."
    );
    let session = flow::resume_send(path, key, None, &expected, RelayChoice::from_env()).await?;
    serve_session(session, qr).await
}

/// `arvolo resume <id>`: replay a saved session (covers deliveries to a contact
/// too, which is why it needs no file). A
/// single file is re-served in place; an archive is repacked deterministically.
pub(crate) async fn resume_by_id(id: &str, qr: bool) -> Result<()> {
    let rec = sessions::load(id)?;
    let expected = ChunkTicket::decode(&rec.ticket).context("parse saved ticket")?;
    let key = rec.key()?;
    let node_seed = rec.node_seed()?;
    for s in &rec.sources {
        anyhow::ensure!(
            s.exists(),
            "source is gone, cannot resume session '{id}': {}",
            s.display()
        );
    }
    eprintln!(
        "Resuming session {id} ({}) — re-serving under the saved ticket…",
        rec.name
    );
    // For an archive, materialize the deterministic tar once and serve from it;
    // for a single file, serve it directly.
    let temp = if rec.archive {
        let t = book::temp_dir().join(format!("arvolo-resume-{}.tar", std::process::id()));
        flow::pack_tar(&rec.sources, &t).context("repack archive for resume")?;
        Some(t)
    } else {
        None
    };
    let payload = temp.clone().unwrap_or_else(|| rec.sources[0].clone());
    let session = flow::resume_send(
        &payload,
        key,
        Some(node_seed),
        &expected,
        RelayChoice::from_env(),
    )
    .await?;
    let result = serve_session(session, qr).await;
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    result
}

/// `arvolo code`: hand the ticket to the receiver via a short pairing code over a
/// relay rendezvous, serving the file (and using the relay for backfill) meanwhile.
pub(crate) async fn send_with_code(
    payload: PathBuf,
    name: String,
    archive: bool,
    temp: Option<PathBuf>,
    relay: Option<String>,
    to: Option<(&Identity, &PublicId)>,
    qr: bool,
) -> Result<()> {
    // An explicit --relay is embedded in the code (works with no receiver config);
    // otherwise fall back to the default relay (short code, shared default).
    let (relay_url, embed) = match relay {
        Some(r) => (r, true),
        None => match book::default_relay_or_builtin() {
            Some(r) => (r, false),
            None => anyhow::bail!("--code needs a relay: pass --relay <host>, set ARVOLO_RELAY, or configure `relay` in config.toml"),
        },
    };

    vprintln!(
        "rendezvous relay: {relay_url} ({})",
        if embed {
            "embedded in the code — receiver needs no config"
        } else {
            "shared default — not embedded"
        }
    );
    eprintln!("Splitting and serving chunks…");
    let session = flow::prepare_send(
        &payload,
        &name,
        archive,
        to,
        Some(relay_url.clone()),
        RelayChoice::from_env(),
    )
    .await?;
    vprintln!(
        "serving {} chunk(s), {}; publishing the encrypted ticket under the pairing code…",
        session.chunks,
        human_size(session.total_size)
    );
    let (shown_code, complete) = code::publish_ticket(&session.ticket, &relay_url, embed)
        .await
        .context("start pairing")?;

    println!("\nOn the other device:\n");
    println!("    arvolo recv {shown_code}\n");
    if qr {
        print_qr(&shown_code);
    }
    println!("Ctrl-C to stop.");

    let cancel = cancel_on_ctrl_c();
    // Finish the pairing (publish the encrypted ticket once the receiver shows up)
    // in the background while we serve.
    let pairing = tokio::spawn(async move {
        if let Err(e) = complete.run().await {
            eprintln!("Pairing failed: {e:#}");
        }
    });
    let result = session
        .serve(cancel, |ev| match ev {
            SendEvent::ReceiverDropped { missing } => {
                eprintln!("Receiver dropped — backfilling {missing} missing chunks to the relay…")
            }
            SendEvent::Backfilled => {
                eprintln!("Backfilled. You can close this; the relay can finish the delivery.")
            }
            SendEvent::BackfillFailed { reason } => eprintln!("Relay backfill failed: {reason}"),
            SendEvent::RelayCapped { limit_bytes } => {
                eprintln!("{}", relay_capped_line(limit_bytes))
            }
            SendEvent::Delivered => eprintln!("✓ A receiver got the whole file."),
            SendEvent::ReceiverConnected => vprintln!("receiver connected — chunk pull started"),
            SendEvent::Peers { count } => vprintln!("{count} peer(s) downloading"),
            SendEvent::Progress { .. } => {}
            SendEvent::Ready { .. } => {} // code already printed
        })
        .await;
    pairing.abort();
    // The packed archive had to stay readable for the whole session (chunks are
    // produced on the fly); clean it up now.
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    result
}
