use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use crate::book;

use crate::ipc;
use crate::output::vprintln;

use crate::ui::*;
use crate::util::*;

/// Run the persistent background engine behind a local control socket.
/// The control channel the daemon listens on: a bound socket on unix, the first
/// pipe instance on Windows. The rest of the daemon does not care which.
#[cfg(unix)]
type Control = tokio::net::UnixListener;
#[cfg(windows)]
type Control = tokio::net::windows::named_pipe::NamedPipeServer;

/// Open the control channel, and with it the guarantee that only one daemon runs.
///
/// The guarantee is the interesting half, and each platform already owns a
/// mechanism for it. On unix it is `flock(2)`: kernel-enforced, dies with the
/// process so it is never stale, and — unlike probing the socket — cannot be
/// fooled by a daemon that is alive but too busy to answer. Without it a second
/// daemon would take the socket path from a live-but-slow first one and two
/// engines would run on one identity: double deposits, forked history.
///
/// On Windows the pipe *is* the lock. Creating the first instance of a name fails
/// outright if another process already holds it, which is the same guarantee from
/// the same kind of source — the kernel, not a file someone might leave behind.
/// So there is no lock file to return there, and nothing to clean up if the
/// process is killed.
#[cfg(unix)]
fn open_control() -> Result<(Control, Option<std::fs::File>)> {
    use std::os::unix::fs::PermissionsExt;

    let sock = ipc::socket_path();
    if let Some(parent) = sock.parent() {
        std::fs::create_dir_all(parent).ok();
        // Owner-only parent dir: closes the bind()→chmod(0o600) race on the socket
        // itself — another local user can't traverse into the dir to connect during
        // the window when the freshly-bound socket may still carry umask perms.
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).ok();
    }
    let lock = acquire_instance_lock(&book::config_dir().join("daemon.lock"))?;
    // With the lock held, an existing socket file can only be a leftover from a
    // crashed daemon — safe to clear before binding.
    if sock.exists() {
        std::fs::remove_file(&sock).ok();
    }
    let listener = tokio::net::UnixListener::bind(&sock)
        .with_context(|| format!("bind control socket {}", sock.display()))?;
    // Owner-only: the filesystem permission is the access control.
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).ok();
    Ok((listener, Some(lock)))
}

#[cfg(windows)]
fn open_control() -> Result<(Control, Option<std::fs::File>)> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let name = ipc::pipe_name();
    let pipe = ServerOptions::new()
        // Both halves of the single-instance guard and of the access control.
        //
        // `first_pipe_instance` makes this call fail when another process already
        // owns the name — that is the flock equivalent, and the reason no lock
        // file exists on this platform.
        //
        // `reject_remote_clients` is set explicitly even though it is the default:
        // a named pipe is reachable over SMB unless it says otherwise, and this
        // channel accepts files, reads the address book and approves offers. A
        // security-relevant default is one to state, not to inherit quietly.
        //
        // What guards it locally is the pipe's default security descriptor, which
        // grants the creating user, SYSTEM and Administrators — and no other
        // ordinary user. That is the same practical boundary the unix side draws
        // with 0600: an administrator of the machine can already read the identity
        // key off disk, so admitting them here concedes nothing that was not
        // already conceded.
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(&name)
        .with_context(|| format!("cannot listen on {name} (another daemon is probably running)"))?;
    Ok((pipe, None))
}

/// Where the control channel can be found, for the startup banner and for error
/// messages. A path on unix, a pipe name on Windows — different kinds of address
/// for the same thing, and a person debugging needs the one their system uses.
#[cfg(unix)]
fn control_address() -> String {
    ipc::socket_path().display().to_string()
}

#[cfg(windows)]
fn control_address() -> String {
    ipc::pipe_name()
}

/// Undo whatever `open_control` left on disk. Nothing, on Windows: a pipe exists
/// only while its process does.
#[cfg(unix)]
fn close_control() {
    std::fs::remove_file(ipc::socket_path()).ok();
}

#[cfg(windows)]
fn close_control() {}

