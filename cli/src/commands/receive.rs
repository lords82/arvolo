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

use crate::ipc;

use crate::output::{vprintln, vvprintln};

use crate::ui::*;
use crate::util::*;

use crate::commands::daemon::{daemon_client, daemon_events};
use crate::commands::offline::recv_offline;
use crate::output::verbosity;

pub(crate) async fn listen(
    accept: Option<crate::args::AcceptWho>,
    no_sync: bool,
    relay: Option<String>,
) -> Result<()> {
    let policy = AcceptPolicy::from(accept);
    // If a daemon is already receiving, attach to it as a viewer/approver instead
    // of standing up a second engine (which would fight over presence/inbox).
    {
        if let Some(client) = daemon_client().await {
            // Say so. Two different things happen under one verb depending on
            // what else is running, and a user who can't tell which one they got
            // can't explain the difference in behaviour they're about to see.
            eprintln!("(a daemon is already listening — attaching to it as the approver)");
            return listen_attached(client, policy).await;
        }
    }

    let relay = require_relay(relay)?;
    let me = my_identity()?;
    let my_id = encode_id(&me.public());
    let download_dir = book::default_download_dir().unwrap_or_else(book::default_home_downloads);

    let manager = TransferManager::new(me, Some(relay.clone()), download_dir.clone());
    let mut events = manager.subscribe();
    let inbox = manager.spawn_inbox()?;
    let auto_sync =
        (!no_sync && book::sync_enabled()).then(|| sync::spawn_auto_sync(relay.clone()));
    if auto_sync.is_some() {
        vprintln!("multi-device address-book sync enabled");
    }
    vprintln!("inbox poller started — publishing presence and watching for offers on the relay");
    if policy.yes {
        vprintln!("auto-accepting ALL offers (--accept all)");
    } else if policy.auto_accept_verified {
        vprintln!("auto-accepting offers from verified contacts only");
    } else if policy.auto_accept_contacts {
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
                        } else if policy.yes {
                            true
                        } else if policy.auto_accept_verified && status.verified {
                            eprintln!("   (auto-accepting: verified contact)");
                            true
                        } else if policy.auto_accept_contacts && status.name.is_some() {
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

/// How `listen` decides whether to auto-accept an incoming offer (both
/// standalone and attached to a daemon).
#[derive(Clone, Copy)]
pub(crate) struct AcceptPolicy {
    pub(crate) auto_accept_contacts: bool,
    pub(crate) auto_accept_verified: bool,
    pub(crate) yes: bool,
}

impl From<Option<crate::args::AcceptWho>> for AcceptPolicy {
    fn from(accept: Option<crate::args::AcceptWho>) -> Self {
        use crate::args::AcceptWho;
        AcceptPolicy {
            auto_accept_contacts: matches!(accept, Some(AcceptWho::Contacts)),
            auto_accept_verified: matches!(accept, Some(AcceptWho::Verified)),
            yes: matches!(accept, Some(AcceptWho::All)),
        }
    }
}

/// Decide + act on one offer over the daemon IPC.
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

pub(crate) async fn listen_attached(
    mut client: ipc::client::DaemonClient,
    policy: AcceptPolicy,
) -> Result<()> {
    use ipc::protocol::{EventDto, OfferDto};

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
                    Ok(Some(EventDto::OfferReceived { id, from, name, size, note, sender_name, .. })) => {
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

/// `arvolo recv` — with something to paste (or a `.arvolo` file, or a waiting
/// offer's handle), fetch it; with nothing, show what is waiting for you and
/// take one from the list.
pub(crate) async fn recv(
    what: Option<String>,
    out: Option<PathBuf>,
    password: Option<String>,
) -> Result<()> {
    // `--password` with no value means "prompt me" (clap stores the empty string).
    let password = match password {
        Some(p) if p.is_empty() => Some(prompt_password()?),
        other => other,
    };
    // One download-dir rule for every door: with no `-o`, files land in the
    // configured download dir — the same place `listen` and the daemon save to.
    // `recv` used to write into whatever directory the shell happened to be in,
    // which made where a file ended up depend on which door it came through.
    // An explicit `-o` naming an existing file would overwrite it: that is the
    // caller's call, but on a terminal it gets asked first.
    let out = match out {
        None => {
            let d = book::default_download_dir().unwrap_or_else(book::default_home_downloads);
            std::fs::create_dir_all(&d).ok();
            Some(d)
        }
        Some(p) => {
            if p.is_file()
                && std::io::stdin().is_terminal()
                && !crate::ui::confirm_blocking(&format!(
                    "{} already exists — overwrite it?",
                    p.display()
                ))
            {
                anyhow::bail!("not overwriting {} — pick another -o path", p.display());
            }
            Some(p)
        }
    };
    let Some(what) = what else {
        return recv_waiting(out, password).await;
    };
    // A `.arvolo` ticket file: the ticket is its content.
    if let Some(ticket) = read_arvolo_file(&what)? {
        return recv_ticket(ticket, out, password).await;
    }
    // The 8-hex handle of a waiting offer (any unique prefix). Nothing else recv
    // takes has this shape: tickets carry their prefix, codes their dashes.
    if crate::handles::looks_like_handle(&what) {
        return take_offer_by_handle(&what, out, password, Decision::Accept).await;
    }
    recv_ticket(what, out, password).await
}

/// If `what` names a `.arvolo` file, the ticket inside it. Only the extension
/// makes a string a path — a ticket or code can never look like one.
fn read_arvolo_file(what: &str) -> Result<Option<String>> {
    let p = std::path::Path::new(what);
    if p.extension().and_then(|e| e.to_str()) != Some("arvolo") {
        return Ok(None);
    }
    let content = std::fs::read_to_string(p)
        .with_context(|| format!("read ticket file {}", p.display()))?;
    let t = content.trim();
    anyhow::ensure!(
        !t.is_empty(),
        "{} is empty — not a ticket file",
        p.display()
    );
    Ok(Some(t.to_string()))
}

/// What to do with the offer a handle resolves to.
pub(crate) enum Decision {
    Accept,
    Decline,
}

/// `arvolo decline <handle>` — drop a waiting offer without fetching it.
pub(crate) async fn decline_cmd(handle: String) -> Result<()> {
    take_offer_by_handle(&handle, None, None, Decision::Decline).await
}

/// Resolve a typed handle (unique prefix) against the waiting offers — the
/// daemon's parked list when one runs, the relay inbox otherwise — and accept or
/// decline the one it names.
async fn take_offer_by_handle(
    prefix: &str,
    out: Option<PathBuf>,
    password: Option<String>,
    decision: Decision,
) -> Result<()> {
    use crate::handles::{resolve_prefix, short, Match};

    if let Some(mut client) = daemon_client().await {
        let pending = client.list_pending().await?;
        match resolve_prefix(prefix, pending.iter().map(|o| (short(&o.id), o.clone()))) {
            Match::One(offer) => match decision {
                Decision::Accept => {
                    note_advertised_name(&offer.from, &offer.sender_name);
                    let id = client
                        .accept_with_password(offer.id.clone(), out, password)
                        .await?;
                    let shown = crate::commands::daemon::handle_for(&mut client, id).await;
                    println!(
                        "Accepted {} — the daemon is downloading it (transfer {shown}). \
                         Follow it with `arvolo status --watch`.",
                        sanitize_display(&offer.name)
                    );
                    Ok(())
                }
                Decision::Decline => {
                    client.reject(offer.id.clone()).await?;
                    println!(
                        "Declined {} — it's off your list.",
                        sanitize_display(&offer.name)
                    );
                    Ok(())
                }
            },
            Match::Many(hs) => anyhow::bail!(
                "'{prefix}' matches more than one waiting offer ({}) — type more of it",
                hs.join(", ")
            ),
            Match::None => anyhow::bail!(
                "no waiting offer matches '{prefix}' — `arvolo recv` lists what's waiting"
            ),
        }
    } else {
        use arvolo_core::presence::InboxSubscription;
        let relay = require_relay(None)?;
        let me = my_identity()?;
        let inbox = InboxSubscription::new(relay, &me);
        let offers = read_inbox(&inbox).await?;
        match resolve_prefix(
            prefix,
            offers.iter().enumerate().map(|(i, o)| (short(&o.id), i)),
        ) {
            Match::One(i) => {
                let chosen = &offers[i];
                match decision {
                    Decision::Accept => {
                        note_advertised_name(&encode_id(&chosen.sender), &chosen.offer.sender_name);
                        recv_ticket(chosen.offer.ticket.clone(), out, password).await?;
                        // Saved, so it can be let go of; see `waiting_from_relay`.
                        if let Err(e) = inbox.ack(&chosen.id).await {
                            eprintln!(
                                "(the file is saved, but the relay still lists the offer: {e:#})"
                            );
                        }
                        Ok(())
                    }
                    Decision::Decline => {
                        inbox.ack(&chosen.id).await.context("decline the offer")?;
                        println!(
                            "Declined {} — it won't be offered again.",
                            sanitize_display(&chosen.offer.name)
                        );
                        Ok(())
                    }
                }
            }
            Match::Many(hs) => anyhow::bail!(
                "'{prefix}' matches more than one waiting offer ({}) — type more of it",
                hs.join(", ")
            ),
            Match::None => anyhow::bail!(
                "no waiting offer matches '{prefix}' — `arvolo recv` lists what's waiting"
            ),
        }
    }
}

/// The half of `recv` that has something to fetch from. Also the resume path's
/// entry point, which always holds a ticket — it goes straight here rather than
/// through the dispatcher, where "no ticket" means something else entirely.
pub(crate) async fn recv_ticket(
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
    // Everything past here connects to the sender. Refuse before the rendezvous, not
    // after: resolving a code consumes it, and burning someone's code to then say we
    // can't fetch would cost them a second one.
    ensure_p2p("this ticket")?;
    if password.is_some() {
        eprintln!("note: --password applies only to a mailbox (arvm…) ticket — ignoring it here.");
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
                    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    crate::ui::saved(&path, bytes);
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

// ---- what's waiting for you -----------------------------------------------
//
// `arvolo recv` with nothing to paste. Only sends *addressed to an identity* can
// be listed: they arrive as offers sealed to the recipient in their inbox slot on
// the relay, so the recipient (and only the recipient — the read is authenticated
// by proof of possession of the slot key) can enumerate them. A code, ticket or
// link cannot appear here and never will: it is itself the capability to fetch,
// nothing on the relay ties one to a person, and that is precisely what stops a
// stranger from listing someone else's. So the two halves of this verb are not
// arbitrary — paste what only you were given, list what was addressed to you.

/// One row of the listing, whichever source produced it: the daemon's parked
/// offers, or a direct read of the inbox when no daemon is running. Both carry the
/// same facts, so both render through this one shape — the same reason `status`
/// and `history` share a row printer rather than growing two dialects for it.
pub(crate) struct Waiting {
    /// The sender's base32 public id. Always the key, never a display name: every
    /// trust question below is answered from this and nothing else.
    pub(crate) from: String,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) note: String,
    /// The sender's self-chosen name — a petname *claim*. Attacker-controlled text
    /// that rides inside the sealed offer: shown as unverified, never in place of
    /// the fingerprint, and never for someone we already have a name for.
    pub(crate) sender_name: String,
    /// What taking this row will actually do.
    kind: &'static str,
    /// The 8-hex handle `recv`/`decline` take, when the row has one.
    pub(crate) handle: Option<String>,
    /// Trailing hint, where the row has an id another command takes.
    hint: Option<String>,
}

/// One inbox offer as a row. Shared with `status`, which shows the same list
/// without the picker — so the two can't drift into disagreeing about what is
/// waiting or who it is from.
pub(crate) fn waiting_row(o: &arvolo_core::presence::ReceivedOffer) -> Waiting {
    Waiting {
        from: encode_id(&o.sender),
        name: o.offer.name.clone(),
        size: o.offer.size,
        note: o.offer.note.clone(),
        sender_name: o.offer.sender_name.clone(),
        // Told apart locally, from the ticket inside the sealed offer: a mailbox
        // deposit is there to be fetched now, a live ticket needs the sender
        // online at the other end. Worth saying — it decides whether "take it
        // later" is a plan or a way to miss it.
        kind: if arvolo_core::offline::OfflineTicket::decode(&o.offer.ticket).is_ok() {
            "in the relay's mailbox — fetchable now"
        } else {
            "live — the sender has to be online"
        },
        handle: Some(crate::handles::short(&o.id)),
        hint: Some(offer_hint(&crate::handles::short(&o.id))),
    }
}

/// The copy-pasteable pair of commands a waiting offer's row carries.
pub(crate) fn offer_hint(handle: &str) -> String {
    format!("arvolo recv {handle}   ·   arvolo decline {handle}")
}

/// One non-destructive read of our own inbox slot, with blocked senders dropped.
///
/// Reading does not consume: the relay drops an offer only on the recipient's
/// DELETE, so both callers can look without committing to anything. Blocked
/// senders never reach the list, exactly as they never reach the prompt in
/// `listen` — blocking that only made you look at it more slowly is not one.
pub(crate) async fn read_inbox(
    inbox: &arvolo_core::presence::InboxSubscription,
) -> Result<Vec<arvolo_core::presence::ReceivedOffer>> {
    // wait=0: whatever is queued right now, rather than the long poll a listener
    // would hold open — both callers answer a question and exit.
    let offers = inbox
        .poll_wait(0)
        .await
        .context("read the offers waiting for you on the relay")?;
    Ok(offers
        .into_iter()
        .filter(|o| !book::is_blocked(&encode_id(&o.sender)))
        .collect())
}

/// How a sender is introduced in the listing: the name *we* saved for them, or
/// else their id and what we know about it. One line per verdict, mirroring
/// [`print_sender_banner`] — an unsaved sender is never given a name to hide
/// behind, because the only name they have is one they chose for themselves.
fn describe_sender(status: &book::SenderStatus, from_b32: &str) -> String {
    match (&status.name, status.seen_before) {
        (Some(name), _) if status.verified => format!("{name}  ✓ verified"),
        (Some(name), _) => format!("{name}  (saved, not verified)"),
        (None, true) => format!("{from_b32}  (known sender, not in contacts)"),
        (None, false) => format!("{from_b32}  ⚠ NEW sender"),
    }
}

pub(crate) fn print_waiting(rows: &[Waiting]) {
    for (i, r) in rows.iter().enumerate() {
        let status = book::sender_status(&r.from);
        println!("  [{}] from {}", i + 1, describe_sender(&status, &r.from));
        println!(
            "      {}  ({})  ·  {}",
            sanitize_display(&r.name),
            human_size(r.size),
            r.kind
        );
        if !r.note.is_empty() {
            println!("      💬 {}", sanitize_display(&r.note));
        }
        // Rendering stays side-effect free — the TOFU name ledger is only touched
        // for the offer actually taken, so browsing a list can't quietly record a
        // name change for something the user then declines.
        if status.name.is_none() && !r.sender_name.is_empty() {
            println!(
                "      🏷  calls themselves \"{}\" (unverified)",
                sanitize_display(&r.sender_name)
            );
        }
        // Only where it's the thing to act on: for a verified contact the
        // fingerprint is a line of noise, for anyone else it is the check.
        if !status.verified {
            if let Some(fp) = book::fingerprint_of(&r.from) {
                println!("      fingerprint: {fp}");
            }
        }
        if let Some(h) = &r.hint {
            println!("      {h}");
        }
        println!();
    }
}

/// An empty list is not "nobody sent you anything" — it is "nothing *addressed to
/// you* is queued". Say which, and say what can never be queued, or the silence
/// reads as a bug to someone who is holding a code and waiting for it to show up.
fn print_nothing_waiting(scope: &str) {
    println!("Nothing waiting for you {scope}.");
    println!();
    println!("Only sends addressed to your identity land here (`arvolo send <you> …`).");
    println!("A code, ticket or link can't: it *is* the permission to fetch, so nothing");
    println!("on the relay knows one is yours — paste it instead:");
    println!();
    println!("    arvolo recv <code|arvc…|arvm…|link>");
}

/// Read one line from stdin. `None` on EOF — which is what a script or a
/// `< /dev/null` run gets, and the reason this prints the list and stops there
/// rather than choosing something nobody asked for.
async fn prompt_line(prompt: String) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    })
    .await
    .ok()
    .flatten()
}

/// Ask which row to take. `None` is "none of them": `q`, an empty line, or EOF.
/// What the picker was told to do with a row.
enum Pick {
    Take(usize),
    /// Decline it: drop it from the relay without fetching it.
    Drop(usize),
}

/// Ask what to do. `None` means "nothing": `q`, an empty line, or EOF.
///
/// Declining is offered because otherwise there is no way out of an unwanted
/// arrival. Leaving it alone means looking at it again on every listing until its
/// TTL lapses — a week, for something the sender may have sent by mistake — and the
/// only lever that removed it was blocking the person, which is a much larger
/// statement than "not this file".
async fn pick_one(count: usize) -> Option<Pick> {
    loop {
        let line = prompt_line(format!(
            "Take which one? [1-{count}, d<n> to decline, Enter to quit] "
        ))
        .await?;
        let line = line.trim();
        if line.is_empty() || line.eq_ignore_ascii_case("q") {
            return None;
        }
        let (rest, decline) = match line.strip_prefix(['d', 'D']) {
            Some(rest) => (rest.trim(), true),
            None => (line, false),
        };
        match rest.parse::<usize>() {
            Ok(n) if (1..=count).contains(&n) => {
                return Some(if decline {
                    Pick::Drop(n - 1)
                } else {
                    Pick::Take(n - 1)
                })
            }
            _ => eprintln!(
                "  not one of those — a number from 1 to {count}, `d` and a number to \
                 decline it, or Enter to quit."
            ),
        }
    }
}

async fn recv_waiting(out: Option<PathBuf>, password: Option<String>) -> Result<()> {
    // A running daemon already drains the inbox into its own parked list, so its
    // view is the authoritative one — reading the relay behind its back would show
    // an inbox it has emptied and hide the offers it is holding.
    if let Some(client) = daemon_client().await {
        return waiting_from_daemon(client, out, password).await;
    }
    waiting_from_relay(out, password).await
}

/// With a daemon: the offers it has parked awaiting approval — the same rows
/// `arvolo status` shows, with the choice attached.
async fn waiting_from_daemon(
    mut client: ipc::client::DaemonClient,
    out: Option<PathBuf>,
    password: Option<String>,
) -> Result<()> {
    let pending = client.list_pending().await?;
    if pending.is_empty() {
        print_nothing_waiting("(the daemon is running and watching the relay)");
        return Ok(());
    }
    let rows: Vec<Waiting> = pending
        .iter()
        .map(|o| Waiting {
            from: o.from.clone(),
            name: o.name.clone(),
            size: o.size,
            note: o.note.clone(),
            sender_name: o.sender_name.clone(),
            // The daemon holds the ticket and drives the fetch either way, so
            // which kind it is changes nothing about what accepting does here.
            kind: "held by the daemon, awaiting you",
            handle: Some(crate::handles::short(&o.id)),
            hint: Some(offer_hint(&crate::handles::short(&o.id))),
        })
        .collect();
    println!("Waiting for you ({}):\n", rows.len());
    print_waiting(&rows);

    let Some(pick) = pick_one(rows.len()).await else {
        eprintln!("(nothing taken — they stay parked; `arvolo recv <handle>` takes one later.)");
        return Ok(());
    };
    let n = match pick {
        Pick::Take(n) => n,
        Pick::Drop(n) => {
            let offer = &pending[n];
            client.reject(offer.id.clone()).await?;
            println!(
                "Declined {} — it's off your list.",
                sanitize_display(&offer.name)
            );
            return Ok(());
        }
    };
    let offer = &pending[n];
    note_advertised_name(&offer.from, &offer.sender_name);
    let id = client
        .accept_with_password(offer.id.clone(), out, password)
        .await?;
    let shown = crate::commands::daemon::handle_for(&mut client, id).await;
    println!(
        "Accepted — the daemon is downloading it (transfer {shown}). \
         Follow it with `arvolo status --watch`."
    );
    Ok(())
}

/// Without a daemon: read the inbox on the relay directly.
///
/// Nobody is polling it in this case, so an offer sits there until its TTL lapses
/// and the recipient never learns it existed — which is the gap this closes. The
/// read is non-destructive by protocol (the relay drops an offer only on our
/// DELETE), so listing costs nothing: the ack goes out below, once the file is
/// actually saved, and a failed download leaves the offer where it was.
async fn waiting_from_relay(out: Option<PathBuf>, password: Option<String>) -> Result<()> {
    use arvolo_core::presence::InboxSubscription;

    let relay = require_relay(None)?;
    let me = my_identity()?;
    let inbox = InboxSubscription::new(relay.clone(), &me);
    vprintln!("reading your inbox slot on {relay} (one round trip, no waiting)…");
    let offers = read_inbox(&inbox).await?;

    if offers.is_empty() {
        print_nothing_waiting(&format!("on {relay}"));
        return Ok(());
    }
    let rows: Vec<Waiting> = offers.iter().map(waiting_row).collect();
    println!("Waiting for you on {relay} ({}):\n", rows.len());
    print_waiting(&rows);

    let Some(pick) = pick_one(rows.len()).await else {
        eprintln!("(nothing taken — they stay on the relay until they expire.)");
        return Ok(());
    };
    let n = match pick {
        Pick::Take(n) => n,
        Pick::Drop(n) => {
            // The ack is what clears an offer, and declining is an ack with no
            // fetch in front of it. The sender is told `taken` either way: the
            // relay records that the recipient dealt with it, and what they
            // decided is not the relay's to know — nor, arguably, the sender's.
            let chosen = &offers[n];
            inbox.ack(&chosen.id).await.context("decline the offer")?;
            println!(
                "Declined {} — it won't be offered again.",
                sanitize_display(&chosen.offer.name)
            );
            return Ok(());
        }
    };
    let chosen = &offers[n];
    note_advertised_name(&encode_id(&chosen.sender), &chosen.offer.sender_name);
    recv_ticket(chosen.offer.ticket.clone(), out, password).await?;
    // Saved, so it can be let go of. Acking earlier would drop it from the relay on
    // the strength of an intention, and a fetch that then failed would have taken
    // the offer with it — the sender's only record that they sent anything.
    if let Err(e) = inbox.ack(&chosen.id).await {
        eprintln!("(the file is saved, but the relay still lists the offer: {e:#})");
    }
    Ok(())
}

// ---- offline mailbox ------------------------------------------------------
