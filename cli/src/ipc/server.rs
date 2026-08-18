//! Daemon side of the IPC: a `UnixListener` accept-loop that translates
//! [`Request`]s into [`TransferManager`] calls and fans engine events out to
//! subscribed connections. It holds no business logic beyond that translation —
//! policy (trust, notifications, history) lives in the daemon command itself.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use arvolo_core::manager::{ManagerEvent, TransferManager};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use super::pairing::Sessions;
use super::protocol::{
    ConfigDto, ConfigPatch, ContactDto, EventDto, HistoryDto, OfferDto, PresenceDto, Request,
    RequestEnvelope, Response, ServerMessage, Setting, StatusDto, SyncDto, TransferDto,
};

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
        shutdown: CancellationToken,
    ) -> Self {
        Daemon {
            manager,
            relay,
            download_dir,
            pending,
            // Deep enough that a client which stalls briefly does not miss a
            // pairing code; a lagged receiver is skipped, never blocking the sender.
            side_events: broadcast::channel(64).0,
            pairings: Sessions::default(),
            sync_state: Arc::new(Mutex::new(SyncState::default())),
            front_ends,
            shutdown,
        }
    }
}

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

async fn dispatch(d: &Daemon, cmd: Request) -> Response {
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
            let mut v: Vec<OfferDto> = d.pending.lock().unwrap().values().cloned().collect();
            v.sort_by(|a, b| a.name.cmp(&b.name));
            Response::Pending(v)
        }
        Request::ListContacts => Response::Contacts(list_contacts()),
        Request::ListDeposits => Response::Deposits(crate::deposits::list_dtos().await),
        Request::RevokeDeposit { id } => revoke_deposit(d, &id).await,
        Request::Cancel { id } => {
            d.manager.cancel(id);
            Response::Ok
        }
        // Refuse (rather than silently no-op) when the transfer isn't finished:
        // a UI that dropped the row on a bare Ok would show a list that no longer
        // matches the engine.
        Request::Remove { id } => {
            if d.manager.remove(id) {
                Response::Ok
            } else {
                Response::Error(
                    "this transfer is still in flight — cancel it before removing it".into(),
                )
            }
        }
        Request::ClearFinished => Response::Cleared(d.manager.clear_finished()),
        Request::MarkVerified { name } => match crate::book::mark_verified(&name) {
            Ok(_) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
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
        Request::AcceptOffer {
            offer_id,
            out,
            password,
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
                    d.pending.lock().unwrap().remove(&offer_id);
                    Response::TransferId(id)
                }
                Err(e) => Response::Error(format!("{e:#}")),
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
                },
            )
            .await
        }
        Request::ServeTicket { paths, seed_relay } => serve_ticket(d, paths, seed_relay).await,
        Request::ServeCode { paths, relay, keep } => serve_code(d, paths, relay, keep).await,
        Request::CreateLink { path, ttl, max } => create_link(d, path, ttl, max).await,
        Request::Recv {
            ticket,
            out,
            password,
        } => recv_ticket(d, ticket, out, password).await,
        // Contact edits reply on their own result; the pushed `ContactsChanged`
        // event (from the daemon's book watcher) is what nudges every *other*
        // attached frontend to refetch.
        Request::AddContact { name, id } => match crate::book::contact_add(&name, &id) {
            Ok(_key_change) => Response::Ok,
            Err(e) => Response::Error(format!("{e:#}")),
        },
        Request::RemoveContact { name } => match crate::book::contact_remove(&name) {
            Ok(true) => Response::Ok,
            Ok(false) => Response::Error(format!("no such contact '{name}'")),
            Err(e) => Response::Error(format!("{e:#}")),
        },
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
        Request::PruneNames => match crate::book::prune_orphan_names() {
            Ok(n) => Response::Cleared(n),
            Err(e) => Response::Error(format!("{e:#}")),
        },
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
}

