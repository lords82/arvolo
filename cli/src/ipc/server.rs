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
    ContactDto, DepositDto, EventDto, OfferDto, Request, RequestEnvelope, Response, ServerMessage,
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
        let env: RequestEnvelope = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(e) => {
                write_reply(&mut write, 0, Response::Error(format!("bad request: {e}"))).await?;
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
        Request::ListDeposits => Response::Deposits(list_deposits().await),
        Request::RevokeDeposit { id } => revoke_deposit(&id).await,
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
        Request::CreateLink { path, ttl, max } => create_link(d, path, ttl, max).await,
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

/// How long the relay gets to answer one status query while the deposit list is
/// being built. The list must open promptly even when the relay is down, so a slow
/// answer degrades to "unknown" instead of hanging whoever asked.
const CLAIM_STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// Everything this client has left on a relay and could still take back, newest
/// first (the order [`crate::deposits::list`] already guarantees). The revoke token
/// stays here: a UI needs the id, never the secret.
///
/// The local record is a *receipt*, not a status: it is written once, at deposit
/// time, and nothing ever updates it. A one-shot link that has been downloaded and
/// a sealed deposit the recipient collected both leave it untouched — the relay
/// never reports back. Listing records alone would therefore present dead entries
/// as alive, which is worse than saying nothing. So ask the relay, concurrently and
/// best-effort: unreachable leaves the live fields `None`, and the UI says it does
/// not know rather than inventing an answer.
async fn list_deposits() -> Vec<DepositDto> {
    let recs = crate::deposits::list();

    let mut set = tokio::task::JoinSet::new();
    for (i, d) in recs.iter().enumerate() {
        // An expired record has nothing left on the relay by definition — `expired`
        // already says so, and a request could only confirm it. Don't spend one.
        if d.expired() {
            continue;
        }
        let (relay, claim) = (d.relay.clone(), d.claim.clone());
        set.spawn(async move {
            let info = tokio::time::timeout(
                CLAIM_STATUS_TIMEOUT,
                arvolo_core::flow::claim_info(&relay, &claim),
            )
            .await
            .ok()
            .and_then(|r| r.ok());
            (i, info)
        });
    }
    let mut live: HashMap<usize, arvolo_core::flow::ClaimInfo> = HashMap::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((i, Some(info))) = joined {
            live.insert(i, info);
        }
    }

    recs.into_iter()
        .enumerate()
        .map(|(i, d)| {
            let info = live.get(&i);
            DepositDto {
                expired: d.expired(),
                max_label: d.max_label(),
                present: info.map(|l| l.present),
                downloads: info.and_then(|l| l.downloads),
                max_downloads: info.and_then(|l| l.max_downloads),
                id: d.id,
                kind: d.kind,
                name: d.name,
                size: d.size,
                link: d.link.unwrap_or_default(),
                recipient: d.recipient.unwrap_or_default(),
                created: d.created,
                expires: d.expires,
            }
        })
        .collect()
}

/// Withdraw a deposit from the relay, then forget the local record. An already
/// expired one has nothing left on the relay — tidy the record and report success
/// rather than an error the user can do nothing about.
async fn revoke_deposit(id: &str) -> Response {
    let Some(rec) = crate::deposits::load(id) else {
        return Response::Error("no such deposit".into());
    };
    if !rec.expired() {
        if let Err(e) =
            arvolo_core::flow::revoke_offline(&rec.relay, &rec.claim, &rec.revoke_token).await
        {
            return Response::Error(format!("{e:#}"));
        }
    }
    match crate::deposits::remove(id) {
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
            name,
            id,
        })
        .collect()
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
            // from `arvolo deposits`, exactly like a link made from the CLI.
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
                ttl,
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
                        | ManagerEvent::Deposited { id: e } if *e == id);
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
