#[cfg(unix)]
use std::time::Duration;

use anyhow::Result;
use arvolo_ipc::protocol::DepositDto;

use crate::{book, deposits, history, sessions};

#[cfg(unix)]
use crate::ipc;

use crate::args::StatusAction;
#[cfg(unix)]
use crate::commands::daemon::{daemon_client, daemon_events, print_transfer_dto};
use crate::commands::receive::{print_waiting, read_inbox, waiting_row, Waiting};
#[cfg(unix)]
use crate::ui::*;
use crate::util::*;

/// `arvolo status` — everything you can still act on. With a daemon running: its
/// live transfers (in/out) and the offers awaiting approval; without one, the
/// offers read straight from the relay, since nobody else is polling for them.
/// Then, either way, what lives on disk: the files left on a relay and the
/// interrupted sends that can still be resumed. Anything here is taken back with
/// `arvolo cancel <id>`, and an offer is taken with `arvolo recv`.
///
/// What already finished is [`crate::commands::history`], not this — the split is
/// exactly "can I still do something about it?".
pub(crate) async fn status_cmd(watch: bool, action: Option<StatusAction>) -> Result<()> {
    if let Some(StatusAction::Clear) = action {
        return clear_finished().await;
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

    // Say the daemon is down before showing anything else. "Is it running?" is a
    // status question, and this view is the answer to it — leaving it out is what
    // used to send people to a separate `arvolo version` just to find out. Note
    // what's missing, too: the live rows below aren't absent because nothing is
    // happening, they're absent because nobody is watching.
    println!(
        "daemon: not running — `arvolo daemon` starts it (no live transfers below; \
         what's waiting for you is read straight from the relay)."
    );

    // No daemon: no live rows (they are the daemon's), but the offers addressed to
    // us are on the relay, and deposits and resumable sends are on disk. Both of
    // the first two are relay round trips, so they go together — the view costs the
    // slower of the two rather than their sum.
    let (deposits_view, waiting) = tokio::join!(deposits::list_dtos(), waiting_on_relay());
    print_waiting_section(waiting);
    print_deposits(&deposits_view);
    print_resumable();
    print_history_pointer();
    Ok(())
}

/// The offers waiting for us on the relay — the section a `status` without a
/// daemon was missing.
///
/// With a daemon this is already covered above: it drains the inbox and parks what
/// it can't decide alone. Without one nobody polls, so an offer sits on the relay
/// until its TTL lapses and shows up nowhere — which is exactly what this view
/// claims to cover, "everything you can still act on".
///
/// Read-only, like the rest of `status`: it lists, `arvolo recv` takes. And
/// best-effort — a relay that can't be reached degrades this to one line, the way
/// the deposit rows already degrade to "unknown", rather than failing a view whose
/// other half lives on local disk and is perfectly readable.
async fn waiting_on_relay() -> Option<(String, Result<Vec<Waiting>>)> {
    let relay = book::default_relay_or_builtin()?;
    // No identity yet ⇒ nobody can have addressed anything to us. Checked rather
    // than created: `status` is the one command that must not bring something into
    // existence as a side effect of being asked what's going on.
    if !identity_path().exists() {
        return None;
    }
    let me = my_identity().ok()?;
    let inbox = arvolo_core::presence::InboxSubscription::new(relay.clone(), &me);
    let rows = read_inbox(&inbox)
        .await
        .map(|offers| offers.iter().map(waiting_row).collect());
    Some((relay, rows))
}

fn print_waiting_section(waiting: Option<(String, Result<Vec<Waiting>>)>) {
    let Some((relay, result)) = waiting else {
        return;
    };
    match result {
        // Silent when there's nothing, like the deposits section: the common case
        // should add no noise to a view people run all day.
        Ok(rows) if rows.is_empty() => {}
        Ok(rows) => {
            println!("waiting for you on {relay} — `arvolo recv` takes one:");
            print_waiting(&rows);
        }
        // Worth a line rather than silence: an unreachable relay and an empty
        // inbox look identical from here, and only one of them means "nothing
        // arrived".
        Err(e) => {
            println!("waiting for you: couldn't ask {relay} ({e:#}) — `arvolo recv` tries again.")
        }
    }
}

/// One line saying the log exists and where it is.
///
/// The log used to be printed here in full, which on a well-used machine meant
/// answering "what's going on?" with a hundred lines of what already happened —
/// the few actionable rows buried under it. Moving it out only works if it stays
/// findable, so this line is the trade: the count is the reason to go look.
pub(crate) fn print_history_pointer() {
    let n = history::list().len();
    if n > 0 {
        println!("\nhistory: {n} record(s) — `arvolo history`");
    }
}

/// `arvolo status clear` — close out what's over.
///
/// The list lives in the daemon (it is the engine's own state, not a file), so
/// without one there is nothing here to clear. Say that rather than reporting a
/// cheerful zero: the rows the user is looking at aren't gone, they were never
/// visible from this process.
///
/// Deliberately narrow. It leaves the log alone — that is the permanent record,
/// and `arvolo history clear` is where you ask for it to be forgotten — and it
/// leaves the relay alone: withdrawing a deposit is `arvolo cancel <id>`, an act
/// with consequences on another machine, never a side effect of tidying a list.
async fn clear_finished() -> Result<()> {
    #[cfg(unix)]
    {
        let Some(mut client) = daemon_client().await else {
            eprintln!(
                "(no daemon running — the transfer list is the daemon's own state, so there's\n \
                 nothing to clear here. `arvolo history clear` forgets the log.)"
            );
            return Ok(());
        };
        let n = client.clear_finished().await?;
        println!(
            "Cleared {n} finished transfer(s) — the log is kept (`arvolo history clear` \
             forgets it), deposits kept (`arvolo cancel <id>` withdraws one)."
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
        // The address itself, not just the row: this listing is where a link or a
        // ticket is looked up when the terminal that printed it has scrolled away,
        // and "there is a deposit called 8cd63bda" is no use to someone trying to
        // hand the file over a second time. A sealed deposit shows the command to
        // pass on rather than the bare ticket, because that is what the recipient
        // has to run.
        if !d.link.is_empty() {
            println!("      {}", d.link);
        } else if !d.ticket.is_empty() {
            println!("      arvolo recv {}", d.ticket);
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
        if let Some(line) = offer_line(d.offer_status.as_deref()) {
            println!("      {line}");
        }
    }
}

/// What became of the offer this deposit left in the recipient's inbox.
///
/// The line above answers "is the file still on the relay?", which is not the
/// question a sender is actually asking. This one is: nobody has looked, their
/// machine has it, or they took it. Silent when there is nothing to say — a public
/// link leaves no offer, and a relay we couldn't ask leaves us with no answer,
/// which must not be printed as "not taken".
pub(crate) fn offer_line(status: Option<&str>) -> Option<String> {
    Some(match status? {
        // Not "hasn't seen it": what we know is that it hasn't got to them, and
        // whether anyone would have looked is not ours to say either way.
        "pending" => "recipient: it hasn't reached them yet".to_string(),
        // Deliberately not "delivered", and not "seen". Any client of theirs
        // reading the offer sets this, a listing as much as a daemon — it says the
        // offer reached a device of theirs, and nothing about a person.
        "arrived" => "recipient: arrived on their device — not taken yet".to_string(),
        "taken" => "recipient: ✓ took it".to_string(),
        // Retracted, or lapsed without being taken. An older relay also lands here
        // for an offer that *was* taken, having no way to tell the two apart.
        "gone" => "recipient: the offer is no longer on the relay".to_string(),
        _ => return None,
    })
}

/// Interrupted P2P sends saved locally: the ticket already handed out stays valid
/// if the send is resumed, which is the whole point of keeping them.
fn print_resumable() {
    let list = sessions::list();
    if list.is_empty() {
        return;
    }
    println!("resumable sends — `arvolo resume <id>`:");
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

/// One history row, in the same shape the live rows above use — direction arrow,
/// who, what, how much, how it ended. Shared so `status` and `history` can never
/// drift into two dialects for the same record.
pub(crate) fn print_history_row(rec: &crate::history::HistoryRecord) {
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
    print_history_pointer();

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
                print_history_pointer();
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

#[cfg(test)]
mod offer_line_tests {
    use super::offer_line;

    /// The distinction the whole state exists for: a client having read the offer
    /// is not the recipient having taken the file, and the line must not blur them.
    #[test]
    fn arrival_is_not_reported_as_taken() {
        let arrived = offer_line(Some("arrived")).expect("arrived says something");
        assert!(
            arrived.contains("not taken yet"),
            "an offer that only arrived is explicitly not taken: {arrived}"
        );
        assert!(
            !arrived.contains("took it"),
            "any client of theirs reading it sets `arrived`, a listing included: {arrived}"
        );
        assert!(offer_line(Some("taken")).unwrap().contains("took it"));
    }

    /// No line may claim a person looked at anything: the relay sees reads of a
    /// slot, never eyes on a screen. `seen` is the word that would smuggle that
    /// claim in, which is why it is not the name of any state.
    #[test]
    fn no_line_claims_the_recipient_looked() {
        for state in ["pending", "arrived", "taken", "gone"] {
            let line = offer_line(Some(state)).expect("every state says something");
            assert!(
                !line.contains("seen") && !line.contains("looked"),
                "{state} must not claim anything about a person's attention: {line}"
            );
        }
    }

    /// No answer is not a negative answer. A public link has no offer, and a relay
    /// that could not be asked has told us nothing — neither may print as "they
    /// haven't taken it", which is a claim about the recipient nobody made.
    #[test]
    fn nothing_is_said_when_nothing_is_known() {
        assert_eq!(offer_line(None), None);
        assert_eq!(offer_line(Some("")), None);
        assert_eq!(offer_line(Some("something-newer")), None);
    }

    #[test]
    fn the_two_ends_of_the_ladder_are_covered() {
        assert!(offer_line(Some("pending"))
            .unwrap()
            .contains("hasn't reached them"));
        assert!(offer_line(Some("gone")).unwrap().contains("no longer"));
    }
}
