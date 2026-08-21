//! Daemon lifecycle: the GUI is a *client* of the same background daemon the CLI
//! drives. On launch we make sure one is running — connecting if it already is,
//! otherwise spawning `arvolo daemon` in its own process group so it **outlives**
//! the GUI window (transfers keep running, and the CLI shares the same engine).

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use arvolo_ipc::client::DaemonClient;

/// Rotate the daemon log past this size. Big enough to hold the history of a
/// long-running daemon, small enough to open in an editor and to mail to someone.
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Locate the `arvolo` CLI binary that hosts the daemon. In order:
/// 1. `ARVOLO_BIN` (explicit override — handy in `tauri dev`),
/// 2. a binary sitting next to the GUI executable (the bundled/installed layout),
/// 3. `arvolo` on `PATH`.
fn arvolo_bin() -> PathBuf {
    if let Ok(p) = std::env::var("ARVOLO_BIN") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(if cfg!(windows) {
                "arvolo.exe"
            } else {
                "arvolo"
            });
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("arvolo")
}

/// True if a daemon answers on the control socket right now.
pub async fn is_running() -> bool {
    match DaemonClient::connect().await {
        Ok(mut c) => c.ping().await.is_ok(),
        Err(_) => false,
    }
}

/// Spawn `arvolo daemon run` detached into its own process group so it survives
/// the GUI closing. `run` (not `start`): the GUI is the supervisor here — it
/// wants the foreground process as its child, with both output streams to
/// redirect into the rotated log. `start` would self-background and write its
/// own log, breaking both.
fn spawn_daemon() -> Result<()> {
    let mut cmd = std::process::Command::new(arvolo_bin());
    cmd.args(["daemon", "run"])
        .stdin(std::process::Stdio::null());
    // Keep what it says. Spawned from here the daemon has no terminal, and its
    // output used to go to /dev/null — which is the *common* case, so in the
    // situation where someone most needs to know what happened there was nothing
    // to read at all. Rotate first (we are the ones opening it), then hand it the
    // same file for both streams: its notices and its warnings belong together and
    // in order.
    arvolo_ipc::rotate_log(LOG_MAX_BYTES);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(arvolo_ipc::log_path())
    {
        // Both streams onto the same handle, so notices and warnings interleave in
        // the order they happened rather than in two files nobody can line up.
        Ok(log) => match log.try_clone() {
            Ok(err) => {
                cmd.stdout(log).stderr(err);
            }
            Err(_) => {
                cmd.stdout(log).stderr(std::process::Stdio::null());
            }
        },
        // No log is not a reason to refuse to start the daemon.
        Err(e) => {
            eprintln!("(couldn't open the daemon log: {e}) ");
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // New process group → a SIGHUP/close of the GUI doesn't take it down.
        cmd.process_group(0);
    }
    cmd.spawn().context("spawn `arvolo daemon`")?;
    Ok(())
}

/// Ensure a daemon is up: connect if one exists, else spawn it and wait (with a
/// short backoff) for the socket to come alive. Returns an error only if the
/// daemon never answers — the UI shows a "disconnesso" banner in that case.
pub async fn ensure_running() -> Result<()> {
    if is_running().await {
        return Ok(());
    }
    spawn_daemon()?;
    // The daemon binds its socket after loading identity/config + connecting to
    // the relay; give it a few seconds of polling before giving up.
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if is_running().await {
            return Ok(());
        }
    }
    anyhow::bail!("the daemon did not come up in time{}", last_words())
}

/// How much of the daemon's log to quote when it fails to start.
const TAIL_LINES: usize = 6;
const TAIL_BYTES: u64 = 16 * 1024;

/// The last thing the daemon said before failing to come up, as a suffix for the
/// error above — or nothing, if it said nothing.
///
/// Without this the UI can only report that the daemon "did not answer", which is
/// the one thing the user already knows. The daemon has no terminal here, so its
/// explanation goes to the log and nowhere else: an identity file it refuses to
/// load, a relay URL it cannot parse, a port already taken. Those are all things
/// with an obvious fix and no way to discover it.
fn last_words() -> String {
    use std::io::{Read, Seek, SeekFrom};
    let path = arvolo_ipc::log_path();
    let Ok(mut f) = std::fs::File::open(&path) else {
        return String::new();
    };
    // Only the tail: this log is capped but still long, and the interesting part is
    // always at the end.
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if f.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .is_err()
    {
        return String::new();
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        // A non-UTF8 tail (we may have cut a character in half) is not worth a
        // second attempt.
        return String::new();
    }
    let tail: Vec<&str> = buf
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .rev()
        .take(TAIL_LINES)
        .collect();
    if tail.is_empty() {
        return String::new();
    }
    let mut out = String::from(". Ultime righe del daemon:\n");
    for line in tail.into_iter().rev() {
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}