/// Where `arvolo daemon stop` leaves its marker. Anything that auto-respawns the
/// daemon (the GUI's event pump above all) checks it: a stop the user asked for
/// must stay stopped, not be resurrected within seconds by a supervisor loop.
/// Shared with the GUI through `arvolo_ipc`, so both sides mean the same file.
pub(crate) fn stop_marker_path() -> PathBuf {
    arvolo_ipc::stop_marker_path()
}

/// `arvolo daemon run` — the service itself, in this terminal (blocking).
pub(crate) async fn daemon(
    download_dir: Option<PathBuf>,
    relay: Option<String>,
    no_sync: bool,
) -> Result<()> {
    use std::sync::Arc;

    // The control channel FIRST, before any engine work: opening it is also what
    // claims the right to be the only daemon, and a losing twin must not get as
    // far as subscribing to the relay inbox.
    let (listener, _instance_lock) = open_control()?;

    // Starting on purpose clears a leftover `daemon stop` marker, so the GUI's
    // supervisor loop resumes its job.
    std::fs::remove_file(stop_marker_path()).ok();

    let relay = require_relay(relay)?;

    // Made here rather than by the engine: this crate is what serves front-ends,
    // so it is what counts them — and what reads the count, to decide whether a
    // notification of its own would be the second one for the same file.
    let front_ends: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let shutdown = daemon_shutdown_signal();

    let running = arvolo_engine::start::start(arvolo_engine::start::Options {
        download_dir,
        relay: relay.clone(),
        auto_sync: !no_sync,
        announce: Arc::new(crate::announce::Terminal {
            front_ends: front_ends.clone(),
        }),
        shutdown: shutdown.clone(),
        // The desktop writes downloads where they belong the first time; only a
        // phone has to move them afterwards.
        publisher: None,
        // The board picks up where it left off, here too.
        //
        // This daemon outlives the window it serves (see the GUI's `daemon.rs`),
        // so most of the time there is nothing to put back — the list was never
        // lost. What this answers is the case where it *was*: a reboot, an
        // update, a `restart`. Before it, the board came back empty and the
        // morning's transfers were only in the history screen, which reads as
        // having lost them.
        //
        // Runs once, at start, against an empty list, so it cannot duplicate a
        // live row; `resume_incomplete` above has already claimed anything still
        // unfinished, and the log only holds what ended.
        restore_board: 300,
        front_ends,
    })
    .await?;

    // Advisory pidfile for service tooling (not the guard).
    let pidfile = book::config_dir().join("daemon.pid");
    std::fs::write(&pidfile, format!("{}\n", std::process::id())).ok();

    eprintln!("arvolo daemon up.");
    eprintln!("  identity: {}", running.my_id);
    eprintln!("  relay:    {relay}");
    eprintln!("  channel:  {}", control_address());
    eprintln!("  saving:   {}", running.download_dir.display());
    if running.resumed > 0 {
        eprintln!("  resuming: {} unfinished download(s)", running.resumed);
    }

    let result = ipc::server::run(running.daemon.clone(), listener, shutdown).await;
    running.stop();
    close_control();
    std::fs::remove_file(&pidfile).ok();
    result
}

/// `arvolo daemon start` — spawn `daemon run` detached and return once it
/// answers. Its output goes to `daemon.log` next to the config, where
/// `daemon stop`'s advice and the GUI's diagnostics already look.
pub(crate) async fn daemon_start(
    download_dir: Option<PathBuf>,
    relay: Option<String>,
    no_sync: bool,
) -> Result<()> {
    if let Some(mut client) = daemon_client().await {
        let s = client.status().await?;
        anyhow::bail!(
            "a daemon is already running (v{}, {} transfer(s)) — `arvolo daemon status` for details",
            s.version,
            s.transfers
        );
    }
    let exe = std::env::current_exe().context("locate the arvolo binary")?;
    let log_path = book::config_dir().join("daemon.log");
    std::fs::create_dir_all(book::config_dir()).ok();
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open {}", log_path.display()))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon").arg("run");
    if let Some(d) = &download_dir {
        cmd.arg("--download-dir").arg(d);
    }
    if let Some(r) = &relay {
        cmd.arg("--relay").arg(r);
    }
    if no_sync {
        cmd.arg("--no-sync");
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(log.try_clone().context("clone log handle")?)
        .stderr(log);
    // Its own process group / detached, so closing this terminal doesn't take
    // the daemon with it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(DETACHED_PROCESS);
    }
    let child = cmd.spawn().context("spawn the daemon")?;
    let pid = child.id();

    // Wait for it to answer, briefly — "started" should mean reachable.
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Some(mut client) = daemon_client().await {
            let s = client.status().await?;
            eprintln!(
                "✓ daemon running (pid {pid}, v{}) — log: {}",
                s.version,
                log_path.display()
            );
            return Ok(());
        }
    }
    anyhow::bail!(
        "the daemon did not come up within 10s — its last words are in {}",
        log_path.display()
    )
}

