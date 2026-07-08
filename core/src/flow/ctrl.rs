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
}

pub(super) fn spawn_control_supervisor(
    receiver: ChunkReceiver,
    sender_addr: iroh::EndpointAddr,
    on_relay: Arc<Mutex<std::collections::HashSet<u32>>>,
    cancel: CancellationToken,
) -> ControlHandle {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    let sender_live = Arc::new(AtomicBool::new(false));
    let (ack_tx, mut ack_rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
    let live = sender_live.clone();
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        // Once the receiver finishes and drops `ack_tx`, stop selecting on it (a
        // closed channel returns immediately) but keep the connection for
        // `RelayHas` updates until cancelled.
        let mut ack_open = true;
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
                        return;
                    }
                    _ = conn.closed() => break,
                    maybe = ack_rx.recv(), if ack_open => {
                        match maybe {
                            Some(idx) => {
                                if control.ack(idx).await.is_err() {
                                    break;
                                }
                            }
                            None => ack_open = false,
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
    }
}
