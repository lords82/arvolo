//! Daemon lifecycle: the GUI is a *client* of the same background daemon the CLI
//! drives. On launch we make sure one is running — connecting if it already is,
//! otherwise spawning `arvolo daemon` in its own process group so it **outlives**
//! the GUI window (transfers keep running, and the CLI shares the same engine).

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use arvolo_ipc::client::DaemonClient;

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

/// Spawn `arvolo daemon` detached into its own process group so it survives the
/// GUI closing. Best-effort: errors are surfaced to the caller to retry/report.
fn spawn_daemon() -> Result<()> {
    let mut cmd = std::process::Command::new(arvolo_bin());
    cmd.arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
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
    anyhow::bail!("the daemon did not come up in time")
}
