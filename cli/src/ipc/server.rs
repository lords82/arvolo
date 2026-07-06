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
    EventDto, OfferDto, Request, RequestEnvelope, Response, ServerMessage, StatusDto, TransferDto,
};

/// Shared daemon state handed to every connection handler. Cheap to clone.
#[derive(Clone)]
pub struct Daemon {
    pub manager: TransferManager,
    pub relay: Option<String>,
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
        Request::Cancel { id } => {
            d.manager.cancel(id);
            Response::Ok
        }
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
        Request::Push { to, paths } => push(d, to, paths).await,
        Request::ServeTicket { paths, seed_relay } => serve_ticket(d, paths, seed_relay).await,
        // Handled in `handle_conn` before dispatch; reachable only if a client
        // pipelines Subscribe with other commands, which we don't support.
        Request::Subscribe => {
            Response::Error("subscribe must be the only command on a connection".into())
        }
    }
}

async fn push(d: &Daemon, to: String, paths: Vec<String>) -> Response {
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
    match d.manager.send_to(&recipient, payload, name, archive).await {
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