/// `arvolo daemon stop` — ask it to exit over IPC; fall back to the pidfile for
/// a daemon too old to know the request. Leaves the stop marker either way, so
/// nothing (the GUI above all) respawns what the user just stopped.
pub(crate) async fn daemon_stop_cmd() -> Result<()> {
    // The marker goes down BEFORE the daemon does: a supervisor that notices the
    // death first would otherwise win the race and respawn it.
    std::fs::write(stop_marker_path(), b"stopped by `arvolo daemon stop`\n").ok();

    // No version gate here: stopping a daemon of ANOTHER version is exactly
    // this command's job — the mismatch error recommends `daemon stop`, so
    // `daemon stop` must not refuse on mismatch. An old daemon that doesn't
    // know `Shutdown` errors the request and the pidfile below finishes it.
    if let Some(mut client) = daemon_client_any_version().await {
        match client.shutdown().await {
            Ok(()) => {
                eprintln!("✓ daemon stopped. Start it again with `arvolo daemon start`.");
                return Ok(());
            }
            Err(e) => vprintln!("IPC shutdown not accepted ({e:#}) — trying the pidfile"),
        }
    }

    let pidfile = book::config_dir().join("daemon.pid");
    let pid = std::fs::read_to_string(&pidfile)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok());
    match pid {
        None => {
            std::fs::remove_file(stop_marker_path()).ok();
            eprintln!("no daemon is running.");
            Ok(())
        }
        #[cfg(unix)]
        Some(pid) => {
            // SAFETY: plain kill(2) with SIGTERM; an ESRCH just means it's gone.
            let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
            if rc == 0 {
                eprintln!(
                    "✓ daemon stopped (pid {pid}). Start it again with `arvolo daemon start`."
                );
            } else {
                std::fs::remove_file(&pidfile).ok();
                eprintln!("no daemon is running (stale pidfile removed).");
            }
            Ok(())
        }
        #[cfg(windows)]
        Some(pid) => {
            let ok = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                eprintln!(
                    "✓ daemon stopped (pid {pid}). Start it again with `arvolo daemon start`."
                );
            } else {
                std::fs::remove_file(&pidfile).ok();
                eprintln!("no daemon is running (stale pidfile removed).");
            }
            Ok(())
        }
    }
}

/// `arvolo daemon status` — is it running, and what is it doing.
pub(crate) async fn daemon_status_cmd(json: bool) -> Result<()> {
    match daemon_client().await {
        None => {
            if json {
                println!("{{\"running\": false}}");
            } else {
                eprintln!("not running — start it with `arvolo daemon start`.");
            }
            Ok(())
        }
        Some(mut client) => {
            let s = client.status().await?;
            if json {
                let mut v = serde_json::to_value(&s)?;
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("running".into(), serde_json::Value::Bool(true));
                }
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(());
            }
            println!("version:       {}", s.version);
            // Which process, before what it is doing: with a released install, a
            // checkout build and one the desktop app spawned all possible on one
            // machine, "there is a daemon" is only half an answer.
            if s.pid > 0 {
                println!("pid:           {}", s.pid);
            }
            if !s.exe.is_empty() {
                println!("running:       {}", s.exe);
            }
            println!("id:            {}", s.public_id);
            println!("fingerprint:   {}", s.fingerprint);
            println!("relay:         {}", s.relay.as_deref().unwrap_or("(none)"));
            println!("downloads to:  {}", s.download_dir);
            if !s.display_name.is_empty() {
                println!("display name:  {}", s.display_name);
            }
            println!("transfers:     {}", s.transfers);
            println!("pending:       {} offer(s) awaiting approval", s.pending);
            Ok(())
        }
    }
}

