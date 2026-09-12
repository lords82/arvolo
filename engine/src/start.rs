//! Starting the engine: everything between "this installation" and a [`Daemon`]
//! ready to answer requests.
//!
//! The desktop reaches what this builds through a Unix socket, and used to build
//! it inside the command that serves that socket. A phone has no socket and no
//! second process: it calls [`start`] in its own, holds the [`Daemon`] it gets
//! back, and calls `dispatch` on it. Nothing else about the two differs — the same
//! inbox subscription, the same trust policy on an incoming offer, the same
//! history, the same address-book watcher.
//!
//! What the caller still owns: how it is reached (a socket, or nothing), when it
//! stops (the [`CancellationToken`]), and what the announcements below do.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use arvolo_core::manager::{ManagerEvent, TransferManager};
use arvolo_ipc::protocol::{EventDto, OfferDto};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::announce::{Announce, Sender};
use crate::daemon::Daemon;
use crate::identity::{auto_accepts, encode_id, is_self, my_identity};
use crate::{book, history};

/// What the embedder decides before the engine starts.
pub struct Options {
    /// Where accepted downloads land. `None` takes the configured folder, then
    /// the platform's own.
    pub download_dir: Option<PathBuf>,
    /// The relay to listen on. Already resolved: only the caller knows whether a
    /// missing one is a flag to explain or a setting to send somebody to.
    pub relay: String,
    /// Run the address-book sync round in the background.
    pub auto_sync: bool,
    /// Where the engine's own announcements go.
    pub announce: Arc<dyn Announce>,
    /// Fires when the engine should stop.
    pub shutdown: CancellationToken,
    /// The platform's say in where finished downloads come to rest. `None` on
    /// every desktop — see [`arvolo_core::manager::SavePublisher`].
    pub publisher: Option<Arc<dyn arvolo_core::manager::SavePublisher>>,
    /// How many finished transfers to put back on the list at startup, out of the
    /// history log.
    ///
    /// `0` on every desktop, where the daemon outlives the window and its list is
    /// already the answer. A phone's engine dies with the app, so without this
    /// the mailbox is empty every time it is opened — see
    /// [`crate::history::restore_board`].
    pub restore_board: usize,
    /// How many front-ends hold an event subscription open.
    ///
    /// The embedder's to make and to maintain, because the embedder is what
    /// serves them — and it is also what reads the count, to decide whether an
    /// announcement of its own would be the second one the user gets for the same
    /// file. The engine only carries it to whoever asks.
    pub front_ends: Arc<AtomicUsize>,
}

/// A running engine.
pub struct Running {
    /// Ready to answer `dispatch`.
    pub daemon: Daemon,
    /// This identity, base32 — for whatever the caller says on the way up.
    pub my_id: String,
    /// Where downloads land, resolved.
    pub download_dir: PathBuf,
    /// Unfinished downloads picked back up from disk at startup.
    pub resumed: usize,
    /// The inbox subscription, to cancel on the way down.
    inbox: CancellationToken,
}

impl Running {
    /// Stop the inbox subscription and drop any pairing window still open.
    ///
    /// A hosted `device pair` offers this device's identity secret for as long as
    /// its window lasts. It must not outlive the engine that was offering it.
    pub fn stop(&self) {
        self.daemon.pairings.cancel_all();
        self.inbox.cancel();
    }
}

