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

use crate::commands::daemon::{
    daemon_client, push_via_daemon, serve_code_via_daemon, serve_ticket_via_daemon,
};
use crate::commands::offline::{send_link, send_sealed};
use crate::commands::receive::record_history;
use crate::output::verbosity;

/// The unified `arvolo send`: one verb, four shapes. Which one runs is decided
/// here, from flags clap has already checked for consistency — by the time this
/// is called the combination is known to make sense.
pub(crate) struct SendOpts {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) to: Option<String>,
    pub(crate) mailbox: bool,
    pub(crate) link: bool,
    pub(crate) code: bool,
    pub(crate) ticket: bool,
    pub(crate) note: Option<String>,
    pub(crate) ttl: u64,
    pub(crate) max: Option<u32>,
    pub(crate) password: Option<String>,
    pub(crate) keep: bool,
    pub(crate) foreground: bool,
    pub(crate) qr: bool,
    pub(crate) relay: Option<String>,
}

pub(crate) async fn send_cmd(opts: SendOpts) -> Result<()> {
    guard_contact_in_path_slot(&opts.paths, opts.to.is_some())?;
    guard_readable(&opts.paths)?;
    // `--password` with no value means "prompt me" (clap stores the empty
    // string); resolve it before anything leaves the machine.
    let password = match opts.password {
        Some(p) if p.is_empty() => Some(prompt_password()?),
        other => other,
    };
    if let Some(who) = opts.to {
        return send_to(
            who,
            opts.paths,
            opts.mailbox,
            opts.note,
            opts.relay,
            opts.ttl,
            opts.max,
            password,
        )
        .await;
    }
    if opts.link {
        // Through the daemon when it runs and the payload is one plain file: the
        // link then has a live engine row — visible in the GUI, cancelled in one
        // place — same as code/ticket/push already do. Several paths are packed
        // into a temp archive this process must clean up, so those stay here.
        if opts.paths.len() == 1 && opts.paths[0].is_file() {
            if let Some(mut client) = daemon_client().await {
                let abs = std::fs::canonicalize(&opts.paths[0])
                    .with_context(|| format!("{}", opts.paths[0].display()))?
                    .to_string_lossy()
                    .into_owned();
                let (handoff, _offer) = crate::commands::daemon::offer_sources(&opts.paths);
                match client
                    .create_link(abs, Some(opts.ttl), opts.max, handoff)
                    .await
                {
                    Ok(url) => {
                        println!("{url}");
                        eprintln!(
                            "\nAnyone with the link above can download it in a browser — no \
                             arvolo needed. Deposited via the daemon; listed in `arvolo status`, \
                             withdrawn with `arvolo cancel <id>`."
                        );
                        if opts.qr {
                            print_qr(&url);
                        }
                        return Ok(());
                    }
                    // An older daemon or a relay hiccup — deposit from here
                    // instead, and say so.
                    Err(e) => eprintln!("Depositing from here instead ({e:#})."),
                }
            }
        }
        return send_link(opts.paths, opts.relay, opts.ttl, opts.max, opts.qr).await;
    }
    if opts.code {
        return code_cmd(opts.paths, opts.relay, opts.keep, opts.foreground, opts.qr).await;
    }
    // Default: a `.arvolo` ticket file, like a .torrent — `--ticket` skips the
    // file and prints the raw ticket for scripts.
    let out = if opts.ticket {
        TicketOut::Raw
    } else {
        TicketOut::File
    };
    ticket_cmd(opts.paths, opts.relay, opts.foreground, out).await
}

/// The habit guard for the old `send <who> <paths…>` shape: with `--to` gone
/// missing, a contact name lands in the *paths* and the natural failure would be
/// "file does not exist" — true, useless. Say what actually happened.
fn guard_contact_in_path_slot(paths: &[PathBuf], has_to: bool) -> Result<()> {
    if has_to {
        return Ok(());
    }
    let Some(first) = paths.first() else {
        return Ok(());
    };
    if first.exists() {
        return Ok(());
    }
    let s = first.to_string_lossy();
    if book::resolve_recipient(&s).is_ok() {
        anyhow::bail!(
            "'{s}' is a contact, not a file. The recipient is a flag now:\n\n    \
             arvolo send <files…> --to {s}\n"
        );
    }
    Ok(())
}

