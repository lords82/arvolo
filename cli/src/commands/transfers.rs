#[cfg(unix)]
use std::time::Duration;

use anyhow::Result;
use arvolo_ipc::protocol::DepositDto;

use crate::{book, deposits, history, sessions};

#[cfg(unix)]
use crate::ipc;

use crate::args::TransferAction;
#[cfg(unix)]
use crate::commands::daemon::{daemon_client, daemon_events, print_transfer_dto};
#[cfg(unix)]
use crate::ui::*;
use crate::util::*;

/// `arvolo transfers` — the one view. With a daemon running: its live transfers
/// (in/out) and the offers awaiting approval. Then, daemon or not, everything
/// that lives on disk: the files left on a relay, the interrupted sends that can
/// still be resumed, and the history. Anything here is taken back with `arvolo
/// cancel <id>`. `clear` wipes the history.
pub(crate) async fn transfers_cmd(watch: bool, action: Option<TransferAction>) -> Result<()> {
    match action {
        Some(TransferAction::ClearHistory) => {
            let n = history::clear()?;
            println!("Cleared {n} history record(s) — the live list, your relay deposits and your resumable sends are untouched.");
            return Ok(());
        }
        Some(TransferAction::Clear) => return clear_finished().await,
        None => {}
    }

    #[cfg(unix)]
    {
        if let Some(client) = daemon_client().await {
            return show_transfers_live(client, watch).await;
        }
        if watch {
            eprintln!(
                "(no daemon running — showing what's on disk; start `arvolo daemon` for a live view)"
            );
        }
    }
    #[cfg(not(unix))]
    let _ = watch;

    // No daemon: no live rows and no parked offers (both are the daemon's), but
    // deposits, resumable sends and history are all on disk and still ours to show.
    print_deposits(&deposits::list_dtos().await);
    print_resumable();
    print_history();
    Ok(())
}

/// `arvolo transfers clear` — close out what's over.
///
/// The list lives in the daemon (it is the engine's own state, not a file), so
/// without one there is nothing here to clear. Say that rather than reporting a
/// cheerful zero: the rows the user is looking at aren't gone, they were never
/// visible from this process.
///
/// Deliberately narrow. It leaves the history log alone — that is the permanent
/// record, and `clear-history` is where you ask for it to be forgotten — and it
/// leaves the relay alone: withdrawing a deposit is `arvolo cancel <id>`, an act
/// with consequences on another machine, never a side effect of tidying a list.
async fn clear_finished() -> Result<()> {
    #[cfg(unix)]
    {
        let Some(mut client) = daemon_client().await else {
            eprintln!(
                "(no daemon running — the transfer list is the daemon's own state, so there's\n \
                 nothing to clear here. `arvolo transfers clear-history` wipes the history log.)"
            );
            return Ok(());
        };
        let n = client.clear_finished().await?;
        println!(
            "Cleared {n} finished transfer(s) — history kept (`clear-history` wipes it), \
             deposits kept (`arvolo cancel <id>` withdraws one)."
        );
        Ok(())
    }
    #[cfg(not(unix))]
    {
        eprintln!("(no daemon on this platform yet — nothing to clear.)");
        Ok(())
    }
}

/// The "left on relay" section: files parked on a relay — public links and sealed
/// mailbox deposits — that can still be withdrawn. Silent when there are none, so
/// the common case adds no noise.
fn print_deposits(list: &[DepositDto]) {
    if list.is_empty() {
        return;
    }
    println!("left on relay — `arvolo cancel <id>` deletes it from the relay:");
    for d in list {
        let kind = if d.kind == deposits::KIND_LINK {
            "link"
        } else {
            "sealed"
        };
        let to = if d.recipient.is_empty() {
            String::new()
        } else {
            let disp = book::resolve_name(&d.recipient).unwrap_or_else(|| d.recipient.clone());
            format!("  → {disp}")
        };
        println!(
            "  ● {}  [{kind}]  {} ({}){to}",
            d.id,
            d.name,
            human_size(d.size)
        );
        if !d.link.is_empty() {
            println!("      {}", d.link);
        }
        let downloads = match d.downloads {
            Some(n) => format!("{n}/{} downloads", d.max_label),
            None => format!("max {} downloads", d.max_label),
        };
        // An expired deposit is never asked about — there is nothing left to ask —
        // so `present: None` means "expired here", not "relay unreachable". Saying
        // "unknown" would invent a doubt we don't have.
        let (when, on_relay) = if d.expired {
            ("EXPIRED".to_string(), "gone (the relay dropped it)")
        } else {
            (
                format!(
                    "expires in {}",
                    human_duration(d.expires.saturating_sub(now_unix()))
                ),
                match d.present {
                    Some(true) => "present",
                    Some(false) => "gone (downloaded / revoked)",
                    None => "unknown (relay unreachable)",
                },
            )
        };
        println!("      {when} · {downloads} · on relay: {on_relay}");
    }
}

