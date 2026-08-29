//! The daemon: what a request means, and what answering it does.
//!
//! Every `Request` in the protocol is turned here into calls on the engine and
//! into the local state this installation keeps — the address book, deposits,
//! history, the config. What is *not* here is how the request arrived. The CLI
//! wraps this in a socket accept-loop; an app on a phone, which has no socket and
//! no second process to run one, calls [`dispatch`] directly.
//!
//! It holds no policy beyond that translation: trust decisions, notifications and
//! history belong to the daemon command that drives it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use arvolo_core::manager::{AcceptRefusal, ManagerEvent, TransferManager};
use arvolo_ipc::protocol::{
    ConfigDto, ConfigPatch, ContactDto, EventDto, HistoryDto, OfferDto, PresenceDto, RelayInfoDto,
    Request, Response, Setting, StatusDto, SyncDto, TransferDto,
};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::pairing::Sessions;

/// How long one presence probe may take. They run concurrently, so this is also
/// roughly the worst case for the whole batch — the same bound `arvolo contacts
/// list` uses, and for the same reason: a relay that accepts the connection and
/// then goes quiet must not hang the caller.
const PRESENCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

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
    /// Events the daemon raises itself, rather than relaying from the engine:
    /// pairing progress, which has no `ManagerEvent` behind it. Merged into the
    /// same subscriber stream so a client has one place to listen.
    pub side_events: broadcast::Sender<EventDto>,
    /// Pairing sessions in flight.
    pub pairings: Sessions,
    /// What the last address-book sync round did. In memory on purpose: a stamp
    /// restored from disk would describe a daemon run that is over, and "synced 2
    /// minutes ago" is only worth showing when it is this process that did it.
    pub sync_state: Arc<Mutex<SyncState>>,
    /// How many clients are holding an event subscription open right now.
    ///
    /// The daemon raises desktop notifications only when nobody is attached. Its
    /// own `notify` module says as much — it exists because a headless daemon has
    /// no front-end to prompt — but until this counter the condition was never
    /// checked, so a running GUI got two notifications for every offer: one from
    /// here and one of its own.
    pub front_ends: Arc<AtomicUsize>,
    /// The same token the accept loop selects on: cancelling it is how
    /// [`Request::Shutdown`] turns into a clean exit.
    pub shutdown: CancellationToken,
}

/// One stream of everything a client should hear, from both places it comes from.
///
/// The transfer manager's own events are one source; the other is what the
/// daemon raises itself — pairing progress, an offer withdrawn by the intake task
/// — which has no transfer behind it. A client subscribes once and gets both,
/// each already stamped with what the address book knows about the sender.
///
/// Holding one of these is what "a front-end is attached" means: the count the
/// embedder reads before making an announcement of its own goes up while this
/// lives and down when it drops, on every path out, so it cannot leak.
pub struct Subscription {
    engine: broadcast::Receiver<ManagerEvent>,
    side: broadcast::Receiver<EventDto>,
    /// To re-subscribe from, see [`Subscription::next`].
    side_source: broadcast::Sender<EventDto>,
    _attached: FrontEnd,
}

impl Subscription {
    /// The next event. `None` when the engine is gone, which is the end of
    /// everything this stream had to say.
    pub async fn next(&mut self) -> Option<EventDto> {
        loop {
            tokio::select! {
                e = self.engine.recv() => match e {
                    Ok(ev) => return Some(stamp_offer(EventDto::from(&ev))),
                    Err(RecvError::Lagged(_)) => continue,
                    // The engine is the daemon: there is nothing after this.
                    Err(RecvError::Closed) => return None,
                },
                e = self.side.recv() => match e {
                    Ok(ev) => return Some(ev),
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => {
                        // Re-subscribe rather than end the stream: a closed side
                        // channel only means nobody currently holds a sender, which
                        // is the normal state between pairings. Ending here would
                        // leave a UI's pairing sheet waiting on an outcome that can
                        // no longer reach it.
                        self.side = self.side_source.subscribe();
                        continue;
                    }
                },
            }
        }
    }
}

/// Counts one attached front-end for as long as it is alive.
struct FrontEnd(Arc<AtomicUsize>);

impl FrontEnd {
    fn new(count: Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(count)
    }
}

impl Drop for FrontEnd {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Answer requests with no socket in between.
///
/// This is the whole of what a shell without a daemon needs: hold a `Daemon`,
/// hand it to `DaemonClient::embedded`, and every typed method the desktop's
/// window calls over a pipe works unchanged in-process. The two clients differ
/// in one line of setup and nowhere else.
impl arvolo_ipc::client::Answer for Daemon {
    fn answer(&self, cmd: Request) -> arvolo_ipc::client::BoxFuture<'_, Response> {
        Box::pin(dispatch(self, cmd))
    }

    fn events(&self) -> Box<dyn arvolo_ipc::client::Events> {
        Box::new(self.subscribe())
    }
}

impl arvolo_ipc::client::Events for Subscription {
    fn next(&mut self) -> arvolo_ipc::client::BoxFuture<'_, Option<EventDto>> {
        Box::pin(Subscription::next(self))
    }
}

/// The outcome of the most recent sync round, for [`Request::SyncStatus`].
#[derive(Debug, Clone, Default)]
pub struct SyncState {
    pub last_sync: u64,
    pub last_merged: usize,
    pub last_error: String,
}

impl Daemon {
    /// A daemon with the auxiliary state defaulted — every field the IPC layer
    /// owns rather than the caller. Keeps `commands::daemon` from having to know
    /// about broadcast channels and session maps to build one.
    pub fn new(
        manager: TransferManager,
        relay: Option<String>,
        download_dir: PathBuf,
        pending: Arc<Mutex<HashMap<String, OfferDto>>>,
        front_ends: Arc<AtomicUsize>,
        // Made by the caller, not here: the daemon's offer-intake task announces
        // removals on it and starts before this server does. Deep enough that a
        // client which stalls briefly does not miss a pairing code; a lagged
        // receiver is skipped, never blocking the sender.
        side_events: broadcast::Sender<EventDto>,
        shutdown: CancellationToken,
    ) -> Self {
        Daemon {
            manager,
            relay,
            download_dir,
            pending,
            side_events,
            pairings: Sessions::default(),
            sync_state: Arc::new(Mutex::new(SyncState::default())),
            front_ends,
            shutdown,
        }
    }

    /// Listen to everything this daemon has to say. See [`Subscription`].
    pub fn subscribe(&self) -> Subscription {
        Subscription {
            engine: self.manager.subscribe(),
            side: self.side_events.subscribe(),
            side_source: self.side_events.clone(),
            _attached: FrontEnd::new(self.front_ends.clone()),
        }
    }
}