/// Bring the engine up: identity, transfer manager, inbox, the intake policy, the
/// background chores. Returns as soon as it is answering.
pub async fn start(opts: Options) -> Result<Running> {
    let Options {
        download_dir,
        relay,
        auto_sync,
        announce,
        shutdown,
        publisher,
        restore_board,
        front_ends,
    } = opts;

    let me = my_identity()?;
    let my_id = encode_id(&me.public());
    let download_dir = download_dir
        .or_else(book::default_download_dir)
        .unwrap_or_else(book::default_home_downloads);
    std::fs::create_dir_all(&download_dir).context("create download dir")?;

    // Persist resumable downloads here so a restart picks them back up.
    let state_dir = book::config_dir().join("transfers");
    let manager = TransferManager::with_state_dir(
        me,
        Some(relay.clone()),
        download_dir.clone(),
        Some(state_dir),
    );
    manager.set_display_name(book::my_display_name());
    manager.set_device_id(book::load_or_init_device());
    if let Some(p) = publisher {
        manager.set_publisher(p);
    }
    let inbox = manager.spawn_inbox()?;
    if auto_sync && book::sync_enabled() {
        // Deliberately not held: it stops when the process does, like the intake
        // task below, and a handle nobody joins is a handle nobody reads.
        crate::sync::spawn_auto_sync(relay.clone());
    }

    // Offers awaiting the user's approval. A subscribed front-end lists them and
    // accepts or rejects; the intake task below puts them here.
    let pending: Arc<Mutex<HashMap<String, OfferDto>>> = Arc::new(Mutex::new(HashMap::new()));

    // Events the engine raises itself rather than relaying from the transfer
    // manager. Made here and handed on, rather than made by the server: the
    // intake task also has things to announce — an offer taken because the sender
    // is trusted, one superseded by a re-post, one dropped because the sender is
    // blocked — and it starts before anything is serving. Without this the
    // removals it makes were invisible, and an attached window kept a row (and a
    // button) pointing at an offer that was already gone.
    let side_events: broadcast::Sender<EventDto> = broadcast::channel(64).0;

    spawn_intake(
        manager.clone(),
        pending.clone(),
        side_events.clone(),
        announce,
    );
    spawn_book_watcher(manager.clone());

    // Advertised-name rows for identities nobody points at any more. Nothing
    // produces them now — `contact_remove` clears its own row, and a sync merge
    // sweeps what a sibling device's deletion stranded — but books written by
    // older builds still carry them, and they would keep riding every snapshot.
    // One sweep per start, best-effort: it reads two small TOMLs.
    match book::prune_orphan_names() {
        Ok(n) if n > 0 => tracing::debug!("pruned {n} orphan advertised-name record(s)"),
        Err(e) => tracing::debug!("orphan-name sweep skipped: {e:#}"),
        _ => {}
    }

    // Downloads that were in flight when we last stopped — each continues from its
    // partial file on disk, no re-accept needed.
    let resumed = manager.resume_incomplete();

    // And, where the engine is not the thing that remembers, what already
    // happened. After the resume so that a transfer still in flight is the live
    // row and not a duplicate of its own history entry.
    if restore_board > 0 {
        let n = history::restore_board(&manager, restore_board);
        tracing::debug!("put {n} finished transfer(s) back on the list");
    }

    let daemon = Daemon::new(
        manager,
        Some(relay),
        download_dir.clone(),
        pending,
        front_ends,
        side_events,
        shutdown,
    );

    Ok(Running {
        daemon,
        my_id,
        download_dir,
        resumed,
        inbox,
    })
}

