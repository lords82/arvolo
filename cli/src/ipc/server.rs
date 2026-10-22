//! The socket the daemon is reached through, and nothing else.
//!
//! What a request *means* lives in [`arvolo_engine::daemon`]: the CLI and a phone
//! answer the same way, and only one of them has a socket. What is here is the
//! accept loop, the line framing, and the subscription that turns engine events
//! into lines on a connection.

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;

use super::protocol::{Request, RequestEnvelope, Response, ServerMessage};
use arvolo_engine::daemon::dispatch;

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
/// Write everything the daemon has to say onto one subscribed connection, until
/// the client hangs up or the engine stops.
///
/// The merging, the stamping and the count of attached front-ends are the
/// engine's ([`arvolo_engine::daemon::Subscription`]) — a phone needs all three
/// and has no socket to put them on. What is left here is the writing.
async fn stream_events(daemon: &Daemon, write: &mut (impl AsyncWrite + Unpin)) {
    let mut events = daemon.subscribe();
    while let Some(ev) = events.next().await {
        if write_msg(write, &ServerMessage::Event(ev)).await.is_err() {
            return; // client hung up
        }
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
