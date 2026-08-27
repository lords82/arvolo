//! The socket the daemon is reached through, and nothing else.
//!
//! What a request *means* lives in [`arvolo_engine::daemon`]: the CLI and a phone
//! answer the same way, and only one of them has a socket. What is here is the
//! accept loop, the line framing, and the subscription that turns engine events
//! into lines on a connection.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use super::protocol::{EventDto, Request, RequestEnvelope, Response, ServerMessage};
use arvolo_engine::daemon::{dispatch, stamp_offer};

// Re-exported because this crate has always spoken of `ipc::server::Daemon`, and
// the same code answering a phone under another name would not make either
// clearer.
pub use arvolo_engine::daemon::Daemon;

/// Accept connections until `shutdown` fires, one task per connection.
#[cfg(unix)]
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
/// The same loop, for a channel that works the other way round.
///
/// A listening socket exists once and hands out connections. A named pipe does
/// not: each *instance* serves exactly one client, and the server must already
/// have the next instance waiting before that client connects — otherwise a
/// client arriving in the gap is told the pipe does not exist, which reads as
/// "no daemon" and sends the caller off to start a second one. So the order here
/// matters: create the successor first, then hand the connected instance to its
/// task.
///
/// `first` is created by the caller with the first-instance flag set, which is
/// also what makes this the single-instance guard on Windows (see the daemon).
#[cfg(windows)]
pub async fn run(
    daemon: Daemon,
    first: tokio::net::windows::named_pipe::NamedPipeServer,
    shutdown: CancellationToken,
) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let name = arvolo_ipc::pipe_name();
    let mut server = first;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            connected = server.connect() => {
                if let Err(e) = connected {
                    tracing::warn!("ipc accept failed: {e}");
                    continue;
                }
                // Stand up the next instance *before* serving this one.
                let next = match ServerOptions::new().create(&name) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("cannot keep listening on {name}: {e}");
                        return Ok(());
                    }
                };
                let stream = std::mem::replace(&mut server, next);
                let d = daemon.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(d, stream).await {
                        tracing::debug!("ipc connection ended: {e:#}");
                    }
                });
            }
        }
    }
}
async fn handle_conn<S>(daemon: Daemon, stream: S) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
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
/// Fan both event sources out onto one subscribed connection: the engine's
/// [`ManagerEvent`]s, and the events the daemon raises itself (pairing progress,
/// which has no transfer behind it). A client subscribes once and gets everything.
///
/// A closed engine channel ends the stream — the engine is the daemon. A closed
/// *side* channel does not: it only means nobody currently holds a sender, which
/// is the normal state between pairings.
async fn stream_events(daemon: &Daemon, write: &mut (impl AsyncWrite + Unpin)) {
    // Held for the life of the subscription: every exit from this function runs
    // the drop, including the `return`s below, so the count cannot leak.
    let _attached = FrontEnd::new(daemon.front_ends.clone());
    let mut engine = daemon.manager.subscribe();
    let mut side = daemon.side_events.subscribe();
    loop {
        let ev = tokio::select! {
            e = engine.recv() => match e {
                Ok(ev) => stamp_offer(EventDto::from(&ev)),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            },
            e = side.recv() => match e {
                Ok(ev) => ev,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => {
                    // Re-subscribe rather than tear the connection down: losing
                    // pairing events would leave a UI's pairing sheet waiting on an
                    // outcome that can no longer reach it.
                    side = daemon.side_events.subscribe();
                    continue;
                }
            },
        };
        if write_msg(write, &ServerMessage::Event(ev)).await.is_err() {
            return; // client hung up
        }
    }
}
/// Counts one attached front-end for as long as it is alive.
struct FrontEnd(Arc<AtomicUsize>);
impl FrontEnd {
    fn new(count: Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}
impl Drop for FrontEnd {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
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