/// Park incoming offers, apply the trust policy, and persist finished transfers —
/// whether or not anything is attached.
fn spawn_intake(
    manager: TransferManager,
    pending: Arc<Mutex<HashMap<String, OfferDto>>>,
    side_events: broadcast::Sender<EventDto>,
    announce: Arc<dyn Announce>,
) {
    let mut events = manager.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(ManagerEvent::OfferReceived {
                    id,
                    from,
                    name,
                    size,
                    note,
                    sender_name,
                    relay,
                }) => {
                    let from_b32 = encode_id(&from);
                    // Blocked senders are dropped before anything else: no name
                    // observed, no offer parked, no announcement. Doing it here
                    // rather than at the prompt is the point — a block that still
                    // queued the offer would only move the chore, not remove it.
                    if book::is_blocked(&from_b32) {
                        tracing::debug!("dropping offer from blocked sender {from_b32}");
                        manager.reject_offer(&id).await;
                        // Say so: the arrival already reached the front-ends (they
                        // subscribe to the engine, not to this task), so silence
                        // here would leave a row for an offer that is gone. Same
                        // reason as the two removals below.
                        let _ = side_events.send(EventDto::OfferGone { id: id.clone() });
                        continue;
                    }
                    // Record any advertised-name change durably (surfaced later in
                    // `contacts list` / to an attached front-end); never blocks.
                    book::observe_advertised_name(&from_b32, &sender_name);
                    // And where to answer them. Only for a contact we already have,
                    // so an offer cannot enrol a stranger into the book.
                    if let Some(r) = book::learn_sender_relay(&from_b32, &relay) {
                        tracing::debug!("{from_b32} now listens on {r}");
                    }
                    // Supersede any older parked offer for the same (sender, file).
                    let superseded = superseded_ids(&pending.lock().unwrap(), &from_b32, &name);
                    for old in &superseded {
                        pending.lock().unwrap().remove(old);
                        manager.reject_offer(old).await;
                        // And tell the windows, which cannot see this map. Their own
                        // rule for spotting a re-post is a comparison of sender and
                        // filename; this is the fact behind it, so a row cannot
                        // outlive the offer it stands for.
                        let _ = side_events.send(EventDto::OfferGone { id: old.clone() });
                    }
                    let status = book::sender_status(&from_b32);
                    let who = status.name.clone().unwrap_or_else(|| from_b32.clone());
                    // Trusted sender → auto-download. Everyone else parks and waits
                    // for the user's approval (the default).
                    //
                    // "Everyone else" excludes ourselves. An offer that opens and
                    // authenticates as our own identity can only have been sealed by
                    // something holding our identity secret — another of our devices
                    // — so there is nobody here to be asked to trust. Asking would
                    // mean asking the user's permission to receive what the user
                    // just sent, from the other side of the same desk. It is
                    // deliberately NOT folded into `trusted` — that is a fact about
                    // the identity, not a standing decision about a contact, and the
                    // two are revoked by completely different means — but both live
                    // behind `auto_accepts`, because the front-ends are told the
                    // answer and must be told the same one.
                    let own_device = is_self(&from_b32);
                    // Set when an automatic take was refused and this offer is going
                    // back in front of the user. The windows were told, on the
                    // arrival itself, that this one needed no answer — so they drew
                    // nothing, and parking it silently would leave the question in a
                    // map nobody is looking at.
                    let mut refused_to_a_question = false;
                    if auto_accepts(&from_b32) {
                        announce.auto_downloading(&name, from_of(own_device, &who), size);
                        match manager.accept_offer(&id, None).await {
                            // Same ledger, same rule as the IPC accept: a receipt is
                            // recorded when the transfer is taken, never when the
                            // offer merely turns up.
                            Ok(_) => {
                                book::record_seen(&from_b32);
                                // The offer is spent. A front-end that drew it as an
                                // arrival has nothing more to do with it, and one
                                // that (wrongly) drew it as a question must lose the
                                // question rather than keep a button wired to an id
                                // we have consumed.
                                let _ = side_events.send(EventDto::OfferGone { id: id.clone() });
                                continue;
                            }
                            // Refused, and the engine still has the offer: the
                            // deposit wants a password, or P2P is off and this one is
                            // a direct transfer. Automatic was the wrong answer, not
                            // the file — so fall through and ask, which is where the
                            // password field lives. Asking the engine rather than
                            // reading the error keeps this right for a refusal nobody
                            // has typed yet.
                            Err(e) if manager.is_pending(&id) => {
                                announce.auto_declined(&format!("{e:#}"));
                                refused_to_a_question = true;
                            }
                            // Failed after the offer was taken: there is nothing left
                            // to accept, so there is no question to ask. Say it out
                            // loud — this is the one place where a window shows
                            // nothing by design, so silence here is a file that never
                            // arrived and never said so. The sender's next re-post is
                            // another chance.
                            Err(e) => {
                                announce.auto_download_failed(
                                    &name,
                                    from_of(own_device, &who),
                                    &format!("{e:#}"),
                                );
                                let _ = side_events.send(EventDto::OfferGone { id: id.clone() });
                                continue;
                            }
                        }
                    }
                    // Announce an *arrival*, not a re-posting. A sender whose live
                    // attempt keeps failing — the recipient's presence beacon says
                    // online, nobody ever connects — posts a fresh offer on every
                    // retry, and the list above already keeps only the newest. Saying
                    // so again each time turned one file into eighty-four
                    // notifications and eighty-four log lines, all about the same
                    // thing still waiting.
                    if superseded.is_empty() {
                        announce.offer_waiting(
                            &name,
                            from_of(own_device, &who),
                            size,
                            &crate::handles::short(&id),
                        );
                    } else {
                        tracing::debug!(
                            "offer for {name} from {who} re-posted (superseding {} earlier \
                             one(s)) — not announcing again",
                            superseded.len()
                        );
                    }
                    // Say it again, as a question this time.
                    //
                    // Only after a refused automatic take: every other offer reached
                    // the front-ends already, on the engine's own event, and a second
                    // one would draw the row twice. This one they were told to
                    // ignore. `auto` is false because that is now true — the daemon
                    // tried and is asking — and the book stamps are filled here
                    // because a side event does not pass through the fan-out that
                    // adds them.
                    if refused_to_a_question {
                        let _ = side_events.send(EventDto::OfferReceived {
                            id: id.clone(),
                            from: from_b32.clone(),
                            name: name.clone(),
                            size,
                            note: note.clone(),
                            sender_name: sender_name.clone(),
                            auto: false,
                            fingerprint: book::fingerprint_of(&from_b32).unwrap_or_default(),
                            seen_count: book::seen_count(&from_b32),
                            known_name: book::known_name_of(&from_b32)
                                .map(|n| crate::text::sanitize_display(&n))
                                .unwrap_or_default(),
                        });
                    }
                    pending.lock().unwrap().insert(
                        id.clone(),
                        OfferDto {
                            id,
                            from: from_b32,
                            name,
                            size,
                            note,
                            sender_name,
                            // Stamped from the address book on the way out
                            // (`stamp_offer`), not here: the book moves while an
                            // offer waits.
                            ..Default::default()
                        },
                    );
                }
                Ok(ManagerEvent::Completed { id, path }) => {
                    if let Some(p) = &path {
                        announce.saved(&p.display().to_string());
                    }
                    history::record_transfer(&manager, id, "completed");
                }
                Ok(ManagerEvent::Deposited { id, info }) => {
                    history::record_transfer(&manager, id, "deposited");
                    crate::deposits::record_from_event(Some(id), &info);
                }
                Ok(ManagerEvent::Failed { id, error }) => {
                    history::record_transfer(&manager, id, &format!("failed: {error}"))
                }
                Ok(ManagerEvent::Cancelled { id }) => {
                    history::record_transfer(&manager, id, "cancelled")
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Who an announcement is about.
fn from_of(own_device: bool, who: &str) -> Sender<'_> {
    Sender { own_device, who }
}

/// Poll the address book so an attached front-end is nudged to refetch, and keep
/// the advertised display name in step with it.
///
/// `arvolo contacts add/remove/verify/trust/name` runs in its own process and
/// edits the book files directly — the engine never sees the call, and a window
/// would show a stale grid until it happened to reconnect. This also covers the
/// engine's own writes (a `MarkVerified`, an approved advertised name).
///
/// The name is here rather than beside the request that changes it because it is
/// no longer only a request that changes it: it belongs to the identity, so it
/// also arrives by *merge*, from a machine in another room. Seeding the manager
/// once at startup — which is all this used to need — would leave a name set on
/// the phone missing from every offer this desktop sent until it was restarted.
fn spawn_book_watcher(manager: TransferManager) {
    tokio::spawn(async move {
        let mut last = book::book_stamp();
        let mut last_name = book::my_display_name();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let now = book::book_stamp();
            if now != last {
                last = now;
                let name = book::my_display_name();
                if name != last_name {
                    manager.set_display_name(name.clone());
                    last_name = name;
                }
                manager.notify_contacts_changed();
            }
        }
    });
}