pub async fn dispatch(d: &Daemon, cmd: Request) -> Response {
    match cmd {
        Request::Ping => Response::Pong,
        Request::Status => Response::Status(status(d)),
        // Answer first, then go: the reply is already queued on this connection
        // when the accept loop sees the cancelled token.
        Request::Shutdown => {
            tracing::info!("shutdown requested over IPC");
            d.shutdown.cancel();
            Response::Ok
        }
        Request::ListTransfers => {
            let mut v: Vec<TransferDto> = d.manager.list().iter().map(TransferDto::from).collect();
            v.sort_by_key(|t| t.id);
            Response::Transfers(v)
        }
        Request::ListPending => {
            let mut v: Vec<OfferDto> = {
                let map = d.pending.lock().unwrap();
                map.values().cloned().collect()
            };
            // Outside the lock: each stamp reads three small TOMLs.
            let mut v: Vec<OfferDto> = v.drain(..).map(stamp_offer_dto).collect();
            v.sort_by(|a, b| a.name.cmp(&b.name));
            Response::Pending(v)
        }
        Request::ListContacts => Response::Contacts(list_contacts()),
        Request::ListDeposits => Response::Deposits(crate::deposits::list_dtos().await),
        Request::RevokeDeposit { id } => revoke_deposit(d, &id).await,
        // A bare `Ok` for an id the engine has never heard of is what left front
        // ends showing "cancelling…" for ever: they take the Ok as "it is being
        // torn down" and then wait for a `Cancelled` event that nothing will emit.
        Request::Cancel { id } => {
            if d.manager.cancel(id) {
                Response::Ok
            } else {
                Response::Error(
                    "no such transfer — it has already finished, or belonged to an \
                     earlier daemon"
                        .into(),
                )
            }
        }
        // Refuse (rather than silently no-op) when the transfer isn't finished:
        // a UI that dropped the row on a bare Ok would show a list that no longer
        // matches the engine.
        Request::Remove { id } => {
            // Read before removing: after it the row is gone and there is nothing
            // left to match the log entry against.
            let row = d.manager.get(id);
            if d.manager.remove(id) {
                if let Some(t) = row {
                    crate::history::hide(&t);
                }
                Response::Ok
            } else {
                Response::Error(
                    "this transfer is still in flight — cancel it before removing it".into(),
                )
            }
        }
        Request::ClearFinished => {
            // Same reason, in bulk: whatever the sweep is about to drop has to
            // be marked before it is dropped, or the next launch puts all of it
            // back.
            let going: Vec<_> = d
                .manager
                .list()
                .into_iter()
                .filter(|t| {
                    use arvolo_core::manager::TransferStatus as S;
                    matches!(t.status, S::Completed | S::Failed(_) | S::Cancelled)
                })
                .collect();
            let n = d.manager.clear_finished();
            for t in going {
                crate::history::hide(&t);
            }
            Response::Cleared(n)
        }
        Request::MarkVerified { name } => match crate::book::mark_verified(&name) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        // Both report their refusal, for the reason spelled out on `DepositNow`
        // below: an `Ok` that changed nothing is a front-end telling the user
        // something happened. Pausing is refused for a transfer that has nothing to
        // pick up again — a mailbox pickup, a share — and resuming for one that is
        // not paused, which is what the play button on a *stalled* row used to ask
        // for and never got.
        Request::Suspend => {
            let n = d.manager.suspend_running().len();
            tracing::info!("suspending: stopped {n} transfer(s) before sleep");
            Response::Ok
        }
        Request::ResumeSuspended => {
            let n = d.manager.resume_suspended().len();
            if n > 0 {
                tracing::info!("back from sleep: resumed {n} transfer(s)");
            }
            Response::Ok
        }
        Request::Pause { id } => {
            if d.manager.pause(id) {
                Response::Ok
            } else {
                Response::Error(
                    "that transfer cannot be held: a mailbox pickup starts over rather \
                     than resuming, and a share is not a delivery to hold"
                        .into(),
                )
            }
        }
        Request::Resume { id } => {
            if d.manager.resume(id) {
                Response::Ok
            } else {
                Response::Error("that transfer is not paused".into())
            }
        }
        // Unlike its neighbours, this one reports the refusal. `deposit_now` says
        // no for a send that is not waiting — one already under way, or already
        // parked — and an `Ok` there would be a front-end telling the user their
        // file is on the relay when nothing happened at all.
        Request::DepositNow { id } => {
            if d.manager.deposit_now(id) {
                Response::Ok
            } else {
                Response::Error(
                    "that transfer is not waiting for the recipient to accept — \
                     only a waiting send can be parked on the relay"
                        .to_string(),
                )
            }
        }
        Request::RejectOffer { offer_id } => {
            d.pending.lock().unwrap().remove(&offer_id);
            d.manager.reject_offer(&offer_id).await;
            // Tell every attached front-end the row is gone: the decline may
            // have come from the CLI while a window shows the same offer.
            let _ = d.side_events.send(EventDto::OfferGone {
                id: offer_id.clone(),
            });
            Response::Ok
        }
        Request::AcceptOffer {
            offer_id,
            out,
            password,
            typed_refusal,
        } => {
            match d
                .manager
                .accept_offer_with_password(
                    &offer_id,
                    out.map(PathBuf::from),
                    password.filter(|p| !p.is_empty()),
                )
                .await
            {
                Ok(id) => {
                    // Drop the daemon's own copy only once the engine has taken
                    // the offer. Dropping it first made a *refused* accept — a
                    // password-protected deposit with no password — erase the row
                    // from `ListPending` while the engine still held it, leaving
                    // nothing to retry against.
                    let taken = d.pending.lock().unwrap().remove(&offer_id);
                    // The TOFU ledger, recorded here because *this* is the receipt:
                    // an offer merely arriving must not count, or a stranger could
                    // walk their own "you know me" counter up by re-posting, and a
                    // defence would become a lever.
                    if let Some(o) = taken {
                        crate::book::record_seen(&o.from);
                    }
                    let _ = d.side_events.send(EventDto::OfferGone {
                        id: offer_id.clone(),
                    });
                    Response::TransferId(id)
                }
                // A refusal is not a failure, and a client that said it can tell
                // them apart gets to. The sentence travels too, so a client that
                // does not know the code still has something to show.
                Err(e) => match AcceptRefusal::of(&e).filter(|_| typed_refusal) {
                    Some(r) => Response::Refused {
                        code: r.code().to_string(),
                        message: r.text().to_string(),
                    },
                    None => Response::Error(format!("{e:#}")),
                },
            }
        }
        Request::Push {
            to,
            paths,
            note,
            deposit,
            ttl,
            max,
            password,
            device,
            fd_socket,
            fd_token,
        } => {
            push(
                d,
                to,
                paths,
                note,
                MailboxOpts {
                    deposit,
                    ttl,
                    max,
                    password,
                    device,
                },
                (fd_socket, fd_token),
            )
            .await
        }
        Request::ListDevices => list_devices(),
        Request::RenameThisDevice { name } => rename_this_device(name),
        Request::ForgetDevice { who } => forget_device(who),
        Request::ServeTicket {
            paths,
            seed_relay,
            fd_socket,
            fd_token,
        } => serve_ticket(d, paths, seed_relay, (fd_socket, fd_token)).await,
        Request::ServeCode {
            paths,
            relay,
            keep,
            fd_socket,
            fd_token,
        } => serve_code(d, paths, relay, keep, (fd_socket, fd_token)).await,
        Request::CreateLink {
            path,
            ttl,
            max,
            fd_socket,
            fd_token,
        } => create_link(d, path, ttl, max, (fd_socket, fd_token)).await,
        Request::Recv {
            ticket,
            out,
            password,
        } => recv_ticket(d, ticket, out, password).await,
        // Contact edits reply on their own result; the pushed `ContactsChanged`
        // event (from the daemon's book watcher) is what nudges every *other*
        // attached frontend to refetch.
        Request::AddContact { name, id, rebind } => add_contact(&name, &id, rebind),
        Request::RemoveContact { name } => {
            // Clear the trust ledgers FIRST, exactly as the CLI does — they
            // resolve the id via the contact name, which is gone once removed.
            // Skipping this (as this handler used to) left a GUI remove with
            // orphan verified/trusted marks the CLI path would have cleared.
            crate::book::unmark_verified(&name).ok();
            crate::book::unmark_trusted(&name).ok();
            match crate::book::contact_remove(&name) {
                Ok(true) => Response::Ok,
                Ok(false) => Response::Error(format!("no such contact '{name}'")),
                Err(e) => Response::Error(format!("{e:#}")),
            }
        }
        Request::RenameContact { old, new } => match crate::book::contact_rename(&old, &new) {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::MarkUnverified { name } => match crate::book::unmark_verified(&name) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::MarkTrusted { who, force } => mark_trusted(who, force),
        Request::MarkUntrusted { who } => match crate::book::unmark_trusted(&who) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::Block { who } => match crate::book::mark_blocked(&who) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::Unblock { who } => match crate::book::unmark_blocked(&who) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::AcceptName { who } => match crate::book::accept_name(&who) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::ListHistory => Response::History(
            crate::history::list()
                .into_iter()
                .map(|r| HistoryDto {
                    id: r.id,
                    direction: r.direction,
                    peer: r.peer_id,
                    name: r.name,
                    total_size: r.total_size,
                    transferred: r.transferred,
                    status: r.status,
                    created: r.created,
                })
                .collect(),
        ),
        Request::ClearHistory => match crate::history::clear() {
            Ok(n) => Response::Cleared(n),
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::GetConfig => Response::Config(config_dto(d)),
        Request::SetConfig(patch) => match apply_config_patch(d, patch) {
            Ok(()) => Response::Config(config_dto(d)),
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::Presence { ids } => Response::Presence(presence(d, ids).await),
        Request::RelayInfo => Response::RelayInfo(relay_info(d).await),
        Request::SyncStatus => match sync_dto(d) {
            Ok(s) => Response::Sync(s),
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::SyncNow => sync_now(d).await,
        Request::StartPairing {
            kind,
            relay,
            code,
            name,
        } => {
            let session =
                super::pairing::start(&d.pairings, d.side_events.clone(), kind, relay, code, name);
            Response::PairingStarted { session }
        }
        Request::CancelPairing { session } => {
            // A handle this daemon doesn't know is not an error worth surfacing: the
            // session finishing and the UI closing its sheet race by nature, and the
            // user's intent — "stop pairing" — is satisfied either way.
            d.pairings.cancel(&session);
            Response::Ok
        }
        Request::SetMyName { name } => match crate::book::set_my_display_name(&name) {
            Ok(()) => {
                // The engine advertises the name inside each sealed offer — flip it
                // live too, or the change would only apply after a daemon restart.
                d.manager.set_display_name(name.trim().to_string());
                Response::Ok
            }
            Err(e) => Response::Error(format!("{e:#}")),
        },
        // Handled in `handle_conn` before dispatch; reachable only if a client
        // pipelines Subscribe with other commands, which we don't support.
        Request::Subscribe => {
            Response::Error("subscribe must be the only command on a connection".into())
        }
    }
}

/// What a [`Request::Push`] asked for beyond "send this to them".
struct MailboxOpts {
    deposit: bool,
    ttl: Option<u64>,
    max: Option<u32>,
    password: Option<String>,
    /// Which of our own machines to aim at, by name or id prefix.
    device: Option<String>,
}

async fn push(
    d: &Daemon,
    to: String,
    paths: Vec<String>,
    note: String,
    opts: MailboxOpts,
    fds: (Option<String>, Option<String>),
) -> Response {
    if paths.is_empty() {
        return Response::Error("provide at least one file or folder to push".into());
    }
    // `me` resolves here too, not only in `arvolo send --to me`: a front-end that
    // pushes through the daemon has no other way to name its own devices, and a
    // second copy of the rule is how the two would drift.
    // The relay comes from the book here rather than over IPC: the daemon owns the
    // address book, so a front-end naming a contact gets the same routing as the
    // CLI without having to know about relays at all.
    let (recipient, theirs) = match crate::identity::resolve_recipient_or_self_with_relay(&to) {
        Ok(r) => r,
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    // Aiming at a device only means anything inside our own identity: the register
    // holds *our* machines, and somebody else's are not ours to name. Refusing is
    // the point — accepting would post an offer stamped with an id nothing on the
    // recipient's side matches, so every one of their devices would skip it and
    // the file would sit on the relay until its TTL, delivered to no one.
    let target = match &opts.device {
        None => None,
        Some(_)
            if crate::identity::encode_id(&recipient)
                != crate::identity::encode_id(&d.manager.public_id()) =>
        {
            return Response::Error(
                "a device can only be named when sending to your own devices".into(),
            )
        }
        Some(who) => match crate::book::resolve_device(who) {
            Ok(id) => Some(id),
            Err(e) => return Response::Error(format!("{e:#}")),
        },
    };
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let (payload, name, archive, temp) = match resolve_payload_for(&paths, &fds) {
        Ok(v) => v,
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    // Name and archive decision came from the real paths above; only what the
    // engine will READ becomes the handed-off descriptor, when there is one.
    let source = adopt_source(fds.0, fds.1, temp.is_none(), payload).await;

    // Subscribe *before* sending so a temp archive is cleaned up even if the
    // transfer's terminal event fires before we start watching.
    let watch = temp.clone().map(|t| (d.manager.subscribe(), t));

    // A forced deposit is a different engine call, not a flag on the same one: it
    // skips the presence check and the held/retry loop entirely (see
    // `TransferManager::deposit_to`). The ticket comes back so a UI can offer the
    // `arvm…` for hand-delivery, exactly as `arvolo send --mailbox` prints it.
    let outcome = if opts.deposit {
        let mailbox = arvolo_core::manager::MailboxOpts {
            ttl: opts.ttl,
            max: opts.max,
            password: opts.password.filter(|p| !p.is_empty()),
            target,
        };
        d.manager
            .deposit_to(&recipient, source, name, note, mailbox, theirs)
            .await
            .map(|(id, ticket)| (id, Some(ticket)))
    } else {
        d.manager
            .send_to(&recipient, source, name, archive, note, target, theirs)
            .await
            .map(|id| (id, None))
    };

    match outcome {
        Ok((id, ticket)) => {
            if let Some((rx, t)) = watch {
                spawn_temp_cleanup(rx, id, t);
            }
            match ticket {
                Some(ticket) => Response::Served { id, ticket },
                None => Response::TransferId(id),
            }
        }
        Err(e) => {
            if let Some(t) = temp {
                let _ = std::fs::remove_file(t);
            }
            Response::Error(format!("{e:#}"))
        }
    }
}

/// The device register, most recently seen first.
///
/// Read straight from the ledger the sync round projects, so it is whatever the
/// last merge left behind — a machine that has never completed a round is not in
/// it, and one that stopped syncing stays until somebody forgets it.
fn list_devices() -> Response {
    let mine = crate::book::load_or_init_device();
    let mine_key = arvolo_core::sync::device_key(&mine);
    let mut out: Vec<arvolo_ipc::protocol::DeviceDto> = crate::book::load_devices()
        .devices
        .into_iter()
        .map(|(id, r)| arvolo_ipc::protocol::DeviceDto {
            is_this_device: id == mine_key,
            id,
            name: r.name,
            platform: r.platform,
            last_seen: r.last_seen,
        })
        .collect();
    // This machine first, then by how recently the others were heard from: the
    // row you can act on, then the ones most likely to be meant.
    out.sort_by(|a, b| {
        b.is_this_device
            .cmp(&a.is_this_device)
            .then(b.last_seen.cmp(&a.last_seen))
            .then(a.name.cmp(&b.name))
    });
    Response::Devices(out)
}

/// Rename this machine — and only this one, which is the whole shape of the
/// register: a row belongs to the device that wrote it.
fn rename_this_device(name: String) -> Response {
    let name = crate::text::sanitize_display(name.trim());
    if name.is_empty() {
        return Response::Error("give this device a name".into());
    }
    let key = arvolo_core::sync::device_key(&crate::book::load_or_init_device());
    let mut devices = crate::book::load_devices();
    let row = devices.devices.entry(key).or_default();
    row.name = name;
    if row.platform.is_empty() {
        row.platform = std::env::consts::OS.to_string();
    }
    match crate::book::save_devices(&devices) {
        // The sidecar picks the edit up on the next round, the same way a contact
        // renamed with `arvolo contacts` reaches the other devices.
        Ok(()) => Response::Ok,
        Err(e) => Response::Error(format!("{e:#}")),
    }
}

/// Retire a machine from the register. Not a revocation: see the request.
fn forget_device(who: String) -> Response {
    let id = match crate::book::resolve_device(&who) {
        Ok(id) => id,
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    if id == crate::book::load_or_init_device() {
        return Response::Error(
            "this is the device you are on — forgetting it here would only make it \
             announce itself again"
                .into(),
        );
    }
    match crate::book::forget_device(&id) {
        Ok(()) => Response::Ok,
        Err(e) => Response::Error(format!("{e:#}")),
    }
}

/// The settings screen's view of this daemon: what is in force, where it came
/// from, and what the file itself says. See [`ConfigDto`] on why both.
fn config_dto(d: &Daemon) -> ConfigDto {
    let file = crate::book::config_snapshot();
    let env_relay = std::env::var("ARVOLO_RELAY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let configured = file.relay.clone().unwrap_or_default();
    let relay_source = if env_relay.is_some() {
        "env"
    } else if !configured.trim().is_empty() {
        "config"
    } else if d.relay.is_some() {
        "builtin"
    } else {
        "none"
    };
    let env_dir = std::env::var("ARVOLO_DOWNLOAD_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty());

    ConfigDto {
        relay: d.relay.clone(),
        relay_configured: configured,
        relay_source: relay_source.into(),
        download_dir: d.download_dir.display().to_string(),
        download_dir_configured: file.download_dir.unwrap_or_default(),
        download_dir_from_env: env_dir.is_some(),
        display_name: crate::book::my_display_name(),
        sync: crate::book::sync_enabled(),
        // Resolved rather than echoed back: what a window has to draw is the rule
        // in force, and an unset key is a rule all the same.
        auto_accept: crate::identity::AutoAccept::configured()
            .as_str()
            .to_string(),
        seed: file.seed,
        swarm: file.swarm.unwrap_or_default(),
        concurrency: file.concurrency,
        config_path: crate::book::config_path().display().to_string(),
        identity_path: crate::identity::identity_path().display().to_string(),
    }
}

/// Write a [`ConfigPatch`] into `config.toml`, key by key.
///
/// Most keys only take effect on the next daemon start, and a UI is expected to
/// say so — rewriting the running engine's relay under a live transfer is not
/// something a settings screen should do silently. The display name is the one
/// exception: it is advertised per-offer, so it is flipped live here as
/// [`Request::SetMyName`] does.
fn apply_config_patch(d: &Daemon, patch: ConfigPatch) -> anyhow::Result<()> {
    use crate::book::set_config_value;

    fn text(s: &Setting<String>) -> Option<toml::Value> {
        match s {
            Setting::Set(v) if !v.trim().is_empty() => {
                Some(toml::Value::String(v.trim().to_string()))
            }
            // An empty string is how a text field says "cleared"; treating it as a
            // value would write `relay = ""`, which reads back as a configured
            // relay that is the empty string.
            _ => None,
        }
    }

    if let Some(s) = &patch.relay {
        set_config_value("relay", text(s))?;
    }
    if let Some(s) = &patch.download_dir {
        set_config_value("download_dir", text(s))?;
    }
    if let Some(s) = &patch.swarm {
        set_config_value("swarm", text(s))?;
    }
    if let Some(s) = &patch.auto_accept {
        set_config_value("auto_accept", text(s))?;
    }
    if let Some(s) = &patch.sync {
        set_config_value(
            "sync",
            match s {
                Setting::Set(v) => Some(toml::Value::Boolean(*v)),
                Setting::Clear => None,
            },
        )?;
    }
    if let Some(s) = &patch.seed {
        set_config_value(
            "seed",
            match s {
                Setting::Set(v) => Some(toml::Value::Boolean(*v)),
                Setting::Clear => None,
            },
        )?;
    }
    if let Some(s) = &patch.concurrency {
        set_config_value(
            "concurrency",
            match s {
                Setting::Set(v) => Some(toml::Value::Integer(i64::from(*v))),
                Setting::Clear => None,
            },
        )?;
    }
    if let Some(s) = &patch.display_name {
        let name = match s {
            Setting::Set(v) => v.trim().to_string(),
            Setting::Clear => String::new(),
        };
        crate::book::set_my_display_name(&name)?;
        d.manager.set_display_name(name);
    }
    Ok(())
}

/// Ask the relay which of `ids` has a live presence beacon.
///
/// Every answer is an `Option` and stays one: a probe that errored means "could
/// not ask", which is a third state and not the same as being away. Collapsing
/// it into `false` is what makes an unreachable relay look exactly like everyone
/// having gone home — the row would read "offline" with total confidence about
/// something nobody actually checked.
async fn presence(d: &Daemon, ids: Vec<String>) -> Vec<PresenceDto> {
    let unknown = |ids: Vec<String>| {
        ids.into_iter()
            .map(|id| PresenceDto { id, online: None })
            .collect()
    };
    let Some(relay) = d.relay.clone() else {
        return unknown(ids);
    };
    let client = arvolo_core::http::client_with_timeout(PRESENCE_TIMEOUT);

    let mut set = tokio::task::JoinSet::new();
    for id in ids {
        let Ok(pk) = crate::book::decode_id(&id) else {
            // Not a public id at all: nothing to ask about, and reporting it as
            // offline would be a claim we have no basis for.
            set.spawn(async move { PresenceDto { id, online: None } });
            continue;
        };
        let (client, relay) = (client.clone(), relay.clone());
        set.spawn(async move {
            PresenceDto {
                online: arvolo_core::presence::check_online(&client, &relay, &pk)
                    .await
                    .ok(),
                id,
            }
        });
    }

    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(dto) = joined {
            out.push(dto);
        }
    }
    out
}

/// Probe the configured relay and report what it is.
///
/// Never an error: "I could not reach it" and "it would not say" are both real
/// answers, and the caller — the desktop app deciding whether to offer an
/// update — needs to be able to tell them apart from each other rather than
/// from a failure. A daemon with no relay at all answers `url: None` and the
/// rest false/None, which reads correctly without a special case.
///
/// Not cached. It is asked once a week by one window, and a cached answer here
/// would go stale exactly when it matters most: right after somebody upgrades
/// the relay because the app told them to.
async fn relay_info(d: &Daemon) -> RelayInfoDto {
    let Some(url) = d.relay.clone() else {
        return RelayInfoDto {
            url: None,
            reachable: false,
            version: None,
        };
    };
    let probe = arvolo_core::code::relay_probe(&url).await;
    RelayInfoDto {
        url: Some(url),
        reachable: probe.reachable,
        version: probe.version,
    }
}

fn sync_dto(d: &Daemon) -> anyhow::Result<SyncDto> {
    let pid = d.manager.public_id();
    let st = d.sync_state.lock().unwrap().clone();
    Ok(SyncDto {
        fingerprint: pid.fingerprint(),
        public_id: crate::identity::encode_id(&pid),
        contacts: crate::book::contact_list().len(),
        enabled: crate::book::sync_enabled(),
        last_sync: st.last_sync,
        last_merged: st.last_merged,
        last_error: st.last_error,
        paired: crate::book::paired_with_another_device(),
    })
}

/// Run one sync round on demand and report the state it left behind. The round's
/// own error is recorded rather than returned: a UI that asked "sync now" wants
/// the panel to say what happened, not to lose the rest of the summary because
/// the relay was briefly unreachable.
async fn sync_now(d: &Daemon) -> Response {
    let outcome = crate::sync::sync_round(d.relay.clone()).await;
    {
        let mut st = d.sync_state.lock().unwrap();
        match &outcome {
            Ok(merged) => {
                st.last_sync = crate::now_unix();
                st.last_merged = *merged;
                st.last_error = String::new();
            }
            Err(e) => st.last_error = format!("{e:#}"),
        }
    }
    match sync_dto(d) {
        Ok(s) => Response::Sync(s),
        Err(e) => Response::Error(format!("{e:#}")),
    }
}

async fn serve_ticket(
    d: &Daemon,
    paths: Vec<String>,
    seed_relay: Option<String>,
    fds: (Option<String>, Option<String>),
) -> Response {
    if paths.is_empty() {
        return Response::Error("provide at least one file or folder to serve".into());
    }
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let (payload, name, archive, temp) = match resolve_payload_for(&paths, &fds) {
        Ok(v) => v,
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    let source = adopt_source(fds.0, fds.1, temp.is_none(), payload).await;
    // Subscribe before serving so the temp archive is cleaned up once the serving
    // transfer ends (it stays alive for the whole session — chunks are on the fly).
    let watch = temp.clone().map(|t| (d.manager.subscribe(), t));
    match d
        .manager
        .serve_ticket(source, name, archive, seed_relay)
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

/// Host a short pairing code in the daemon: the rendezvous *and* the ticket
/// behind it, so the terminal that asked for the code can go away.
async fn serve_code(
    d: &Daemon,
    paths: Vec<String>,
    relay: Option<String>,
    keep: bool,
    fds: (Option<String>, Option<String>),
) -> Response {
    if paths.is_empty() {
        return Response::Error("provide at least one file or folder to serve".into());
    }
    let Some(relay) = relay.or_else(crate::book::default_relay_or_builtin) else {
        return Response::Error(
            "a pairing code needs a rendezvous relay: pass --relay <host>, set ARVOLO_RELAY, \
             or configure `relay` in config.toml"
                .into(),
        );
    };
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let (payload, name, archive, temp) = match resolve_payload_for(&paths, &fds) {
        Ok(v) => v,
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    let source = adopt_source(fds.0, fds.1, temp.is_none(), payload).await;
    // Same cleanup contract as `serve_ticket`: a packed archive has to stay
    // readable for the whole session, so it goes when the transfer ends.
    let watch = temp.clone().map(|t| (d.manager.subscribe(), t));
    // A code is always self-contained (`code@relay`) — on every path now, not just
    // this one: the receiver shouldn't have to be configured, and which it got used
    // to depend on whether a daemon happened to be running.
    let max_sessions = (!keep).then_some(1);
    match d
        .manager
        .serve_code(source, name, archive, relay, max_sessions)
        .await
    {
        Ok((id, code)) => {
            if let Some((rx, t)) = watch {
                spawn_temp_cleanup(rx, id, t);
            }
            Response::CodeServed { id, code }
        }
        Err(e) => {
            if let Some(t) = temp {
                let _ = std::fs::remove_file(t);
            }
            Response::Error(format!("{e:#}"))
        }
    }
}

/// Withdraw a deposit from the relay, then forget the local record. An already
/// expired one has nothing left on the relay — tidy the record and report success
/// rather than an error the user can do nothing about.
///
/// A deposit this engine made is withdrawn *through* the engine. Revoking its blob
/// directly would be half a withdrawal: the send also left an offer in the
/// recipient's inbox, and only the engine's `cancel` retracts that (and ends the
/// still-live `Deposited` row). Deleting the file while leaving the offer standing
/// would point the recipient at something that is no longer there.
async fn revoke_deposit(d: &Daemon, id: &str) -> Response {
    let Some(rec) = crate::deposits::load(id) else {
        return Response::Error("no such deposit".into());
    };

    if let Some(tid) = rec.transfer_id.filter(|t| d.manager.get(*t).is_some()) {
        // Detached and best-effort inside the engine (a dead relay must not hang the
        // row), so the record goes now: the deposit is on its way out either way, and
        // a receipt for a withdrawal already ordered would only be a lie in the list.
        d.manager.cancel(tid);
        return match crate::deposits::remove(id) {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        };
    }

    // No engine row: a one-shot CLI deposit, or one whose daemon has since restarted
    // without restoring it. The record carries what the withdrawal needs — including
    // the inbox offer, which must come down with the blob.
    match crate::deposits::withdraw(&rec).await {
        Ok(()) => Response::Ok,
        Err(e) => Response::Error(format!("{e:#}")),
    }
}

/// The saved address book, projected to serializable [`ContactDto`]s for the
/// GUI's "Persone" grid.
///
/// The two advertised names go through [`sanitize_display`] on the way out, as
/// they already do for `contacts list`. They are the sender's own text, and the
/// GUI now prints them next to the name the user chose: a bidi override in there
/// would reorder the line and could make the brackets read as part of the
/// user's own petname. React escapes HTML; it does nothing about U+202E.
fn list_contacts() -> Vec<ContactDto> {
    use crate::text::sanitize_display;
    crate::book::contact_list()
        .into_iter()
        .map(|(name, id)| ContactDto {
            fingerprint: crate::book::fingerprint_of(&id).unwrap_or_default(),
            verified: crate::book::is_verified(&id),
            trusted: crate::book::is_trusted(&id),
            blocked: crate::book::is_blocked(&id),
            display_name: crate::book::display_name_of(&id)
                .map(|n| sanitize_display(&n))
                .unwrap_or_default(),
            pending_name: crate::book::pending_name_of(&id)
                .map(|n| sanitize_display(&n))
                .unwrap_or_default(),
            relay: crate::book::relay_of(&id).unwrap_or_default(),
            last_exchange: crate::book::last_exchange(&id),
            name,
            id,
        })
        .collect()
}

/// Trust an identity to auto-download — same refusal as `arvolo contacts trust`:
/// an unverified key needs `force`, because auto-downloading from a key never
/// confirmed out-of-band is a MITM risk.
fn mark_trusted(who: String, force: bool) -> Response {
    let id = match crate::book::resolve_recipient(&who) {
        Ok(r) => crate::identity::encode_id(&r),
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    if !crate::book::is_verified(&id) && !force {
        let fp = crate::book::fingerprint_of(&id).unwrap_or_default();
        return Response::Error(format!(
            "'{who}' isn't verified — trusting it would auto-download from a key you \
             haven't confirmed out-of-band (MITM risk). Fingerprint: {fp}. \
             Verify first, or force."
        ));
    }
    match crate::book::mark_trusted(&who) {
        Ok(_) => Response::Ok,
        Err(e) => Response::Error(format!("{e:#}")),
    }
}

/// Receive from a pasted artefact, exactly as `arvolo recv` sorts them: a short
/// pairing code is resolved to its ticket over the rendezvous first, an `arvm…`
/// offline ticket is fetched from the relay mailbox, and anything else must be an
/// `arvc…` chunked ticket fetched live. The download runs in the engine, so it
/// shows up as a normal transfer row with progress, pause and cancel.
async fn recv_ticket(
    d: &Daemon,
    ticket: String,
    out: Option<String>,
    password: Option<String>,
) -> Response {
    use arvolo_core::{chunked::ChunkTicket, code, offline::OfflineTicket};

    let ticket = ticket.trim().to_string();

    // Resolve a code to its ticket, bounded: resolution waits for the sender, and
    // an RPC that can hang forever on a typo would freeze the UI that sent it.
    let ticket = if code::looks_like_code(&ticket) {
        let default_relay = crate::book::default_relay_or_builtin();
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            code::resolve_code(&ticket, default_relay.as_deref()),
        )
        .await
        {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => return Response::Error(format!("pairing: {e:#}")),
            Err(_) => {
                return Response::Error(
                    "pairing timed out — the sender isn't answering on that code (check it, \
                     or try again while they're online)"
                        .into(),
                )
            }
        }
    } else {
        ticket
    };

    // What lands where: honour an explicit file path; treat an explicit directory
    // as "in there, under the payload's own name"; default to the download dir.
    // `recv_chunked` resolves dir→file itself, but the offline fetch does not, so
    // the file name is settled here for both.
    let (name, size, peer) = if let Ok(t) = ChunkTicket::decode(&ticket) {
        (t.name, t.total_size, None)
    } else if let Ok(t) = OfflineTicket::decode(&ticket) {
        if t.has_password() && password.as_deref().map(str::is_empty).unwrap_or(true) {
            return Response::Error(
                "this ticket is password-protected — supply the password".into(),
            );
        }
        // The payload name travels sealed, so the row starts under a claim-derived
        // one; the sender id is right in the ticket though, so the row knows who.
        let name = arvolo_core::flow::default_out(&ticket)
            .display()
            .to_string();
        let peer = arvolo_core::crypto::PublicId::from_bytes(&t.sender).ok();
        (name, t.total_size, peer)
    } else {
        return Response::Error(
            "not a ticket this daemon understands — paste an arvc… ticket, a pairing \
             code (like 4821-crater-mango), or an arvm… mailbox ticket"
                .into(),
        );
    };
    let safe_name = arvolo_core::flow::safe_download_name(&name).unwrap_or_else(|| name.clone());
    let out_path = match out.map(PathBuf::from) {
        Some(p) if p.is_dir() => p.join(&safe_name),
        Some(p) => p,
        None => d.download_dir.join(&safe_name),
    };
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).ok();
        }
    }

    let id = d
        .manager
        .start_download_with_password(ticket, out_path, peer, name, size, password);
    Response::TransferId(id)
}

/// Encrypt `path` (a single file, or a folder packed into an archive) locally and
/// deposit a public, browser-openable download link on the relay. The key rides
/// in the URL fragment; the relay only ever sees ciphertext. Mirrors the CLI's
/// `--link` send, including saving a revocable deposit record.
async fn create_link(
    d: &Daemon,
    path: String,
    ttl: Option<u64>,
    max: Option<u32>,
    fds: (Option<String>, Option<String>),
) -> Response {
    let relay = match d.relay.as_deref() {
        Some(r) => r.to_string(),
        None => return Response::Error("no relay configured — links need a relay".into()),
    };
    let ttl = ttl.unwrap_or(7 * 24 * 3600);
    let max = max.unwrap_or(crate::deposits::UNLIMITED);

    let (payload, _name, _archive, temp) = match resolve_payload_for(&[PathBuf::from(path)], &fds) {
        Ok(v) => v,
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    // The upload happens inside this handler; the source (and any descriptor
    // inside it) lives exactly as long as this call.
    let source = adopt_source(fds.0, fds.1, temp.is_none(), payload).await;

    let outcome = arvolo_core::link::deposit_link(source, &relay, ttl, max).await;
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    match outcome {
        Ok(out) => {
            // Save a revocable record so a GUI-created link can still be cancelled
            // with `arvolo cancel <id>`, exactly like a link made from the CLI.
            let _ = crate::deposits::save(
                crate::deposits::KIND_LINK,
                &relay,
                &out.claim,
                &out.revoke_token,
                &out.name,
                out.size,
                max,
                Some(out.link.clone()),
                // No `arvm…` ticket on the link path: the URL is the capability.
                "",
                None,
                crate::now_unix().saturating_add(ttl),
                // A link is deposited directly, not through the engine: no transfer
                // row, and no recipient, so no inbox offer. The blob is the whole job.
                None,
                None,
                // Nobody's inbox, so no offer and no second relay.
                None,
            );
            Response::Link(out.link)
        }
        Err(e) => Response::Error(format!("{e:#}")),
    }
}

/// Remove a packed-archive temp once its transfer reaches a terminal state.
/// [`crate::payload::resolve_payload`], minus one assumption: a request that carries a
/// descriptor hand-off for its single path must not depend on the daemon being
/// able to even `stat` that path (macOS folder consent, systemd `ProtectHome=`).
/// The client only offers descriptors for a single regular file, so the name
/// comes straight from the path string and the payload from the descriptor.
fn resolve_payload_for(
    paths: &[PathBuf],
    fds: &(Option<String>, Option<String>),
) -> anyhow::Result<(PathBuf, String, bool, Option<PathBuf>)> {
    if fds.0.is_some() && fds.1.is_some() && paths.len() == 1 {
        let name = paths[0]
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        return Ok((paths[0].clone(), name, false, None));
    }
    crate::payload::resolve_payload(paths)
}

/// Adopt the descriptor hand-off riding on a request, when there is one: a
/// single plain-file source becomes a [`SendSource::Handle`] — the CLI opened
/// the file in the user's own context (macOS settles folder consent at that
/// open, with a prompt the daemon could never show), and the descriptor
/// crossed over via [`arvolo_ipc::fdpass`]. Anything else — archives,
/// directories, requests without the fields, an old client — stays a path.
/// The engine reads the handle positionally and never re-opens anything, and
/// the descriptor lives exactly as long as the send that uses it (it rides
/// inside the source).
async fn adopt_source(
    fd_socket: Option<String>,
    fd_token: Option<String>,
    single_plain_file: bool,
    payload: PathBuf,
) -> arvolo_core::source::SendSource {
    // The phone's hand-off: same two fields, same one-shot collection, but the
    // engine is inside the app that opened the file, so there is no socket to
    // cross. See `fdpass::IN_PROCESS` — and note this arm is not unix-gated,
    // because what it answers is a `content://` URI, which has no path to fall
    // back to on any platform.
    if fd_socket.as_deref() == Some(arvolo_ipc::fdpass::IN_PROCESS) {
        let Some(token) = &fd_token else {
            return payload.into();
        };
        if !single_plain_file {
            return payload.into();
        }
        return match arvolo_ipc::fdpass::take_stashed(token) {
            Ok(mut files) if !files.is_empty() => {
                arvolo_core::source::SendSource::handle(files.remove(0), payload.to_string_lossy())
            }
            // Nothing to degrade to: the payload is a display name here, not a
            // path anything could open. Say so where it will be read.
            other => {
                if let Err(e) = other {
                    tracing::error!("in-process descriptor hand-off failed: {e:#}");
                }
                payload.into()
            }
        };
    }
    #[cfg(unix)]
    {
        let (Some(sock), Some(token)) = (&fd_socket, &fd_token) else {
            return payload.into();
        };
        if !single_plain_file {
            return payload.into();
        }
        let (sock, token) = (sock.clone(), token.clone());
        let fetched = tokio::task::spawn_blocking(move || {
            arvolo_ipc::fdpass::take_files(std::path::Path::new(&sock), &token, 1)
        })
        .await;
        match fetched {
            Ok(Ok(mut files)) => {
                // The label is the original path: names, errors and the
                // persisted retry record all speak it, never a raw fd number.
                arvolo_core::source::SendSource::handle(files.remove(0), payload.to_string_lossy())
            }
            Ok(Err(e)) => {
                // The path may still be readable here (same-user setups mostly
                // are) — degrade rather than fail, but say so.
                tracing::warn!("descriptor hand-off failed ({e:#}); trying the path");
                payload.into()
            }
            Err(e) => {
                tracing::warn!("descriptor hand-off task died ({e}); trying the path");
                payload.into()
            }
        }
    }
    #[cfg(windows)]
    {
        // No per-app folder consent on Windows: same-user processes read the
        // same files, so the fields are never set and paths just work.
        let _ = (fd_socket, fd_token, single_plain_file);
        payload.into()
    }
}

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
                        | ManagerEvent::Deposited { id: e, .. } if *e == id);
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

/// Fill in everything about an offer's sender that lives in the address book and
/// not in the offer.
///
/// `auto` marks one the daemon is about to auto-accept, so a front-end can say
/// "arriving" instead of asking a question with only one answer; it is read
/// through the same predicate the auto-accept decision uses, so the two cannot
/// disagree — spelling the rule out twice is how our own devices came to be
/// auto-downloaded and asked about at the same time. The other
/// three are what the user needs in order to *answer* the question — the sender's
/// fingerprint, how many times they have been received from before, and the name
/// this key is already on record as claiming.
///
/// Stamped here rather than where the offer is parked, because the book moves:
/// saving the sender, approving their name or taking a transfer all change the
/// answers while the offer sits waiting.
pub fn stamp_offer(mut ev: EventDto) -> EventDto {
    if let EventDto::OfferReceived {
        from,
        auto,
        fingerprint,
        seen_count,
        known_name,
        ..
    } = &mut ev
    {
        *auto = crate::identity::auto_accepts(from);
        *fingerprint = crate::book::fingerprint_of(from).unwrap_or_default();
        *seen_count = crate::book::seen_count(from);
        *known_name = crate::book::known_name_of(from)
            .map(|n| crate::text::sanitize_display(&n))
            .unwrap_or_default();
    }
    ev
}

/// Save a contact, refusing a silent re-key.
///
/// `contact_add` re-points a name that already exists at the new key and drops
/// that name's verified and trusted marks. Reached from an offer dialog — where
/// the name being typed is a stranger's — that turns a save into the quiet
/// demotion of whoever already held it: exactly the person the marks were there
/// to protect. The CLI's save prompt refuses the same collision, for the same
/// reason; this is that refusal, moved somewhere every front-end inherits it.
///
/// `rebind` is the deliberate version, and it answers with the two fingerprints
/// so the caller can say what it changed rather than reporting a bare success.
fn add_contact(name: &str, id: &str, rebind: bool) -> Response {
    let taken = crate::book::contact_list()
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, other)| other);
    // Re-adding somebody under the name they already have is not a re-key: it is
    // how a relay hint gets refreshed, and it must stay a plain success. Compare
    // the same normalisation `contact_add` stores, `<id>@<relay>` form included.
    let (bare, _) = crate::book::split_id_and_relay(id);
    let collides = taken.is_some_and(|other| other != bare.trim().to_lowercase());
    if collides && !rebind {
        return Response::Error(format!(
            "'{name}' is already someone else in your contacts — pick another name. \
             Re-pointing it at this key would drop their verified and trusted marks."
        ));
    }
    match crate::book::contact_add(name, id) {
        Ok(Some(change)) => Response::KeyChanged {
            old_fingerprint: change.old_fingerprint,
            new_fingerprint: change.new_fingerprint,
        },
        Ok(None) => Response::Ok,
        Err(e) => Response::Error(format!("{e:#}")),
    }
}

/// The same three facts, onto a parked offer on its way out of `ListPending`.
fn stamp_offer_dto(mut o: OfferDto) -> OfferDto {
    o.fingerprint = crate::book::fingerprint_of(&o.from).unwrap_or_default();
    o.seen_count = crate::book::seen_count(&o.from);
    o.known_name = crate::book::known_name_of(&o.from)
        .map(|n| crate::text::sanitize_display(&n))
        .unwrap_or_default();
    o
}

fn status(d: &Daemon) -> StatusDto {
    let pid = d.manager.public_id();
    let id_b32 = crate::identity::encode_id(&pid);
    StatusDto {
        version: env!("CARGO_PKG_VERSION").to_string(),
        // Assembled here, by the same function `arvolo me` prints, so a window and
        // a terminal hand out the same string — and neither has to know that the
        // form drops `https://`.
        shareable_id: crate::book::shareable_id(&id_b32, d.relay.as_deref()),
        public_id: id_b32,
        fingerprint: pid.fingerprint(),
        relay: d.relay.clone(),
        transfers: d.manager.list().len(),
        pending: d.pending.lock().unwrap().len(),
        download_dir: d.download_dir.display().to_string(),
        display_name: crate::book::my_display_name(),
        pid: std::process::id(),
        // Best-effort: an exe path is not guaranteed on every platform, and not
        // knowing it is no reason to fail a status request.
        exe: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arvolo_core::crypto::Identity;

    fn fresh_id() -> String {
        data_encoding::BASE32_NOPAD
            .encode(&Identity::generate().public().to_bytes())
            .to_lowercase()
    }

    /// The collision guard, where every front-end inherits it.
    ///
    /// `contact_add` re-points an existing name at the new key and drops that
    /// name's verified and trusted marks. Reached from an offer dialog, where the
    /// name being typed is a stranger's, that is the quiet demotion of whoever
    /// already held it — so the plain save refuses, and only a caller that says
    /// `rebind` gets through.
    #[test]
    fn saving_a_stranger_cannot_demote_the_person_whose_name_it_is() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let giulia = fresh_id();
        let stranger = fresh_id();
        crate::book::contact_add("giulia", &giulia).unwrap();
        crate::book::mark_verified("giulia").unwrap();
        crate::book::mark_trusted("giulia").unwrap();

        match add_contact("giulia", &stranger, false) {
            Response::Error(e) => assert!(
                e.contains("verified") && e.contains("trusted"),
                "the refusal has to name what it is protecting, got: {e}"
            ),
            other => panic!("a silent re-key got through: {other:?}"),
        }
        assert!(crate::book::is_verified(&giulia), "her mark is untouched");
        assert!(crate::book::is_trusted(&giulia));
        assert_eq!(
            crate::book::resolve_name(&giulia).as_deref(),
            Some("giulia")
        );

        // Re-adding the same person under the name they already have is not a
        // re-key — it is how a relay hint is refreshed — and must still work.
        assert!(matches!(
            add_contact("giulia", &giulia, false),
            Response::Ok
        ));
        assert!(crate::book::is_verified(&giulia));

        // The deliberate version goes through, and says what it changed.
        match add_contact("giulia", &stranger, true) {
            Response::KeyChanged {
                old_fingerprint,
                new_fingerprint,
            } => {
                assert_ne!(old_fingerprint, new_fingerprint);
                assert!(!old_fingerprint.is_empty());
            }
            other => panic!("expected the two fingerprints back, got {other:?}"),
        }
        assert!(
            !crate::book::is_verified(&stranger) && !crate::book::is_trusted(&stranger),
            "the new key starts unchecked"
        );

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// What the front-end reads off a parked offer is stamped at read time, not
    /// when the offer was parked — the address book moves while an offer waits.
    #[test]
    fn a_parked_offer_is_stamped_from_the_book_as_it_stands() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let from = fresh_id();
        crate::book::observe_advertised_name(&from, "Giulia");
        let parked = OfferDto {
            id: "o1".into(),
            from: from.clone(),
            name: "arrivo.zip".into(),
            size: 9,
            note: String::new(),
            sender_name: "Marco".into(),
            ..Default::default()
        };

        let o = stamp_offer_dto(parked.clone());
        assert!(
            !o.fingerprint.is_empty(),
            "a stranger's words exist before they are saved — that is the point"
        );
        assert_eq!(o.seen_count, 0, "nothing taken from this key yet");
        assert_eq!(o.known_name, "Giulia");

        // A receipt, and the same offer now reads differently.
        crate::book::record_seen(&from);
        assert_eq!(stamp_offer_dto(parked).seen_count, 1);

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// `auto` is the front-end's copy of a decision the daemon has already taken,
    /// so it must be stamped from the same predicate the daemon decides with —
    /// including the half that says an offer from ourselves needs nobody's
    /// permission. Spelled out separately here once, it said "ask" for a file the
    /// daemon was already downloading, and the window's Accept button then pointed
    /// at an offer that had been consumed.
    #[test]
    fn an_offer_the_daemon_will_take_by_itself_is_stamped_as_arriving() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        std::env::remove_var("ARVOLO_IDENTITY");

        let auto_of = |from: &str| match stamp_offer(EventDto::OfferReceived {
            id: "o1".into(),
            from: from.into(),
            name: "arrivo.zip".into(),
            size: 9,
            note: String::new(),
            sender_name: String::new(),
            auto: false,
            fingerprint: String::new(),
            seen_count: 0,
            known_name: String::new(),
        }) {
            EventDto::OfferReceived { auto, .. } => auto,
            other => panic!("stamping changed the event: {other:?}"),
        };

        let stranger = fresh_id();
        assert!(!auto_of(&stranger), "a stranger is a question");

        let me = crate::identity::encode_id(&crate::identity::my_identity().unwrap().public());
        assert!(
            auto_of(&me),
            "our own device: the daemon takes it, so the window must not ask"
        );

        crate::book::contact_add("giulia", &stranger).unwrap();
        crate::book::mark_verified("giulia").unwrap();
        crate::book::mark_trusted("giulia").unwrap();
        assert!(auto_of(&stranger), "trusted: taken without asking");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