/// Is semver `a` strictly older than `b`? Compares the numeric major/minor/patch
/// only (both sides are our own `CARGO_PKG_VERSION`s). An unparsable or empty `b`
/// — an ancient daemon that predates versioning — reads as older, not newer.
fn version_lt(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> (u64, u64, u64) {
        let mut it = v
            .split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0));
        (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
        )
    }
    parts(a) < parts(b)
}

/// Take the daemon's exclusive instance lock: an `flock(2)` on `path`, held for
/// the returned handle's lifetime. The kernel releases it when the process dies
/// (any exit path — crash, SIGKILL), so it can never go stale, and a second
/// daemon fails **immediately** instead of probing a socket that a busy-but-live
/// daemon might be slow to answer. The lock file itself is never deleted; only
/// the lock matters.
#[cfg(unix)]
pub(crate) fn acquire_instance_lock(path: &std::path::Path) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("open instance lock {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
    // Non-blocking exclusive lock: EWOULDBLOCK == another daemon holds it.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        anyhow::bail!(
            "a daemon is already running (instance lock {} is held)",
            path.display()
        );
    }
    Ok(file)
}

#[cfg(all(test, unix))]
mod lock_tests {
    use super::{acquire_instance_lock, version_lt};

    #[test]
    fn version_lt_orders_semver_and_tolerates_junk() {
        assert!(version_lt("0.9.0", "0.9.2"), "patch");
        assert!(
            version_lt("0.9.9", "0.10.0"),
            "minor is numeric, not textual"
        );
        assert!(!version_lt("0.9.2", "0.9.2"), "equal is not older");
        assert!(!version_lt("1.0.0", "0.9.2"), "newer is not older");
        // A pre-versioning daemon reports "" — must read as older, so we never
        // tell the user to update a CLI that is in fact the newer one.
        assert!(!version_lt("0.9.2", ""), "empty peer is not newer");
    }

    #[test]
    fn instance_lock_is_exclusive_and_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.lock");

        // flock is per open-file-description, so a second open in the same
        // process contends exactly like a second process would.
        let first = acquire_instance_lock(&path).expect("first lock");
        let second = acquire_instance_lock(&path);
        assert!(second.is_err(), "second acquisition must fail while held");
        assert!(
            second.unwrap_err().to_string().contains("already running"),
            "failure names the running daemon"
        );

        // Dropping the holder releases the lock (as process death would).
        drop(first);
        acquire_instance_lock(&path).expect("re-acquire after release");
    }
}

/// A cancellation token that fires on SIGINT (Ctrl-C) or SIGTERM (systemd stop).
pub(crate) fn daemon_shutdown_signal() -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    #[cfg(windows)]
    tokio::spawn(async move {
        // Ctrl-C is all there is: Windows has no SIGTERM, and a service stop
        // arrives through the service control manager, which this daemon does not
        // register with (it runs as a plain process, started by the GUI or by
        // hand). Closing the console is the other way out, and that kills us
        // outright rather than politely.
        let _ = tokio::signal::ctrl_c().await;
        t.cancel();
    });
    #[cfg(unix)]
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
pub(crate) async fn daemon_client() -> Option<ipc::client::DaemonClient> {
    let mut c = daemon_client_any_version().await?;
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
            // Which side is stale decides the advice. Restarting the daemon only
            // helps when *it* is the old one; if this CLI is older (a leftover on
            // PATH while a newer daemon runs — e.g. a GUI started one from a fresh
            // build), restarting the daemon with it would only downgrade things.
            let cli_is_older = version_lt(ours, &st.version);
            eprintln!(
                "✗ version mismatch: this CLI is {ours}, but the running daemon is {theirs}."
            );
            if cli_is_older {
                eprintln!(
                    "  This CLI binary is the stale one. Update it:\n    \
                     cargo install --path cli    # or reinstall the {theirs} release"
                );
            } else {
                eprintln!(
                    "  The daemon kept running the old binary after the upgrade. Restart it:\n    \
                     arvolo daemon stop && arvolo daemon start   # restart on {ours}"
                );
            }
            std::process::exit(1);
        }
    }
    Some(c)
}