/// The parked offers a newly arrived one replaces: same sender, same file.
///
/// A live→mailbox fallback posts a fresh offer and retracts the stale live one,
/// and a sender whose live attempt keeps failing re-posts on every retry — so the
/// id churns while the file does not, and accepting a stale copy would download
/// nothing. Keep only the newest.
///
/// Lifted out of the intake loop, which needs a relay and a real sender to reach,
/// so the rule can be tested on its own. It is also kept, independently, at the
/// other end of the socket (the desktop app drops rows by the same sender+name
/// comparison): two implementations of one rule, and this is the one that decides.
fn superseded_ids(pending: &HashMap<String, OfferDto>, from: &str, name: &str) -> Vec<String> {
    pending
        .values()
        .filter(|o| o.from == from && o.name == name)
        .map(|o| o.id.clone())
        .collect()
}

#[cfg(test)]
mod intake_tests {
    use super::superseded_ids;
    use arvolo_ipc::protocol::OfferDto;
    use std::collections::HashMap;

    fn offer(id: &str, from: &str, name: &str) -> OfferDto {
        OfferDto {
            id: id.into(),
            from: from.into(),
            name: name.into(),
            size: 1,
            ..Default::default()
        }
    }

    /// What counts as "the same file, again" — the rule that keeps a re-posting
    /// sender from filling the list with one file, and the reason an id the user
    /// was shown can stop existing while the file is still on its way.
    #[test]
    fn a_re_post_supersedes_only_the_same_sender_and_file() {
        let mut pending = HashMap::new();
        for o in [
            offer("old", "anna", "arrivo.zip"),
            offer("other-file", "anna", "altro.zip"),
            offer("other-sender", "bruno", "arrivo.zip"),
        ] {
            pending.insert(o.id.clone(), o);
        }

        assert_eq!(
            superseded_ids(&pending, "anna", "arrivo.zip"),
            vec!["old".to_string()],
            "only the same sender and the same file"
        );
        assert!(superseded_ids(&pending, "carla", "arrivo.zip").is_empty());
    }
}