/// Interrupted P2P sends saved locally: the ticket already handed out stays valid
/// if the send is resumed, which is the whole point of keeping them.
fn print_resumable() {
    let list = sessions::list();
    if list.is_empty() {
        return;
    }
    println!("resumable sends — `arvolo send --resume <id>`:");
    for rec in list {
        let kind = if rec.archive { "archive" } else { "file" };
        println!(
            "  {}  {}  {} chunk(s), {}  [{kind}]",
            rec.id,
            rec.name,
            rec.chunks,
            human_size(rec.total_size)
        );
    }
}

/// Print the persisted transfer history (the bottom of every view).
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

/// While `--watch` is on, how often the relay is asked about the deposits. The
/// live rows redraw on a 1s beat, but a deposit only changes when someone acts on
/// it — a per-tick poll would spend a relay round-trip a second to reprint the
/// same three lines, and `--watch` is exactly the mode left running for hours.
#[cfg(unix)]
const DEPOSIT_REFRESH: Duration = Duration::from_secs(30);

/// The live daemon view (transfers in/out + pending offers), with the deposits,
/// resumable sends and history below, and an optional `--watch` redraw loop.
#[cfg(unix)]
pub(crate) async fn show_transfers_live(
    mut client: ipc::client::DaemonClient,
    watch: bool,
) -> Result<()> {
    use std::collections::HashMap;

    use arvolo_ipc::protocol::{OfferDto, StatusDto, TransferDto};

    /// One pass over the daemon's own state. The client is a single request/response
    /// socket, so these three are sequential by construction — but they're local, and
    /// keeping them in one future lets the caller run the relay-bound deposit fetch
    /// alongside, so the view costs the slower of the two rather than their sum.
    async fn fetch(
        client: &mut ipc::client::DaemonClient,
    ) -> Result<(StatusDto, Vec<TransferDto>, Vec<OfferDto>)> {
        let st = client.status().await?;
        let transfers = client.list().await?;
        let pending = client.list_pending().await?;
        Ok((st, transfers, pending))
    }

    fn paint(
        st: &StatusDto,
        transfers: &[TransferDto],
        pending: &[OfferDto],
        samples: &mut HashMap<u64, RateEstimator>,
        rates: bool,
    ) {
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
            for t in transfers {
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
            for o in pending {
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
    }

    let mut samples: HashMap<u64, RateEstimator> = HashMap::new();

    // The deposit list is read locally rather than via the daemon: it is the same
    // `list_dtos` over the same store the daemon would run, and going direct keeps
    // it off the socket so it can overlap the daemon round-trips above.
    let (mut deposits_view, snap) = tokio::join!(deposits::list_dtos(), fetch(&mut client));
    let (st, transfers, pending) = snap?;
    paint(&st, &transfers, &pending, &mut samples, watch);
    print_deposits(&deposits_view);
    print_resumable();
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
    let mut deposits_at = std::time::Instant::now();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {
                if deposits_at.elapsed() >= DEPOSIT_REFRESH {
                    deposits_view = deposits::list_dtos().await;
                    deposits_at = std::time::Instant::now();
                }
                println!("\n---");
                let (st, transfers, pending) = fetch(&mut client).await?;
                paint(&st, &transfers, &pending, &mut samples, true);
                print_deposits(&deposits_view);
                print_resumable();
                print_history();
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
