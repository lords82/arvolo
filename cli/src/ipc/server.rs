//! Daemon side of the IPC: a `UnixListener` accept-loop that translates
//! [`Request`]s into [`TransferManager`] calls and fans engine events out to
//! subscribed connections. It holds no business logic beyond that translation —
//! policy (trust, notifications, history) lives in the daemon command itself.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use arvolo_core::manager::{ManagerEvent, TransferManager};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use super::protocol::{
    ContactDto, EventDto, HistoryDto, OfferDto, Request, RequestEnvelope, Response, ServerMessage,
    StatusDto, TransferDto,
};

/// Shared daemon state handed to every connection handler. Cheap to clone.
#[derive(Clone)]
pub struct Daemon {
    pub manager: TransferManager,
    pub relay: Option<String>,
    /// Where accepted downloads land by default (surfaced in `Status` so a UI can
    /// show the real folder in its accept dialog).
    pub download_dir: PathBuf,
    /// Offers parked awaiting the user's approval (populated by the daemon's
    /// engine task; drained on accept/reject).
    pub pending: Arc<Mutex<HashMap<String, OfferDto>>>,
}

/// Accept connections until `shutdown` fires, one task per connection.
pub async fn run(
    daemon: Daemon,
    listener: UnixListener,
    shutdown: CancellationToken,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        let d = daemon.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_conn(d, stream).await {
                                tracing::debug!("ipc connection ended: {e:#}");
                            }
                        });
                    }
                    Err(e) => tracing::warn!("ipc accept failed: {e}"),
                }
            }
        }
    }
}

async fn handle_conn(daemon: Daemon, stream: tokio::net::UnixStream) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        // The correlation id and the command parse independently — and the reply must
        // use the real id even when the command is gibberish. The client is blocked
        // reading for *its* id and skips anything else, so answering with a
        // placeholder leaves it waiting on a reply that will never come: it hangs
        // instead of learning what went wrong. The case is not hypothetical — a CLI
        // newer than the daemon sends a variant this build has never heard of, which
        // is precisely what every upgrade leaves behind until the daemon restarts.
        let env: RequestEnvelope = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(e) => {
                let id = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").and_then(serde_json::Value::as_u64))
                    .unwrap_or(0) as u32;
                write_reply(&mut write, id, Response::Error(format!("bad request: {e}"))).await?;
                continue;
            }
        };
        // Subscribe takes over the connection: reply Ok, then stream events until
        // the client disconnects. No further requests are read on this socket.
        if matches!(env.cmd, Request::Subscribe) {
            write_reply(&mut write, env.id, Response::Ok).await?;
            stream_events(&daemon, &mut write).await;
            return Ok(());
        }
        let resp = dispatch(&daemon, env.cmd).await;
        write_reply(&mut write, env.id, resp).await?;
    }
    Ok(())
}