async fn push(
    d: &Daemon,
    to: String,
    paths: Vec<String>,
    note: String,
    opts: MailboxOpts,
) -> Response {
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

    // A forced deposit is a different engine call, not a flag on the same one: it
    // skips the presence check and the held/retry loop entirely (see
    // `TransferManager::deposit_to`). The ticket comes back so a UI can offer the
    // `arvm…` for hand-delivery, exactly as `arvolo send --deposit` prints it.
    let outcome = if opts.deposit {
        let mailbox = arvolo_core::manager::MailboxOpts {
            ttl: opts.ttl,
            max: opts.max,
            password: opts.password.filter(|p| !p.is_empty()),
        };
        d.manager
            .deposit_to(&recipient, payload, name, note, mailbox)
            .await
            .map(|(id, ticket)| (id, Some(ticket)))
    } else {
        d.manager
            .send_to(&recipient, payload, name, archive, note)
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
        seed: file.seed,
        swarm: file.swarm.unwrap_or_default(),
        concurrency: file.concurrency,
        config_path: crate::book::config_path().display().to_string(),
        identity_path: crate::identity_path().display().to_string(),
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

fn sync_dto(d: &Daemon) -> anyhow::Result<SyncDto> {
    let pid = d.manager.public_id();
    let st = d.sync_state.lock().unwrap().clone();
    Ok(SyncDto {
        fingerprint: pid.fingerprint(),
        public_id: crate::encode_id(&pid),
        contacts: crate::book::contact_list().len(),
        enabled: crate::book::sync_enabled(),
        last_sync: st.last_sync,
        last_merged: st.last_merged,
        last_error: st.last_error,
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
                st.last_sync = crate::util::now_unix();
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

/// Host a short pairing code in the daemon: the rendezvous *and* the ticket
/// behind it, so the terminal that asked for the code can go away.
async fn serve_code(d: &Daemon, paths: Vec<String>, relay: Option<String>, keep: bool) -> Response {
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
    let (payload, name, archive, temp) = match crate::resolve_payload(&paths) {
        Ok(v) => v,
        Err(e) => return Response::Error(format!("{e:#}")),
    };
    // Same cleanup contract as `serve_ticket`: a packed archive has to stay
    // readable for the whole session, so it goes when the transfer ends.
    let watch = temp.clone().map(|t| (d.manager.subscribe(), t));
    // The code is always self-contained (`code@relay`): the daemon knows which
    // relay it claimed on, and the receiver shouldn't have to be configured.
    let max_sessions = (!keep).then_some(1);
    match d
        .manager
        .serve_code(payload, name, archive, relay, true, max_sessions)
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
fn list_contacts() -> Vec<ContactDto> {
    crate::book::contact_list()
        .into_iter()
        .map(|(name, id)| ContactDto {
            fingerprint: crate::book::fingerprint_of(&id).unwrap_or_default(),
            verified: crate::book::is_verified(&id),
            trusted: crate::book::is_trusted(&id),
            blocked: crate::book::is_blocked(&id),
            display_name: crate::book::display_name_of(&id).unwrap_or_default(),
            pending_name: crate::book::pending_name_of(&id).unwrap_or_default(),
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
        Ok(r) => crate::encode_id(&r),
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
async fn create_link(d: &Daemon, path: String, ttl: Option<u64>, max: Option<u32>) -> Response {
    let relay = match d.relay.as_deref() {
        Some(r) => r.to_string(),
        None => return Response::Error("no relay configured — links need a relay".into()),
    };
    let ttl = ttl.unwrap_or(7 * 24 * 3600);
    let max = max.unwrap_or(crate::deposits::UNLIMITED);

    let (payload, _name, _archive, temp) = match crate::resolve_payload(&[PathBuf::from(path)]) {
        Ok(v) => v,
        Err(e) => return Response::Error(format!("{e:#}")),
    };

    let outcome = arvolo_core::link::deposit_link(&payload, &relay, ttl, max).await;
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
                crate::util::now_unix().saturating_add(ttl),
                // A link is deposited directly, not through the engine: no transfer
                // row, and no recipient, so no inbox offer. The blob is the whole job.
                None,
                None,
            );
            Response::Link(out.link)
        }
        Err(e) => Response::Error(format!("{e:#}")),
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
                Ok(ev) => stamp_auto(EventDto::from(&ev)),
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

/// Mark an offer that the daemon is about to auto-accept, so a front-end can say
/// "arriving" instead of asking a question with only one answer. Trust is read
/// from the same address book the auto-accept decision uses, so the two agree.
fn stamp_auto(mut ev: EventDto) -> EventDto {
    if let EventDto::OfferReceived { from, auto, .. } = &mut ev {
        *auto = crate::book::sender_status(from).trusted;
    }
    ev
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

fn status(d: &Daemon) -> StatusDto {
    let pid = d.manager.public_id();
    StatusDto {
        version: env!("CARGO_PKG_VERSION").to_string(),
        public_id: crate::encode_id(&pid),
        fingerprint: pid.fingerprint(),
        relay: d.relay.clone(),
        transfers: d.manager.list().len(),
        pending: d.pending.lock().unwrap().len(),
        download_dir: d.download_dir.display().to_string(),
        display_name: crate::book::my_display_name(),
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
