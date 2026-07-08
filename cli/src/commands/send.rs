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
use crate::commands::daemon::{daemon_client, push_via_daemon, serve_ticket_via_daemon};
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
                    Ok(ManagerEvent::Deposited { id: eid }) if eid == id => {
                        eprintln!("✓ deposited to the mailbox (delivered when they return).");
                        record_history(&manager, id, "deposited");
                        break;
                    }
                    Ok(ManagerEvent::Waiting { id: eid, reason }) if eid == id => {
                        eprintln!("\n⏳ held: {reason}");
                        eprintln!(
                            "   The daemon keeps trying in the background — see `arvolo transfers`."
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn send(
    paths: Vec<PathBuf>,
    resume: Option<String>,
    code_mode: bool,
    relay: Option<String>,
    use_http: bool,
    to: Option<String>,
    ticket_mode: bool,
    note: Option<String>,
    link: bool,
    ttl: u64,
    max: Option<u32>,
    password: Option<String>,
    foreground: bool,
    qr: bool,
) -> Result<()> {
    // A note only rides a `--to` send; cap it so it fits comfortably in the offer.
    let note = {
        let n = note.unwrap_or_default();
        anyhow::ensure!(n.len() <= 4096, "--note is too long (max 4096 bytes)");
        if !n.is_empty() && to.is_none() {
            eprintln!("note: --note only applies to a `--to` send — ignoring it.");
            String::new()
        } else {
            n
        }
    };
    // Resume short-circuits the normal flow: re-serve a previous send so the
    // ticket you already handed out stays valid after the sender restarted.
    // Recovery is pure P2P. The argument is either a plain `arvc…` ticket
    // (re-serve, needs the file) or a saved session id.
    if let Some(arg) = resume {
        anyhow::ensure!(
            !code_mode && to.is_none() && !link && !ticket_mode,
            "--resume replays a saved P2P send (no --to/--code/--link/--ticket)"
        );
        if ChunkTicket::looks_like(&arg) {
            anyhow::ensure!(
                paths.len() == 1,
                "resuming from an `arvc…` ticket needs exactly one path: the file to re-serve"
            );
            return resume_by_ticket(&arg, &paths[0], qr).await;
        }
        anyhow::ensure!(
            paths.is_empty(),
            "resuming a session by id takes no paths (the saved session remembers the file)"
        );
        return resume_by_id(&arg, qr).await;
    }

    anyhow::ensure!(
        !paths.is_empty(),
        "provide at least one file or folder to send (or use --resume)"
    );

    // --link: a public, browser-openable download URL (no recipient).
    if link {
        anyhow::ensure!(to.is_none(), "--link makes a public link; drop --to");
        anyhow::ensure!(
            !code_mode && !ticket_mode,
            "--link can't be combined with --code / --ticket"
        );
        return send_offline(
            paths, None, true, relay, use_http, ttl, max, password, qr, false, "",
        )
        .await;
    }

    // --to: deliver to a known recipient. Online (a live daemon) → delivered live
    // (push); offline or --ticket → deposited on the mailbox + an inbox offer, and
    // an `arvm…` ticket is printed so you can also hand it over.
    if let Some(to) = to {
        anyhow::ensure!(
            !code_mode,
            "--code makes a shareable P2P ticket; it doesn't apply with --to"
        );
        let relay_url = require_relay(relay, use_http)?;
        let recipient = book::resolve_recipient(&to)?;
        let online = if ticket_mode {
            false
        } else {
            vprintln!("checking {to}'s presence on the relay…");
            arvolo_core::presence::check_online(&reqwest::Client::new(), &relay_url, &recipient)
                .await
                .unwrap_or(false)
        };
        if online {
            if max.is_some() || password.is_some() {
                eprintln!(
                    "note: --max/--password apply to a mailbox send; ignored for a live delivery."
                );
            }
            return push(paths, to, Some(relay_url), use_http, &note).await;
        }
        return send_offline(
            paths,
            Some(to),
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
        .await;
    }

    // No --to: a shareable P2P ticket (arvc…) or pairing code. Mailbox-only flags
    // don't apply here.
    anyhow::ensure!(
        max.is_none() && password.is_none(),
        "--max/--password apply to a mailbox send or --link (use --to or --link)"
    );

    // A bare relay host gets a scheme (https by default, http with --use-http).
    let relay = relay.map(|r| book::normalize_relay(&r, use_http));
    // Swarm is the norm: embed the configured relay in every arvc… ticket so the
    // recipient can backfill from it AND the relay acts as the swarm tracker
    // (peers seed to each other). Best-effort in the core — if the relay is
    // unreachable at send time, the ticket falls back to pure P2P. `--code`
    // carries its own rendezvous relay, so it opts out here.
    let seed_relay = if code_mode {
        None
    } else {
        relay.clone().or_else(book::default_relay)
    };
    if let Some(r) = &seed_relay {
        vprintln!("swarm relay (embedded in ticket): {r}");
    }

    // By default hand a plain ticket send to a running daemon: it serves in the
    // background, observable via `arvolo transfers` and surviving this terminal.
    // `--foreground` (or `--code`) keep it inline in this process.
    #[cfg(unix)]
    {
        if !code_mode && !foreground {
            if let Some(client) = daemon_client().await {
                return serve_ticket_via_daemon(client, paths, seed_relay, qr).await;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = foreground;

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
    vprintln!("plain ticket: the ticket itself is the capability (anonymous, unauthenticated)");

    if code_mode {
        return send_with_code(payload, name, archive, temp, relay, None, qr).await;
    }
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
    // see `arvolo sessions` and `send --resume`.
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

/// `send --resume <arvc…> <file>`: re-serve `path` under an existing *plain*
/// ticket so it stays valid after the sender restarted. The key rides in the
/// ticket, so no saved session is needed. Sealed (`--to`) tickets resume by
/// session id instead (`send --resume <id>`).
pub(crate) async fn resume_by_ticket(ticket: &str, path: &Path, qr: bool) -> Result<()> {
    let expected = ChunkTicket::decode(ticket).context("parse ticket")?;
    let key: [u8; 32] = match &expected.key {
        KeyDelivery::Plain(bytes) => bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("ticket content key has the wrong size"))?,
        KeyDelivery::Sealed { .. } => anyhow::bail!(
            "this ticket is sealed to a recipient (--to), so its key isn't in the ticket — \
             resume it with `arvolo send --resume <id>` (see `arvolo sessions list`)"
        ),
    };
    // A plain ticket carries no transport secret, so we can't rebind the original
    // node id — the sender comes back under a fresh id and the receiver must use
    // the reprinted ticket (their partial download still resumes: same chunks).
    eprintln!(
        "Resuming send — re-serving the file. NOTE: use the reprinted ticket below on the receiver \
         (the original ticket's address is stale after a restart). For full old-ticket recovery, \
         start sends normally and resume with `arvolo send --resume <id>`."
    );
    let session = flow::resume_send(path, key, None, &expected, RelayChoice::from_env()).await?;
    serve_session(session, qr).await
}

/// `send --resume <id>`: replay a saved session (covers `--to` sends too). A
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

/// `send --code`: hand the ticket to the receiver via a short pairing code over a
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
