use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::manager::{ManagerEvent, TransferManager};
use tokio_util::sync::CancellationToken;

use crate::{book, sync};

#[cfg(unix)]
use crate::ipc;
#[cfg(unix)]
use crate::notify;

use crate::ui::*;
use crate::util::*;

use crate::commands::receive::record_history;

/// Run the persistent background engine behind a local control socket.
#[cfg(unix)]
pub(crate) async fn daemon(
    download_dir: Option<PathBuf>,
    relay: Option<String>,
    use_http: bool,
    no_sync: bool,
) -> Result<()> {
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    let relay = require_relay(relay, use_http)?;
    let me = my_identity()?;
    let my_id = encode_id(&me.public());
    let download_dir = download_dir
        .or_else(book::default_download_dir)
        .unwrap_or_else(book::default_home_downloads);
    std::fs::create_dir_all(&download_dir).context("create download dir")?;

    // Persist resumable downloads here so a daemon restart picks them back up.
    let state_dir = book::config_dir().join("transfers");
    let manager = TransferManager::with_state_dir(
        me,
        Some(relay.clone()),
        download_dir.clone(),
        Some(state_dir),
    );
    manager.set_display_name(book::my_display_name());
    let inbox = manager.spawn_inbox()?;
    let _auto_sync =
        (!no_sync && book::sync_enabled()).then(|| sync::spawn_auto_sync(relay.clone()));

    // Single-instance guard: if the socket answers, a daemon is already up.
    let sock = ipc::socket_path();
    if let Some(parent) = sock.parent() {
        std::fs::create_dir_all(parent).ok();
        // Owner-only parent dir: closes the bind()→chmod(0o600) race on the socket
        // itself — another local user can't traverse into the dir to connect during
        // the window when the freshly-bound socket may still carry umask perms.
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).ok();
    }
    if tokio::net::UnixStream::connect(&sock).await.is_ok() {
        anyhow::bail!(
            "a daemon is already running (socket {} answers)",
            sock.display()
        );
    }
    // Stale socket from a previous crash — bind() would fail on an existing path.
    if sock.exists() {
        std::fs::remove_file(&sock).ok();
    }
    let listener = tokio::net::UnixListener::bind(&sock)
        .with_context(|| format!("bind control socket {}", sock.display()))?;
    // Owner-only: the filesystem permission is the access control.
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).ok();

    // Advisory pidfile for service tooling (not the guard).
    let pidfile = book::config_dir().join("daemon.pid");
    std::fs::write(&pidfile, format!("{}\n", std::process::id())).ok();

    // Offers awaiting the user's approval. In M1 every offer parks here (no trust
    // policy yet); a subscribed front-end lists and accepts/rejects them.
    let pending: Arc<Mutex<HashMap<String, ipc::protocol::OfferDto>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Engine task: park incoming offers and persist finished transfers to history,
    // whether or not a front-end is attached.
    {
        let mut events = manager.subscribe();
        let manager = manager.clone();
        let pending = pending.clone();
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
                    }) => {
                        let from_b32 = encode_id(&from);
                        // Record any advertised-name change durably (surfaced later in
                        // `contacts list` / to an attached front-end); never blocks.
                        book::observe_advertised_name(&from_b32, &sender_name);
                        // Supersede any older parked offer for the same (sender, file).
                        // A live→mailbox fallback posts a fresh offer and retracts the
                        // stale live one, but we may already have pulled that stale copy
                        // into the inbox; accepting it would download nothing. Drop the
                        // older one(s) and keep only this newest offer.
                        let superseded: Vec<String> = {
                            let map = pending.lock().unwrap();
                            map.values()
                                .filter(|o| o.from == from_b32 && o.name == name)
                                .map(|o| o.id.clone())
                                .collect()
                        };
                        for old in &superseded {
                            pending.lock().unwrap().remove(old);
                            manager.reject_offer(old).await;
                        }
                        let status = book::sender_status(&from_b32);
                        let who = status.name.clone().unwrap_or_else(|| from_b32.clone());
                        // Trusted sender → auto-download. Everyone else parks and
                        // waits for the user's approval (the default).
                        if status.trusted {
                            let size_h = human_size(size);
                            eprintln!(
                                "⬇ auto-downloading {} ({size_h}) from trusted {who}",
                                sanitize_display(&name)
                            );
                            // Auto-accept, but still surface a notification so the
                            // user knows a trusted download is happening.
                            notify::auto_downloading(&name, &who, &size_h);
                            if let Err(e) = manager.accept_offer(&id, None).await {
                                eprintln!("   ✗ could not auto-accept: {e:#}");
                            }
                        } else {
                            let size_h = human_size(size);
                            eprintln!(
                                "📨 offer parked: {name} ({size_h}) from {who} — approve with `arvolo accept {id}`"
                            );
                            // Nudge the user with a desktop notification (best-effort;
                            // no-op on headless hosts, where the log line above stands in).
                            notify::offer_awaiting(&name, &who, &size_h);
                            pending.lock().unwrap().insert(
                                id.clone(),
                                ipc::protocol::OfferDto {
                                    id,
                                    from: from_b32,
                                    name,
                                    size,
                                    note,
                                    sender_name,
                                },
                            );
                        }
                    }
                    Ok(ManagerEvent::Completed { id, path }) => {
                        if let Some(p) = &path {
                            eprintln!("✓ saved {}", p.display());
                        }
                        record_history(&manager, id, "completed");
                    }
                    Ok(ManagerEvent::Deposited { id }) => record_history(&manager, id, "deposited"),
                    Ok(ManagerEvent::Failed { id, error }) => {
                        record_history(&manager, id, &format!("failed: {error}"))
                    }
                    Ok(ManagerEvent::Cancelled { id }) => record_history(&manager, id, "cancelled"),
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    eprintln!("arvolo daemon up.");
    eprintln!("  identity: {my_id}");
    eprintln!("  relay:    {relay}");
    eprintln!("  socket:   {}", sock.display());
    eprintln!("  saving:   {}", download_dir.display());

    // Resume any downloads that were in flight when the daemon last stopped — each
    // continues from its partial file on disk, no re-accept needed.
    let resumed = manager.resume_incomplete();
    if resumed > 0 {
        eprintln!("  resuming: {resumed} unfinished download(s)");
    }

    let shutdown = daemon_shutdown_signal();
    let daemon = ipc::server::Daemon {
        manager,
        relay: Some(relay),
        pending,
    };
    let result = ipc::server::run(daemon, listener, shutdown).await;

    inbox.cancel();
    std::fs::remove_file(&sock).ok();
    std::fs::remove_file(&pidfile).ok();
    result
}