/// Connect + ping, with no version gate. Only for operations that must work
/// across versions (today: `daemon stop`).
async fn daemon_client_any_version() -> Option<ipc::client::DaemonClient> {
    let mut c = ipc::client::DaemonClient::connect().await.ok()?;
    c.ping().await.ok()?;
    Some(c)
}

/// A fresh subscribed event stream from the daemon.
pub(crate) async fn daemon_events() -> Result<ipc::client::EventStream> {
    ipc::client::DaemonClient::connect()
        .await?
        .subscribe()
        .await
}

/// Print a one-line summary of a transfer DTO.
/// What a row is doing, said in its own terms.
///
/// The engine calls a background serve `active` because it is running, which is
/// true and useless: nothing is in flight, the file is simply available. Left as
/// "active" a served ticket sits at 100% for ever — indistinguishable from a
/// transfer that stalled — and the seeding a finished download turns into shows up
/// as a 0% outgoing send of a file nobody sent. Both are the same thing, and the
/// honest word for it is not "active".
///
/// While someone really is pulling, it *is* a transfer, so it says so.
pub(crate) fn transfer_state(t: &ipc::protocol::TransferDto) -> String {
    if t.sharing && t.status == "active" {
        return if t.download_peers > 0 {
            "sharing — being downloaded now".into()
        } else {
            "sharing — available, nobody downloading".into()
        };
    }
    t.status.clone()
}

/// What a row shows and the user types: the transfer's 8-hex handle — falling
/// back to the numeric id only against a daemon too old to send one.
pub(crate) fn dto_handle(t: &ipc::protocol::TransferDto) -> String {
    if t.handle.is_empty() {
        t.id.to_string()
    } else {
        t.handle.clone()
    }
}

/// Resolve what the user typed — an 8-hex handle (any unique prefix) or a plain
/// number — to the daemon's internal transfer id.
pub(crate) async fn resolve_transfer_id(
    client: &mut ipc::client::DaemonClient,
    input: &str,
) -> Result<u64> {
    use crate::handles::{looks_like_handle, resolve_prefix, Match};
    if looks_like_handle(input) {
        let list = client.list().await?;
        match resolve_prefix(input, list.iter().map(|t| (t.handle.clone(), t.id))) {
            Match::One(id) => return Ok(id),
            Match::Many(hs) => anyhow::bail!(
                "'{input}' matches more than one transfer ({}) — type more of it",
                hs.join(", ")
            ),
            // An all-digit input can still be a plain transfer number.
            Match::None => {
                if input.parse::<u64>().is_err() {
                    // Well-formed handle, no such transfer: say that, not
                    // "invalid digit" from trying to read hex as a number.
                    anyhow::bail!(
                        "no transfer with handle '{input}' — it may have ended, or \
                         belonged to a previous daemon. `arvolo status` lists the \
                         live ones."
                    );
                }
            }
        }
    }
    input
        .parse::<u64>()
        .context("not a transfer id — `arvolo status` lists them")
}

/// The handle of a transfer the daemon just created, looked back up from its
/// numeric id — so freshly-printed hints use the same currency as `status`.
pub(crate) async fn handle_for(client: &mut ipc::client::DaemonClient, id: u64) -> String {
    match client.list().await {
        Ok(list) => list
            .iter()
            .find(|t| t.id == id)
            .map(dto_handle)
            .unwrap_or_else(|| id.to_string()),
        Err(_) => id.to_string(),
    }
}

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
        dto_handle(t),
        t.name,
        transfer_state(t)
    );
    // A code hosted in the daemon has no terminal left to have printed it — this
    // list is where you come back to read it out again.
    if let Some(code) = &t.code {
        println!("        code: arvolo recv {code}");
    }
    // A deposited send says "waiting to be picked up" for as long as a week. What
    // the recipient's end has actually done with it belongs right here, in the
    // words the deposits section already uses.
    if let Some(line) = crate::commands::status::offer_line(t.offer_status.as_deref()) {
        println!("        {line}");
    }
}

