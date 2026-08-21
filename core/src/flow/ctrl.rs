use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::chunked::ChunkReceiver;

/// Fetch the file described by `ticket` into `out` (default derived from the
/// ticket). Resumes a partial output, prefers P2P, falls back to the relay, and
/// releases relay chunks as they're taken. Returns the output path. If `cancel`
/// fires mid-transfer it returns early with the partial (resumable) output.
/// A supervised control channel to the sender. Unlike a one-shot `open_control`,
/// it keeps the channel connected across churn: on a drop it reconnects with
/// backoff, so the sender's `RelayHas` updates keep flowing into `on_relay` and
/// acks keep reaching the sender. It also publishes the sender's live/offline
/// state so the fetch scheduler can prefer P2P while the sender is up and lean on
/// the relay while it's down — re-evaluated per fetch attempt, not fixed at start.
pub(super) struct ControlHandle {
    /// True while a control connection to the sender is currently up.
    pub(super) sender_live: Arc<std::sync::atomic::AtomicBool>,
    /// Best-effort ack of a committed chunk to the sender (dropped while offline).
    pub(super) ack_tx: tokio::sync::mpsc::UnboundedSender<u32>,
    /// Resolves once the supervisor has flushed its queued acks and stopped. The
    /// caller awaits this after cancelling, so the final acks reach the sender
    /// before the endpoint goes away (see the flush in the cancel arm below).
    pub(super) task: tokio::task::JoinHandle<()>,
}

pub(super) fn spawn_control_supervisor(
    receiver: ChunkReceiver,
    sender_addr: iroh::EndpointAddr,
    on_relay: Arc<Mutex<std::collections::HashSet<u32>>>,
    cancel: CancellationToken,
    abort_intent: Arc<std::sync::atomic::AtomicBool>,
) -> ControlHandle {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    let sender_live = Arc::new(AtomicBool::new(false));
    let (ack_tx, mut ack_rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
    let live = sender_live.clone();
    let task = tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        // The receiver dropping `ack_tx` is our "download finished" signal: we then
        // flush the acks and exit on our own (see the `None` arm below). The caller
        // awaits that rather than cancelling us outright — cancelling mid-connect
        // would strand the acks of a small file that finished before we were even
        // connected, which is exactly how a delivered send used to hang forever.
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let opened = tokio::select! {
                _ = cancel.cancelled() => break,
                r = tokio::time::timeout(
                    Duration::from_secs(12),
                    receiver.open_control(&sender_addr, on_relay.clone()),
                ) => r,
            };
            let mut control = match opened {
                Ok(Some(c)) => c,
                _ => {
                    live.store(false, Ordering::Relaxed);
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            live.store(true, Ordering::Relaxed);
            backoff = Duration::from_secs(1);
            // A separate clone of the connection so we can await its close without
            // holding a borrow that would block `control.ack`.
            let conn = control.connection();
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        live.store(false, Ordering::Relaxed);
                        // A deliberate cancel says goodbye properly, so the
                        // sender ends its side instead of nursing the tail; a
                        // pause slips away like a network drop on purpose (the
                        // resume wants that tail kept warm). Best-effort and
                        // bounded — a sender that already vanished can't stall
                        // the user's cancel.
                        if abort_intent.load(Ordering::Relaxed) {
                            let _ = tokio::time::timeout(
                                Duration::from_secs(3),
                                control.abort(),
                            )
                            .await;
                        }
                        return;
                    }
                    _ = conn.closed() => break,
                    maybe = ack_rx.recv() => {
                        match maybe {
                            Some(idx) => {
                                if control.ack(idx).await.is_err() {
                                    break;
                                }
                            }
                            // The receiver dropped its side: it has committed every
                            // chunk it is going to, and the queue is now drained (an
                            // unbounded channel yields all pending items before it
                            // reports closed). Flush and leave — we are done.
                            //
                            // The flush matters: `ack` only *buffers* into the QUIC
                            // stream, and `Control`'s `Drop` closes the connection,
                            // discarding anything not yet on the wire. `finish()`
                            // ends the stream and waits for the sender to read it to
                            // EOF. Without this the sender sees an undelivered tail
                            // for chunks that did arrive, never concludes the send,
                            // and keeps re-offering a file already received.
                            None => {
                                let _ = control.finish().await;
                                live.store(false, Ordering::Relaxed);
                                return;
                            }
                        }
                    }
                }
            }
            live.store(false, Ordering::Relaxed);
        }
        live.store(false, Ordering::Relaxed);
    });
    ControlHandle {
        sender_live,
        ack_tx,
        task,
    }
}