/// A cancellation token that fires on SIGINT (Ctrl-C) or SIGTERM (systemd stop).
#[cfg(unix)]
pub(crate) fn daemon_shutdown_signal() -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match term.as_mut() {
                    Some(s) => { s.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
        t.cancel();
    });
    token
}

/// Connect to a running daemon and confirm it answers, else `None` (so callers
/// fall back to running the engine in-process).
///
/// A daemon is long-lived: after upgrading the binary the old process keeps
/// running the old code, and a mismatched daemon speaks a different IPC dialect
/// — it may not answer newer requests at all, hanging the client. So we gate on
/// version here: same version → use it; different (or a pre-versioning daemon
/// that reports none) → refuse loudly and exit, telling the user to restart it.
#[cfg(unix)]
pub(crate) async fn daemon_client() -> Option<ipc::client::DaemonClient> {
    let mut c = ipc::client::DaemonClient::connect().await.ok()?;
    c.ping().await.ok()?;
    // `status` predates the newer requests, so an old daemon still answers it
    // (with an empty version) — safe to probe without risking a hang.
    if let Ok(st) = c.status().await {
        let ours = env!("CARGO_PKG_VERSION");
        if st.version != ours {
            let theirs = if st.version.is_empty() {
                "unknown (older, pre-versioning)".to_string()
            } else {
                st.version.clone()
            };
            eprintln!(
                "✗ version mismatch: this CLI is {ours}, but the running daemon is {theirs}.\n  \
                 The daemon kept running the old binary after the upgrade. Restart it:\n    \
                 kill $(cat ~/.config/arvolo/daemon.pid)   # stop the stale daemon\n    \
                 arvolo daemon                             # start it on {ours}"
            );
            std::process::exit(1);
        }
    }
    Some(c)
}

/// A fresh subscribed event stream from the daemon.
#[cfg(unix)]
pub(crate) async fn daemon_events() -> Result<ipc::client::EventStream> {
    ipc::client::DaemonClient::connect()
        .await?
        .subscribe()
        .await
}

/// Print a one-line summary of a transfer DTO.
#[cfg(unix)]
pub(crate) fn print_transfer_dto(t: &ipc::protocol::TransferDto, rate: Option<u64>) {
    let is_send = t.direction == "send";
    let arrow = if is_send { "→" } else { "←" };
    let peer = t
        .peer
        .as_deref()
        .map(|id| book::resolve_name(id).unwrap_or_else(|| id.to_string()))
        .unwrap_or_else(|| "anonymous".into());
    let progress = if t.total_size > 0 {
        format!(
            " {}/{}",
            human_size(t.transferred),
            human_size(t.total_size)
        )
    } else {
        String::new()
    };
    // Live throughput (only in --watch): the arrow already signals the direction.
    let speed = match rate {
        Some(bps) if bps > 0 => format!(" @ {}/s", human_size(bps)),
        _ => String::new(),
    };
    // Who's on the other side of the swarm: for a send, how many are pulling from
    // us right now; for a receive, how many peers we're pulling from.
    let peers = if is_send {
        match t.download_peers {
            0 => String::new(),
            1 => "  1 downloading".to_string(),
            n => format!("  {n} downloading"),
        }
    } else if t.swarm_peers > 0 {
        format!(
            "  from {} peer{} ({} via swarm)",
            t.swarm_peers,
            if t.swarm_peers == 1 { "" } else { "s" },
            t.pieces_from_peers
        )
    } else {
        String::new()
    };
    println!(
        "  [{}] {arrow} {peer}  {}{progress}{speed}  ({}){peers}",
        t.id, t.name, t.status
    );
}