async fn dispatch(d: &Daemon, cmd: Request) -> Response {
    match cmd {
        Request::Ping => Response::Pong,
        Request::Status => Response::Status(status(d)),
        Request::ListTransfers => {
            let mut v: Vec<TransferDto> = d.manager.list().iter().map(TransferDto::from).collect();
            v.sort_by_key(|t| t.id);
            Response::Transfers(v)
        }
        Request::ListPending => {
            let mut v: Vec<OfferDto> = d.pending.lock().unwrap().values().cloned().collect();
            v.sort_by(|a, b| a.name.cmp(&b.name));
            Response::Pending(v)
        }
        Request::ListContacts => Response::Contacts(list_contacts()),
        Request::ListDeposits => Response::Deposits(crate::deposits::list_dtos().await),
        Request::RevokeDeposit { id } => revoke_deposit(d, &id).await,
        Request::Cancel { id } => {
            d.manager.cancel(id);
            Response::Ok
        }
        // Refuse (rather than silently no-op) when the transfer isn't finished:
        // a UI that dropped the row on a bare Ok would show a list that no longer
        // matches the engine.
        Request::Remove { id } => {
            if d.manager.remove(id) {
                Response::Ok
            } else {
                Response::Error(
                    "this transfer is still in flight — cancel it before removing it".into(),
                )
            }
        }
        Request::ClearFinished => Response::Cleared(d.manager.clear_finished()),
        Request::MarkVerified { name } => match crate::book::mark_verified(&name) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::Pause { id } => {
            d.manager.pause(id);
            Response::Ok
        }
        Request::Resume { id } => {
            d.manager.resume(id);
            Response::Ok
        }
        Request::RejectOffer { offer_id } => {
            d.pending.lock().unwrap().remove(&offer_id);
            d.manager.reject_offer(&offer_id).await;
            Response::Ok
        }
        Request::AcceptOffer { offer_id, out } => {
            d.pending.lock().unwrap().remove(&offer_id);
            match d
                .manager
                .accept_offer(&offer_id, out.map(PathBuf::from))
                .await
            {
                Ok(id) => Response::TransferId(id),
                Err(e) => Response::Error(format!("{e:#}")),
            }
        }
        Request::Push { to, paths, note } => push(d, to, paths, note).await,
        Request::ServeTicket { paths, seed_relay } => serve_ticket(d, paths, seed_relay).await,
        Request::ServeCode { paths, relay, keep } => serve_code(d, paths, relay, keep).await,
        Request::CreateLink { path, ttl, max } => create_link(d, path, ttl, max).await,
        Request::Recv {
            ticket,
            out,
            password,
        } => recv_ticket(d, ticket, out, password).await,
        // Contact edits reply on their own result; the pushed `ContactsChanged`
        // event (from the daemon's book watcher) is what nudges every *other*
        // attached frontend to refetch.
        Request::AddContact { name, id } => match crate::book::contact_add(&name, &id) {
            Ok(_key_change) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::RemoveContact { name } => match crate::book::contact_remove(&name) {
            Ok(true) => Response::Ok,
            Ok(false) => Response::Error(format!("no such contact '{name}'")),
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::RenameContact { old, new } => match crate::book::contact_rename(&old, &new) {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::MarkUnverified { name } => match crate::book::unmark_verified(&name) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::MarkTrusted { who, force } => mark_trusted(who, force),
        Request::MarkUntrusted { who } => match crate::book::unmark_trusted(&who) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::Block { who } => match crate::book::mark_blocked(&who) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::Unblock { who } => match crate::book::unmark_blocked(&who) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::AcceptName { who } => match crate::book::accept_name(&who) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::ListHistory => Response::History(
            crate::history::list()
                .into_iter()
                .map(|r| HistoryDto {
                    id: r.id,
                    direction: r.direction,
                    peer: r.peer_id,
                    name: r.name,
                    total_size: r.total_size,
                    transferred: r.transferred,
                    status: r.status,
                    created: r.created,
                })
                .collect(),
        ),
        Request::ClearHistory => match crate::history::clear() {
            Ok(n) => Response::Cleared(n),
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::SetMyName { name } => match crate::book::set_my_display_name(&name) {
            Ok(()) => {
                // The engine advertises the name inside each sealed offer — flip it
                // live too, or the change would only apply after a daemon restart.
                d.manager.set_display_name(name.trim().to_string());
                Response::Ok
            }
            Err(e) => Response::Error(format!("{e:#}")),
        },
        // Handled in `handle_conn` before dispatch; reachable only if a client
        // pipelines Subscribe with other commands, which we don't support.
        Request::Subscribe => {
            Response::Error("subscribe must be the only command on a connection".into())
        }
    }
}

async fn push(d: &Daemon, to: String, paths: Vec<String>, note: String) -> Response {
    if paths.is_empty() {
        return Response::Error("provide at least one file or folder to push".into());
    }
    let recipient = match crate::book::resolve_recipient(&to) {
        Ok(r) => r,
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let (payload, name, archive, temp) = match crate::resolve_payload(&paths) {
        Ok(v) => v,
        Err(e) => return Response::Error(format!("{e:#}")),
    };

    // Subscribe *before* sending so a temp archive is cleaned up even if the
    // transfer's terminal event fires before we start watching.
    let watch = temp.clone().map(|t| (d.manager.subscribe(), t));
    match d
        .manager
        .send_to(&recipient, payload, name, archive, note)
        .await
    {
        Ok(id) => {
            if let Some((rx, t)) = watch {
                spawn_temp_cleanup(rx, id, t);
            }
            Response::TransferId(id)
        }
        Err(e) => {
            if let Some(t) = temp {
                let _ = std::fs::remove_file(t);
            }
            Response::Error(format!("{e:#}"))
        }
    }
}

async fn serve_ticket(d: &Daemon, paths: Vec<String>, seed_relay: Option<String>) -> Response {
    if paths.is_empty() {
        return Response::Error("provide at least one file or folder to serve".into());
    }
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let (payload, name, archive, temp) = match crate::resolve_payload(&paths) {
        Ok(v) => v,
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    // Subscribe before serving so the temp archive is cleaned up once the serving
    // transfer ends (it stays alive for the whole session — chunks are on the fly).
    let watch = temp.clone().map(|t| (d.manager.subscribe(), t));
    match d
        .manager
        .serve_ticket(payload, name, archive, seed_relay)
        .await
    {
        Ok((id, ticket)) => {
            if let Some((rx, t)) = watch {
                spawn_temp_cleanup(rx, id, t);
            }
            Response::Served { id, ticket }
        }
        Err(e) => {
            if let Some(t) = temp {
                let _ = std::fs::remove_file(t);
            }
            Response::Error(format!("{e:#}"))
        }
    }
}

/// Host a short pairing code in the daemon: the rendezvous *and* the ticket
/// behind it, so the terminal that asked for the code can go away.
async fn serve_code(d: &Daemon, paths: Vec<String>, relay: Option<String>, keep: bool) -> Response {
    if paths.is_empty() {
        return Response::Error("provide at least one file or folder to serve".into());
    }
    let Some(relay) = relay.or_else(crate::book::default_relay_or_builtin) else {
        return Response::Error(
            "a pairing code needs a rendezvous relay: pass --relay <host>, set ARVOLO_RELAY, \
             or configure `relay` in config.toml"
                .into(),
        );
    };
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let (payload, name, archive, temp) = match crate::resolve_payload(&paths) {
        Ok(v) => v,
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    // Same cleanup contract as `serve_ticket`: a packed archive has to stay
    // readable for the whole session, so it goes when the transfer ends.
    let watch = temp.clone().map(|t| (d.manager.subscribe(), t));
    // The code is always self-contained (`code@relay`): the daemon knows which
    // relay it claimed on, and the receiver shouldn't have to be configured.
    let max_sessions = (!keep).then_some(1);
    match d
        .manager
        .serve_code(payload, name, archive, relay, true, max_sessions)
        .await
    {
        Ok((id, code)) => {
            if let Some((rx, t)) = watch {
                spawn_temp_cleanup(rx, id, t);
            }
            Response::CodeServed { id, code }
        }
        Err(e) => {
            if let Some(t) = temp {
                let _ = std::fs::remove_file(t);
            }
            Response::Error(format!("{e:#}"))
        }
    }
}

/// Withdraw a deposit from the relay, then forget the local record. An already
/// expired one has nothing left on the relay — tidy the record and report success
/// rather than an error the user can do nothing about.
///
/// A deposit this engine made is withdrawn *through* the engine. Revoking its blob
/// directly would be half a withdrawal: the send also left an offer in the
/// recipient's inbox, and only the engine's `cancel` retracts that (and ends the
/// still-live `Deposited` row). Deleting the file while leaving the offer standing
/// would point the recipient at something that is no longer there.
async fn revoke_deposit(d: &Daemon, id: &str) -> Response {
    let Some(rec) = crate::deposits::load(id) else {
        return Response::Error("no such deposit".into());
    };

    if let Some(tid) = rec.transfer_id.filter(|t| d.manager.get(*t).is_some()) {
        // Detached and best-effort inside the engine (a dead relay must not hang the
        // row), so the record goes now: the deposit is on its way out either way, and
        // a receipt for a withdrawal already ordered would only be a lie in the list.
        d.manager.cancel(tid);
        return match crate::deposits::remove(id) {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        };
    }

    // No engine row: a one-shot CLI deposit, or one whose daemon has since restarted
    // without restoring it. The record carries what the withdrawal needs — including
    // the inbox offer, which must come down with the blob.
    match crate::deposits::withdraw(&rec).await {
        Ok(()) => Response::Ok,
        Err(e) => Response::Error(format!("{e:#}")),
    }
}

/// The saved address book, projected to serializable [`ContactDto`]s for the
/// GUI's "Persone" grid.
fn list_contacts() -> Vec<ContactDto> {
    crate::book::contact_list()
        .into_iter()
        .map(|(name, id)| ContactDto {
            fingerprint: crate::book::fingerprint_of(&id).unwrap_or_default(),
            verified: crate::book::is_verified(&id),
            trusted: crate::book::is_trusted(&id),
            blocked: crate::book::is_blocked(&id),
            display_name: crate::book::display_name_of(&id).unwrap_or_default(),
            pending_name: crate::book::pending_name_of(&id).unwrap_or_default(),
            name,
            id,
        })
        .collect()
}

/// Trust an identity to auto-download — same refusal as `arvolo contacts trust`:
/// an unverified key needs `force`, because auto-downloading from a key never
/// confirmed out-of-band is a MITM risk.
fn mark_trusted(who: String, force: bool) -> Response {
    let id = match crate::book::resolve_recipient(&who) {
        Ok(r) => crate::encode_id(&r),
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    if !crate::book::is_verified(&id) && !force {
        let fp = crate::book::fingerprint_of(&id).unwrap_or_default();
        return Response::Error(format!(
            "'{who}' isn't verified — trusting it would auto-download from a key you \
             haven't confirmed out-of-band (MITM risk). Fingerprint: {fp}. \
             Verify first, or force."
        ));
    }
    match crate::book::mark_trusted(&who) {
        Ok(_) => Response::Ok,
        Err(e) => Response::Error(format!("{e:#}")),
    }
}

/// Receive from a pasted artefact, exactly as `arvolo recv` sorts them: a short
/// pairing code is resolved to its ticket over the rendezvous first, an `arvm…`
/// offline ticket is fetched from the relay mailbox, and anything else must be an
/// `arvc…` chunked ticket fetched live. The download runs in the engine, so it
/// shows up as a normal transfer row with progress, pause and cancel.
async fn recv_ticket(
    d: &Daemon,
    ticket: String,
    out: Option<String>,
    password: Option<String>,
) -> Response {
    use arvolo_core::{chunked::ChunkTicket, code, offline::OfflineTicket};

    let ticket = ticket.trim().to_string();

    // Resolve a code to its ticket, bounded: resolution waits for the sender, and
    // an RPC that can hang forever on a typo would freeze the UI that sent it.
    let ticket = if code::looks_like_code(&ticket) {
        let default_relay = crate::book::default_relay_or_builtin();
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            code::resolve_code(&ticket, default_relay.as_deref()),
        )
        .await
        {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => return Response::Error(format!("pairing: {e:#}")),
            Err(_) => {
                return Response::Error(
                    "pairing timed out — the sender isn't answering on that code (check it, \
                     or try again while they're online)"
                        .into(),
                )
            }
        }
    } else {
        ticket
    };

    // What lands where: honour an explicit file path; treat an explicit directory
    // as "in there, under the payload's own name"; default to the download dir.
    // `recv_chunked` resolves dir→file itself, but the offline fetch does not, so
    // the file name is settled here for both.
    let (name, size, peer) = if let Ok(t) = ChunkTicket::decode(&ticket) {
        (t.name, t.total_size, None)
    } else if let Ok(t) = OfflineTicket::decode(&ticket) {
        if t.has_password() && password.as_deref().map(str::is_empty).unwrap_or(true) {
            return Response::Error(
                "this ticket is password-protected — supply the password".into(),
            );
        }
        // The payload name travels sealed, so the row starts under a claim-derived
        // one; the sender id is right in the ticket though, so the row knows who.
        let name = arvolo_core::flow::default_out(&ticket)
            .display()
            .to_string();
        let peer = arvolo_core::crypto::PublicId::from_bytes(&t.sender).ok();
        (name, t.total_size, peer)
    } else {
        return Response::Error(
            "not a ticket this daemon understands — paste an arvc… ticket, a pairing \
             code (like 4821-crater-mango), or an arvm… offline ticket"
                .into(),
        );
    };
    let safe_name = arvolo_core::flow::safe_download_name(&name).unwrap_or_else(|| name.clone());
    let out_path = match out.map(PathBuf::from) {
        Some(p) if p.is_dir() => p.join(&safe_name),
        Some(p) => p,
        None => d.download_dir.join(&safe_name),
    };
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).ok();
        }
    }

    let id = d
        .manager
        .start_download_with_password(ticket, out_path, peer, name, size, password);
    Response::TransferId(id)
}

/// Encrypt `path` (a single file, or a folder packed into an archive) locally and
/// deposit a public, browser-openable download link on the relay. The key rides
/// in the URL fragment; the relay only ever sees ciphertext. Mirrors the CLI's
/// `--link` send, including saving a revocable deposit record.
async fn create_link(d: &Daemon, path: String, ttl: Option<u64>, max: Option<u32>) -> Response {
    let relay = match d.relay.as_deref() {
        Some(r) => r.to_string(),
        None => return Response::Error("no relay configured — links need a relay".into()),
    };
    let ttl = ttl.unwrap_or(7 * 24 * 3600);
    let max = max.unwrap_or(crate::deposits::UNLIMITED);

    let (payload, _name, _archive, temp) = match crate::resolve_payload(&[PathBuf::from(path)]) {
        Ok(v) => v,
        Err(e) => return Response::Error(format!("{e:#}")),
    };

    let outcome = arvolo_core::link::deposit_link(&payload, &relay, ttl, max).await;
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    match outcome {
        Ok(out) => {
            // Save a revocable record so a GUI-created link can still be cancelled
            // with `arvolo cancel <id>`, exactly like a link made from the CLI.
            let _ = crate::deposits::save(
                crate::deposits::KIND_LINK,
                &relay,
                &out.claim,
                &out.revoke_token,
                &out.name,
                out.size,
                max,
                Some(out.link.clone()),
                None,
                crate::util::now_unix().saturating_add(ttl),
                // A link is deposited directly, not through the engine: no transfer
                // row, and no recipient, so no inbox offer. The blob is the whole job.
                None,
                None,
            );
            Response::Link(out.link)
        }
        Err(e) => Response::Error(format!("{e:#}")),
    }
}

/// Remove a packed-archive temp once its transfer reaches a terminal state.
fn spawn_temp_cleanup(
    mut rx: tokio::sync::broadcast::Receiver<ManagerEvent>,
    id: u64,
    temp: PathBuf,
) {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let done = matches!(&ev,
                        ManagerEvent::Completed { id: e, .. }
                        | ManagerEvent::Failed { id: e, .. }
                        | ManagerEvent::Cancelled { id: e }
                        | ManagerEvent::Deposited { id: e, .. } if *e == id);
                    if done {
                        let _ = std::fs::remove_file(&temp);
                        return;
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => {
                    let _ = std::fs::remove_file(&temp);
                    return;
                }
            }
        }
    });
}

async fn stream_events(daemon: &Daemon, write: &mut (impl AsyncWrite + Unpin)) {
    let mut rx = daemon.manager.subscribe();
    loop {
        match rx.recv().await {
            Ok(ev) => {
                let msg = ServerMessage::Event(EventDto::from(&ev));
                if write_msg(write, &msg).await.is_err() {
                    return; // client hung up
                }
            }
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        }
    }
}

fn status(d: &Daemon) -> StatusDto {
    let pid = d.manager.public_id();
    StatusDto {
        version: env!("CARGO_PKG_VERSION").to_string(),
        public_id: crate::encode_id(&pid),
        fingerprint: pid.fingerprint(),
        relay: d.relay.clone(),
        transfers: d.manager.list().len(),
        pending: d.pending.lock().unwrap().len(),
        download_dir: d.download_dir.display().to_string(),
        display_name: crate::book::my_display_name(),
    }
}

async fn write_msg(w: &mut (impl AsyncWrite + Unpin), msg: &ServerMessage) -> std::io::Result<()> {
    let mut line = serde_json::to_string(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    w.write_all(line.as_bytes()).await?;
    w.flush().await
}

async fn write_reply(
    w: &mut (impl AsyncWrite + Unpin),
    id: u32,
    result: Response,
) -> std::io::Result<()> {
    write_msg(w, &ServerMessage::Reply { id, result }).await
}