/// `arvolo pause <id>` — hold a transfer running in the daemon.
pub(crate) async fn pause_cmd(id: String) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon start`)")?;
    let tid = resolve_transfer_id(&mut client, &id).await?;
    let shown = handle_for(&mut client, tid).await;
    client.pause(tid).await?;
    eprintln!(
        "paused transfer {shown} — `arvolo resume {shown}` to continue, or `arvolo cancel {shown}`."
    );
    Ok(())
}

/// `arvolo deposit <id>` — park a waiting send on the relay instead of waiting
/// any longer for the recipient to accept.
pub(crate) async fn deposit_cmd(id: String) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon start`)")?;
    let tid = resolve_transfer_id(&mut client, &id).await?;
    let shown = handle_for(&mut client, tid).await;
    client.deposit_now(tid).await?;
    eprintln!(
        "transfer {shown} is going to the mailbox — they collect it whenever they come back. \
         Take it back with `arvolo cancel {shown}`."
    );
    Ok(())
}

/// `arvolo resume <id>` — continue a paused transfer.
pub(crate) async fn resume_cmd(id: &str) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon start`)")?;
    let tid = resolve_transfer_id(&mut client, id).await?;
    let shown = handle_for(&mut client, tid).await;
    client.resume(tid).await?;
    eprintln!("resumed transfer {shown}.");
    Ok(())
}

/// The guard for a pending descriptor hand-off; `()` where none can exist.
#[cfg(unix)]
pub(crate) type FdOfferGuard = arvolo_ipc::fdpass::FdOffer;
#[cfg(windows)]
pub(crate) type FdOfferGuard = ();

/// Stand up the descriptor hand-off for `paths` when it applies — unix, exactly
/// one plain file this process can open. The CLI runs in the user's own context
/// (macOS settles folder consent here, prompting if it must), so its open
/// succeeds where the daemon's would not; the daemon then reads through the
/// descriptor instead of the path. Keep the returned guard alive until the
/// daemon has replied — its thread is what serves the fd. Every `None` simply
/// means plain path behavior, as before.
pub(crate) fn offer_sources(
    paths: &[PathBuf],
) -> (Option<ipc::client::FdHandoff>, Option<FdOfferGuard>) {
    #[cfg(unix)]
    {
        if paths.len() != 1 || !paths[0].is_file() {
            return (None, None);
        }
        let Ok(f) = std::fs::File::open(&paths[0]) else {
            // `guard_readable` in `send` already turned a real permission
            // problem into its own error; anything left is a race — let the
            // daemon try the path and report in its own words.
            return (None, None);
        };
        match arvolo_ipc::fdpass::offer_files(vec![f]) {
            Ok(o) => {
                let handoff = ipc::client::FdHandoff {
                    socket: o.socket.display().to_string(),
                    token: o.token.clone(),
                };
                (Some(handoff), Some(o))
            }
            Err(_) => (None, None),
        }
    }
    #[cfg(windows)]
    {
        // Windows has no per-app folder consent: same-user processes read the
        // same files, so the daemon's own open is exactly as good.
        let _ = paths;
        (None, None)
    }
}

/// Hand a plain ticket send to the daemon: it serves in the background and this
/// returns immediately with the transfer's handle and the `arvc…` ticket — the
/// caller decides how the ticket is delivered (raw, or written as a `.arvolo`
/// file).
pub(crate) async fn serve_ticket_via_daemon(
    mut client: ipc::client::DaemonClient,
    paths: Vec<PathBuf>,
    seed_relay: Option<String>,
) -> Result<(String, String)> {
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
    let (handoff, _offer) = offer_sources(&paths);
    let (id, ticket) = client
        .serve_ticket(paths_s, seed_relay, handoff)
        .await
        .context("daemon rejected the serve")?;
    Ok((handle_for(&mut client, id).await, ticket))
}

/// Hand a pairing code to the daemon: it hosts the rendezvous *and* serves the
/// ticket behind it. Prints the code and returns; the code outlives this terminal
/// and a daemon restart alike.
pub(crate) async fn serve_code_via_daemon(
    mut client: ipc::client::DaemonClient,
    paths: Vec<PathBuf>,
    relay: Option<String>,
    keep: bool,
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
    let (handoff, _offer) = offer_sources(&paths);
    let (id, code) = client
        .serve_code(paths_s, relay, keep, handoff)
        .await
        .context("daemon refused to host the code")?;
    let shown = handle_for(&mut client, id).await;
    // The artefact alone on stdout; the words around it go to stderr.
    println!("{code}");
    eprintln!("\nOn the other device:\n");
    eprintln!("    arvolo recv {code}\n");
    if qr {
        print_qr(&code);
    }
    if keep {
        eprintln!(
            "Serving via the daemon, for anyone with the code. Tracked as transfer {shown} — \
             follow it with `arvolo status`, stop it with `arvolo cancel {shown}`."
        );
    } else {
        eprintln!(
            "Serving via the daemon. The code works once; the transfer keeps going after that. \
             Tracked as {shown} — follow it with `arvolo status`, stop it with `arvolo cancel {shown}`."
        );
    }
    Ok(())
}

/// Hand a push off to the running daemon and return immediately — the daemon
/// delivers it in the background, concurrent and surviving our exit. Mirrors
/// [`serve_ticket_via_daemon`]; observe progress with `arvolo status`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn push_via_daemon(
    mut client: ipc::client::DaemonClient,
    paths: Vec<PathBuf>,
    to: String,
    note: String,
    ttl: Option<u64>,
    max: Option<u32>,
    password: Option<String>,
    device: Option<String>,
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
    let (handoff, _offer) = offer_sources(&paths);
    // `--to me` has already become our own base32 id by the time it gets here
    // (the in-process fallback resolves against the address book, which does not
    // know the reserved word), and a 52-character id is not a recipient anybody
    // recognises as themselves. Same rule as everywhere else: never print the
    // wire value.
    let shown_to = if crate::is_self(&to) {
        "your devices".to_string()
    } else {
        to.clone()
    };
    eprintln!("Handing off to the daemon (sending to {shown_to})…");
    let id = client
        .push(
            to, paths_s, note, ttl, max, password, device, false, handoff,
        )
        .await
        .context("daemon rejected the push")?;
    let shown = handle_for(&mut client, id).await;
    println!(
        "queued as transfer {shown} — the daemon delivers it in the background.\n\
         Track it with `arvolo status`, stop it with `arvolo cancel {shown}`."
    );
    Ok(())
}

#[cfg(all(test, unix))]
mod share_line_tests {
    use super::transfer_state;
    use arvolo_ipc::protocol::TransferDto;

    fn dto(sharing: bool, download_peers: usize, status: &str) -> TransferDto {
        TransferDto {
            id: 1,
            handle: "8cd63bda".into(),
            direction: "send".into(),
            peer: None,
            name: "delega.pdf".into(),
            total_size: 100,
            transferred: 100,
            status: status.into(),
            swarm_peers: 0,
            pieces_from_peers: 0,
            download_peers,
            created: 0,
            code: None,
            sharing,
            copies_served: 0,
            bytes_served: 0,
            last_pickup: 0,
            from_download: 0,
            path: None,
            saved_uri: None,
            sender_name: String::new(),
            offer_status: None,
            waiting_code: None,
            waiting_detail: None,
            can_pause: None,
            fail_code: None,
            fail_detail: None,
            link: None,
            ticket: None,
        }
    }

    /// The bug in one assertion: a served ticket that has finished serving is not
    /// a transfer stuck at 100%, and must not be worded as one.
    #[test]
    fn a_finished_share_is_not_reported_as_a_running_transfer() {
        let line = transfer_state(&dto(true, 0, "active"));
        assert!(line.contains("sharing"), "{line}");
        assert!(!line.contains("active"), "{line}");
    }

    /// While bytes really are moving it *is* a transfer, and says so.
    #[test]
    fn a_share_being_pulled_says_so() {
        let line = transfer_state(&dto(true, 2, "active"));
        assert!(line.contains("being downloaded now"), "{line}");
    }

    /// Everything else is left exactly as the engine worded it: a cancelled share
    /// is cancelled, and a send with a recipient was never a share to begin with.
    #[test]
    fn nothing_else_is_reworded() {
        assert_eq!(transfer_state(&dto(true, 0, "cancelled")), "cancelled");
        assert_eq!(transfer_state(&dto(false, 0, "active")), "active");
        let failed = "failed: relay unreachable";
        assert_eq!(transfer_state(&dto(true, 0, failed)), failed);
    }
}