/// `arvolo accept <offer_id>` — approve a parked offer and download it.
#[cfg(unix)]
pub(crate) async fn accept_cmd(offer_id: String, out: Option<PathBuf>) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon`)")?;
    let id = client.accept(offer_id, out).await?;
    eprintln!("✓ accepted — downloading (transfer {id}). Track it with `arvolo transfers`.");
    Ok(())
}

/// `arvolo reject <offer_id>` — decline a parked offer.
#[cfg(unix)]
pub(crate) async fn reject_cmd(offer_id: String) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon`)")?;
    client.reject(offer_id).await?;
    eprintln!("✗ rejected.");
    Ok(())
}

/// `arvolo cancel <id>` — stop a transfer running in the daemon.
#[cfg(unix)]
pub(crate) async fn cancel_cmd(id: u64) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon`)")?;
    client.cancel(id).await?;
    eprintln!("cancelled transfer {id}.");
    Ok(())
}

/// `arvolo pause <id>` — hold a `send --to` running in the daemon.
#[cfg(unix)]
pub(crate) async fn pause_cmd(id: u64) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon`)")?;
    client.pause(id).await?;
    eprintln!("paused transfer {id} — `arvolo resume {id}` to continue, or `arvolo cancel {id}`.");
    Ok(())
}

/// `arvolo resume <id>` — continue a paused `send --to`.
#[cfg(unix)]
pub(crate) async fn resume_cmd(id: u64) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon`)")?;
    client.resume(id).await?;
    eprintln!("resumed transfer {id}.");
    Ok(())
}

/// Hand a plain ticket send to the daemon: it serves in the background. Prints the
/// `arvc…` ticket and returns immediately; the transfer is tracked in the daemon.
#[cfg(unix)]
pub(crate) async fn serve_ticket_via_daemon(
    mut client: ipc::client::DaemonClient,
    paths: Vec<PathBuf>,
    seed_relay: Option<String>,
    qr: bool,
) -> Result<()> {
    // The daemon resolves paths on its own cwd — absolutize relative to ours.
    let paths_s: Vec<String> = paths
        .iter()
        .map(|p| {
            std::fs::canonicalize(p)
                .with_context(|| format!("{}", p.display()))
                .map(|abs| abs.to_string_lossy().into_owned())
        })
        .collect::<Result<Vec<_>>>()
        .context("no such file or folder to serve")?;
    let (id, ticket) = client
        .serve_ticket(paths_s, seed_relay)
        .await
        .context("daemon rejected the serve")?;
    println!("\nServing via the daemon. On the other device:\n");
    println!("    arvolo recv {ticket}\n");
    if qr {
        print_qr(&ticket);
    }
    println!(
        "Tracked as transfer {id} — follow it with `arvolo transfers`, stop it with `arvolo cancel {id}`."
    );
    Ok(())
}

/// Hand a push off to the running daemon and return immediately — the daemon
/// delivers it in the background, concurrent and surviving our exit. Mirrors
/// [`serve_ticket_via_daemon`]; observe progress with `arvolo transfers`.
#[cfg(unix)]
pub(crate) async fn push_via_daemon(
    mut client: ipc::client::DaemonClient,
    paths: Vec<PathBuf>,
    to: String,
    note: String,
) -> Result<()> {
    // The daemon resolves paths on *its own* cwd (e.g. `/` under systemd), not
    // ours — so absolutize here, relative to the client's cwd, and validate the
    // files exist now with a clear error instead of a confusing daemon-side one.
    let paths_s: Vec<String> = paths
        .iter()
        .map(|p| {
            std::fs::canonicalize(p)
                .with_context(|| format!("{}", p.display()))
                .map(|abs| abs.to_string_lossy().into_owned())
        })
        .collect::<Result<Vec<_>>>()
        .context("no such file or folder to push")?;
    eprintln!("Handing off to the daemon (sending to {to})…");
    let id = client
        .push(to, paths_s, note)
        .await
        .context("daemon rejected the push")?;
    println!(
        "queued as transfer {id} — the daemon delivers it in the background.\n\
         Track it with `arvolo transfers`, stop it with `arvolo cancel {id}`."
    );
    Ok(())
}
