use std::time::Duration;

use anyhow::Result;

use crate::{book, history};

#[cfg(unix)]
use crate::ipc;

use crate::args::TransferAction;
#[cfg(unix)]
use crate::commands::daemon::{daemon_client, daemon_events, print_transfer_dto};
use crate::ui::*;
use crate::util::*;

/// `arvolo transfers` — the unified view: with a daemon running, its live
/// transfers (in/out) + pending offers, then the persisted history below;
/// without one, just the history. `clear` wipes the history.
pub(crate) async fn transfers_cmd(watch: bool, action: Option<TransferAction>) -> Result<()> {
    if let Some(TransferAction::Clear) = action {
        let n = history::clear()?;
        println!("Cleared {n} transfer record(s).");
        return Ok(());
    }

    #[cfg(unix)]
    {
        if let Some(client) = daemon_client().await {
            return show_transfers_live(client, watch).await;
        }
        if watch {
            eprintln!(
                "(no daemon running — showing history only; start `arvolo daemon` for a live view)"
            );
        }
    }
    #[cfg(not(unix))]
    let _ = watch;

    print_history();
    Ok(())
}

/// Print the persisted transfer history (the no-daemon / below-the-live view).
pub(crate) fn print_history() {
    let list = history::list();
    if list.is_empty() {
        println!("history: (none)");
        return;
    }
    println!("history:");
    for rec in list {
        let arrow = if rec.direction == "send" {
            "→"
        } else {
            "←"
        };
        let peer = rec
            .peer_id
            .as_deref()
            .map(|id| book::resolve_name(id).unwrap_or_else(|| id.to_string()))
            .unwrap_or_else(|| "anonymous".into());
        println!(
            "  {arrow} {peer}\t{}\t{}\t{}",
            rec.name,
            human_size(rec.transferred),
            rec.status
        );
    }
}

/// Smooths a transfer's throughput with a **time-based exponentially weighted
/// moving average** — the same estimator TCP uses for RTT and Unix for load
/// average. Byte counters here jump a full 16 MiB chunk at a time, so a raw
/// delta/interval reading flaps between 0 and a spike; the EWMA converges on the
/// true sustained rate while still reacting to real changes. Using a wall-clock
/// time constant (not a fixed per-tick weight) keeps it correct under irregular
/// redraw intervals. Used only by the (unix-only) live `--watch` view.
#[cfg(unix)]
pub(crate) struct RateEstimator {
    bytes: u64,
    at: std::time::Instant,
    /// Smoothed rate in bytes/second; `None` until the first real interval.
    ewma: Option<f64>,
}

#[cfg(unix)]
impl RateEstimator {
    /// Smoothing time constant: ~90% of a step change is reflected within ~3τ.
    const TAU_SECS: f64 = 3.0;

    fn new(bytes: u64, at: std::time::Instant) -> Self {
        Self {
            bytes,
            at,
            ewma: None,
        }
    }

    /// Fold in a new cumulative byte count observed at `now`; returns the current
    /// smoothed bytes/second (once at least one interval has elapsed).
    fn observe(&mut self, bytes: u64, now: std::time::Instant) -> Option<f64> {
        let dt = now.duration_since(self.at).as_secs_f64();
        if dt <= 0.0 {
            return self.ewma;
        }
        let instant = bytes.saturating_sub(self.bytes) as f64 / dt;
        // Continuous-time EWMA weight for this (possibly irregular) interval.
        let alpha = 1.0 - (-dt / Self::TAU_SECS).exp();
        self.ewma = Some(match self.ewma {
            Some(prev) => alpha * instant + (1.0 - alpha) * prev,
            None => instant,
        });
        self.bytes = bytes;
        self.at = now;
        self.ewma
    }
}

/// The live daemon view (transfers in/out + pending offers), with the history
/// below and an optional `--watch` redraw loop.
#[cfg(unix)]
pub(crate) async fn show_transfers_live(
    mut client: ipc::client::DaemonClient,
    watch: bool,
) -> Result<()> {
    use std::collections::HashMap;

    async fn render(
        client: &mut ipc::client::DaemonClient,
        samples: &mut HashMap<u64, RateEstimator>,
        rates: bool,
    ) -> Result<()> {
        let st = client.status().await?;
        let transfers = client.list().await?;
        let pending = client.list_pending().await?;
        let ver = if st.version.is_empty() {
            "?".to_string()
        } else {
            st.version.clone()
        };
        println!(
            "daemon {ver}: {}  relay: {}",
            st.public_id,
            st.relay.as_deref().unwrap_or("-")
        );
        if transfers.is_empty() {
            println!("transfers: (none)");
        } else {
            println!("transfers:");
            let now = std::time::Instant::now();
            let (mut up, mut down) = (0f64, 0f64);
            for t in &transfers {
                let rate = if rates {
                    let est = samples
                        .entry(t.id)
                        .or_insert_with(|| RateEstimator::new(t.transferred, now));
                    est.observe(t.transferred, now)
                } else {
                    None
                };
                if let Some(r) = rate {
                    if t.direction == "send" {
                        up += r;
                    } else {
                        down += r;
                    }
                }
                print_transfer_dto(t, rate.map(|r| r as u64));
            }
            // Drop samples for transfers that are gone.
            samples.retain(|id, _| transfers.iter().any(|t| t.id == *id));
            if rates && (up >= 1.0 || down >= 1.0) {
                println!(
                    "  ── ↑ {}/s   ↓ {}/s",
                    human_size(up as u64),
                    human_size(down as u64)
                );
            }
        }
        if !pending.is_empty() {
            println!("pending offers (awaiting approval):");
            for o in &pending {
                let who = book::resolve_name(&o.from).unwrap_or_else(|| o.from.clone());
                println!(
                    "  ? {}  {} ({})  — arvolo accept {}",
                    who,
                    o.name,
                    human_size(o.size),
                    o.id
                );
            }
        }
        Ok(())
    }

    let mut samples: HashMap<u64, RateEstimator> = HashMap::new();
    render(&mut client, &mut samples, watch).await?;
    print_history();

    if !watch {
        return Ok(());
    }
    let mut events = daemon_events().await?;
    let cancel = cancel_on_ctrl_c();
    println!("\n(watching — Ctrl-C to stop)");
    // Redraw on a steady 1s beat so byte/s rates are stable; drain daemon events
    // just to notice the socket closing.
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {
                println!("\n---");
                render(&mut client, &mut samples, true).await?;
            }
            ev = events.next() => match ev {
                Ok(Some(_)) => {}       // absorbed — the 1s tick handles redraw
                Ok(None) => break,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}