/// Prove every source is readable from THIS process before anything is queued.
/// macOS keeps Downloads/Desktop/Documents behind per-app consent, and a daemon
/// spawned outside the granted app gets `Operation not permitted` on open — a
/// send queued anyway used to sit as a silent "active, 0 B" forever. An open()
/// here fails fast, in the terminal the user is looking at, with the fix named.
fn guard_readable(paths: &[PathBuf]) -> Result<()> {
    for p in paths {
        let probe = if p.is_dir() {
            std::fs::read_dir(p).map(|_| ())
        } else {
            std::fs::File::open(p).map(|_| ())
        };
        if let Err(e) = probe {
            let hint = if e.kind() == std::io::ErrorKind::PermissionDenied {
                "\n  This folder is privacy-guarded (macOS asks per app). Move the file \
                 somewhere neutral, or grant access in System Settings → Privacy & \
                 Security → Files and Folders (or Full Disk Access)."
            } else {
                ""
            };
            anyhow::bail!("cannot read {}: {e}{hint}", p.display());
        }
    }
    Ok(())
}

pub(crate) async fn push(
    paths: Vec<PathBuf>,
    to: String,
    relay: Option<String>,
    note: &str,
    ttl: Option<u64>,
    max: Option<u32>,
    password: Option<String>,
) -> Result<()> {
    anyhow::ensure!(
        !paths.is_empty(),
        "provide at least one file or folder to push"
    );

    // If a daemon is running, hand the send off to it (concurrent, survives our
    // exit); otherwise fall back to a one-shot in-process send. The mailbox
    // options ride along: the daemon re-probes presence, and a recipient who
    // dropped offline in between must still get the ttl/max/password asked for.
    {
        if let Some(client) = daemon_client().await {
            return push_via_daemon(client, paths, to, note.to_string(), ttl, max, password)
                .await;
        }
    }

    let relay = require_relay(relay)?;
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
    // Built on the first Progress event, which is what carries the total.
    let mut progress: Option<Progress> = None;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                manager.cancel(id);
                break;
            }
            ev = events.recv() => {
                match ev {
                    Ok(ManagerEvent::Progress { id: eid, transferred, total_size }) if eid == id && total_size > 0 => {
                        progress
                            .get_or_insert_with(|| Progress::new("sending", total_size))
                            .update(transferred);
                    }
                    Ok(ManagerEvent::Completed { id: eid, .. }) if eid == id => {
                        // The offline path emits Deposited first (handled below); a
                        // Completed here is a live P2P delivery.
                        if let Some(p) = progress.take() { p.finish(); }
                        eprintln!("✓ delivered.");
                        record_history(&manager, id, "completed");
                        break;
                    }
                    Ok(ManagerEvent::Deposited { id: eid, info }) if eid == id => {
                        if let Some(p) = progress.take() { p.finish(); }
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
                        if let Some(p) = progress.take() { p.finish(); }
                        eprintln!("⏳ held: {reason}");
                        eprintln!(
                            "   The daemon keeps trying in the background — see `arvolo status`."
                        );
                        record_history(&manager, id, &format!("waiting: {reason}"));
                        break;
                    }
                    Ok(ManagerEvent::Paused { id: eid, reason }) if eid == id => {
                        if let Some(p) = progress.take() { p.finish(); }
                        eprintln!("⏸  paused: {reason}");
                        eprintln!("   `arvolo resume {id}` to continue, or `arvolo cancel {id}`.");
                        record_history(&manager, id, &format!("paused: {reason}"));
                        break;
                    }
                    Ok(ManagerEvent::Failed { id: eid, error }) if eid == id => {
                        if let Some(p) = progress.take() { p.finish(); }
                        eprintln!("✗ failed: {error}");
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

/// `arvolo send <paths…> --to <who>` — deliver to a known recipient.
///
/// Online (their daemon is reachable) → delivered live; offline, or `--mailbox`
/// → left on the mailbox as an inbox offer, with an `arvm…` ticket printed so
/// the sender can hand it over by another route too.
#[allow(clippy::too_many_arguments)]
async fn send_to(
    who: String,
    paths: Vec<PathBuf>,
    mailbox: bool,
    note: Option<String>,
    relay: Option<String>,
    ttl: u64,
    max: Option<u32>,
    password: Option<String>,
) -> Result<()> {
    // The note rides inside the sealed offer; cap it so it fits comfortably.
    let note = note.unwrap_or_default();
    anyhow::ensure!(note.len() <= 4096, "--note is too long (max 4096 bytes)");

    let relay_url = require_relay(relay)?;
    let recipient = book::resolve_recipient(&who)?;
    book::warn_if_unverified(&who, &encode_id(&recipient));
    // With P2P off there is nothing to gain from the probe: a live delivery is not
    // available whatever it answers, so don't spend the request — and say why, or the
    // sudden absence of the live path looks like the recipient never being online.
    let online = if mailbox {
        false
    } else if !arvolo_core::transfer::p2p_enabled() {
        vprintln!("P2P is off — depositing on the relay instead of probing presence");
        false
    } else {
        vprintln!("checking {who}'s presence on the relay…");
        arvolo_core::presence::check_online(&arvolo_core::http::client(), &relay_url, &recipient)
            .await
            .unwrap_or(false)
    };
    if online {
        // Not a flag conflict — which of the two paths runs is decided by the
        // presence probe, so this can only be found out here. The options are
        // NOT dropped: if the daemon's own probe disagrees and the send falls
        // back to the mailbox, they apply there.
        if max.is_some() || password.is_some() {
            eprintln!(
                "note: --max/--password apply only if this send falls back to the mailbox."
            );
        }
        return push(
            paths,
            who,
            Some(relay_url),
            &note,
            Some(ttl),
            max,
            password,
        )
        .await;
    }
    send_sealed(
        paths,
        who,
        Some(relay_url),
        ttl,
        max,
        password,
        true,
        &note,
    )
    .await
}

/// `arvolo send --code` — hand the ticket over as a short pairing code.
async fn code_cmd(
    paths: Vec<PathBuf>,
    relay: Option<String>,
    keep: bool,
    foreground: bool,
    qr: bool,
) -> Result<()> {
    // A code brokers a *direct* transfer: the rendezvous only carries the ticket.
    ensure_p2p("a pairing code")?;
    // A bare relay host gets a scheme (https unless written explicitly).
    let relay = relay.map(|r| book::normalize_relay(&r));

    // By default hand the code to a running daemon: it hosts the rendezvous in
    // the background, survives this terminal *and* a daemon restart, and shows up
    // in `arvolo status`. `--foreground` keeps it inline. Mirrors `ticket_cmd`.
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

/// Where the `arvc…` ticket of a serve ends up.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TicketOut {
    /// The raw ticket alone on stdout — for scripts and pipes (`--ticket`).
    Raw,
    /// A `<name>.arvolo` file in the current directory, its path alone on
    /// stdout — the default: share the file like a .torrent.
    File,
}

/// Deliver a ready ticket the way [`TicketOut`] asks: stdout carries exactly the
/// artefact (the ticket, or the path of the `.arvolo` file it was written to);
/// everything else goes to stderr.
fn emit_ticket(out: TicketOut, base_name: &str, ticket: &str) {
    match out {
        TicketOut::Raw => {
            println!("{ticket}");
            eprintln!("\nOn the other device:  arvolo recv <the ticket above>");
        }
        TicketOut::File => match write_arvolo_file(base_name, ticket) {
            Ok(path) => {
                println!("{}", path.display());
                eprintln!("\nTicket file written. Share it over any channel; on the other device:\n");
                eprintln!("    arvolo recv {}\n", path.display());
            }
            // The send is already serving — a file that can't be written must
            // not kill it. Fall back to the raw ticket, and say why.
            Err(e) => {
                eprintln!("(could not write the .arvolo file: {e:#} — printing the ticket instead)");
                println!("{ticket}");
                eprintln!("\nOn the other device:  arvolo recv <the ticket above>");
            }
        },
    }
}

/// `arvolo send` (default) / `arvolo send --ticket` — a self-contained `arvc…`
/// ticket, as a shareable `.arvolo` file or raw on stdout.
async fn ticket_cmd(
    paths: Vec<PathBuf>,
    relay: Option<String>,
    foreground: bool,
    out: TicketOut,
) -> Result<()> {
    // An `arvc…` ticket is an invitation to connect to this node directly; the
    // relay can only ever backfill chunks behind it.
    ensure_p2p("an arvc… ticket")?;
    // A bare relay host gets a scheme (https unless written explicitly).
    let relay = relay.map(|r| book::normalize_relay(&r));
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
    {
        if !foreground {
            if let Some(client) = daemon_client().await {
                let base = arvolo_base_name(&paths);
                let (id, ticket) = serve_ticket_via_daemon(client, paths, seed_relay).await?;
                emit_ticket(out, &base, &ticket);
                eprintln!(
                    "Serving via the daemon — follow it with `arvolo status`, stop it with \
                     `arvolo cancel {id}`."
                );
                return Ok(());
            }
        }
    }

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
    let record_id = crate::sessions::id_for(&session.ticket);
    let result = serve_session(session, Some((out, name)), Some(record_id)).await;
    // The payload (a packed archive) had to stay readable for the whole session,
    // since chunks are produced on the fly; clean it up now that serving ended.
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    result
}

/// The stem the `.arvolo` file is named after: the first path's file name, the
/// same rule [`resolve_payload`] uses for the payload's suggested name.
fn arvolo_base_name(paths: &[PathBuf]) -> String {
    paths
        .first()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "bundle".into())
}

/// Write `<base>.arvolo` in the current directory, never clobbering: an existing
/// file gets ` (1)`, ` (2)`, … appended, the same rule downloads use.
fn write_arvolo_file(base: &str, ticket: &str) -> Result<PathBuf> {
    use std::io::Write;
    let mut n = 0u32;
    loop {
        let candidate = if n == 0 {
            PathBuf::from(format!("{base}.arvolo"))
        } else {
            PathBuf::from(format!("{base} ({n}).arvolo"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut f) => {
                f.write_all(ticket.as_bytes()).context("write ticket file")?;
                f.write_all(b"\n").context("write ticket file")?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => {
                return Err(e).with_context(|| format!("create {}", candidate.display()))
            }
        }
    }
}

/// Drive a prepared/resumed [`flow::SendSession`] to completion, emitting the
/// ticket when it's ready. Shared by fresh sends and both resume paths;
/// `emit` is `None` when the caller already announced the artefact (resume
/// reprints it itself).
pub(crate) async fn serve_session(
    session: flow::SendSession,
    emit: Option<(TicketOut, String)>,
    // The resumable-session record backing this serve, dropped once a receiver
    // has the whole file — a delivered send listed as "resumable" forever was
    // the old leak (`cancel` used to be the only remover).
    session_record: Option<String>,
) -> Result<()> {
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
                match &emit {
                    Some((out, base)) => emit_ticket(*out, base, &ticket),
                    None => {
                        // A resume: the caller said what's happening; reprint the
                        // ticket bare so it can be piped like a fresh one.
                        println!("{ticket}");
                        eprintln!("\nOn the other device:  arvolo recv <the ticket above>");
                    }
                }
                if has_relay {
                    eprintln!("P2P-first; if the receiver drops, only the missing chunks are backfilled to the relay.");
                }
                vprintln!(
                    "serving {chunks} chunk(s), {} total; waiting for a receiver to connect",
                    human_size(total_size)
                );
                eprintln!("Ctrl-C to stop.");
            }
            SendEvent::ReceiverConnected => vprintln!("receiver connected — chunk pull started"),
            SendEvent::Progress { transferred, total } if total > 0 => {
                let pct = (transferred * 100 / total) as u8;
                // Narrate every ~10% step, once each.
                if verbosity() >= 1 && pct / 10 != last_pct.load(Ordering::Relaxed) / 10 {
                    last_pct.store(pct, Ordering::Relaxed);
                    vprintln!(
                        "sent {pct}% ({}/{})",
                        human_size(transferred),
                        human_size(total)
                    );
                }
            }
            SendEvent::Progress { .. } => {}
            SendEvent::Delivered => {
                eprintln!("✓ A receiver got the whole file.");
                if let Some(id) = &session_record {
                    crate::sessions::remove_if_present(id);
                }
            }
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
pub(crate) async fn resume_by_ticket(ticket: &str, path: &Path) -> Result<()> {
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
    serve_session(session, None, None).await
}

/// `arvolo resume <id>`: replay a saved session (covers deliveries to a contact
/// too, which is why it needs no file). A
/// single file is re-served in place; an archive is repacked deterministically.
pub(crate) async fn resume_by_id(id: &str) -> Result<()> {
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
    let result = serve_session(session, None, Some(id.to_string())).await;
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    result
}

/// `arvolo send --code`: hand the ticket to the receiver via a short pairing code
/// over a relay rendezvous, serving the file (and using the relay for backfill)
/// meanwhile.
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

    // The artefact alone on stdout (`arvolo send --code f | pbcopy` copies just
    // the code); the words around it are narration and go to stderr.
    println!("{shown_code}");
    eprintln!("\nOn the other device:\n");
    eprintln!("    arvolo recv {shown_code}\n");
    if qr {
        print_qr(&shown_code);
    }
    eprintln!("Ctrl-C to stop.");

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
