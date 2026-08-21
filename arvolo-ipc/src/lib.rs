//! Local daemon IPC — the wire contract and client half, shared by the CLI and
//! the desktop GUI so both drive one long-lived
//! [`arvolo_core::manager::TransferManager`] behind a single background daemon.
//!
//! * [`protocol`] — the wire types (requests, responses, event/DTO structs).
//! * [`client`] — [`client::DaemonClient`], a thin typed dialer (unix only).
//!
//! The daemon **server** (the accept-loop that owns the `TransferManager`) lives
//! in the CLI crate: it needs engine + policy (trust, notifications, history)
//! that don't belong on the wire. This crate is just the contract plus the client.
//!
//! **Where the channel lives, and what guards it, differs by platform.** On unix
//! it is a socket file under the config dir, and the filesystem is the access
//! control: owner-only permissions on both the socket and its parent directory,
//! so no token scheme is needed. On Windows there is no such file — the channel is
//! a **named pipe**, which lives in a machine-wide namespace rather than in a
//! directory. Everything above the connect call is identical: the same
//! newline-delimited JSON, the same requests, the same replies.
//!
//! That difference is why the pipe's name is derived from the config directory
//! (see [`pipe_name`]): two users, or two isolated test instances, must not land
//! on the same pipe just because the namespace is flat.

pub mod protocol;

pub mod client;

#[cfg(unix)]
pub mod fdpass;

use std::path::PathBuf;

/// The config directory holding the socket and ledgers. Mirrors the CLI's
/// `book::config_dir()` (kept in lockstep on purpose): honors `ARVOLO_CONFIG_DIR`,
/// else `~/.config/arvolo`. Duplicated here — rather than depending on the CLI —
/// so the GUI can resolve the socket without pulling in the whole binary crate.
fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("ARVOLO_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    home_dir().join(".config/arvolo")
}

fn home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    if let Ok(p) = std::env::var("USERPROFILE") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        if !drive.is_empty() && !path.is_empty() {
            return PathBuf::from(format!("{drive}{path}"));
        }
    }
    PathBuf::from(".")
}

/// Path to the daemon control socket (honors `ARVOLO_CONFIG_DIR`).
///
/// Unix only in practice: on Windows the channel is a named pipe and has no path
/// on disk. Kept available on both so callers that merely *report* where the
/// daemon lives — an error message, a status line — have something to print.
pub fn socket_path() -> PathBuf {
    config_dir().join("daemon.sock")
}

/// The Windows named pipe the daemon listens on, derived from the config dir.
///
/// Pipe names live in one flat, machine-wide namespace, so a fixed name would put
/// two users of the same machine — or two tests that isolate themselves with
/// `ARVOLO_CONFIG_DIR`, which is how this suite keeps instances apart — on the
/// same channel. Deriving it from the directory reproduces the separation the
/// unix side gets for free by being a file inside that directory.
///
/// The tail of the path is kept readable (it is what a person sees in a process
/// explorer) and a short FNV-1a digest of the whole path is appended so that two
/// different directories ending in the same name cannot collide. FNV rather than
/// the standard hasher because this value must be identical in two separate
/// processes, possibly built at different times: `DefaultHasher` promises no such
/// stability, and a mismatch would be a client unable to find its own daemon.
pub fn pipe_name() -> String {
    let dir = config_dir();
    let full = dir.to_string_lossy();

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in full.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    let tail: String = full
        .chars()
        .rev()
        .take(24)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    format!(r"\\.\pipe\arvolo-{tail}-{hash:016x}")
}

/// Path to the daemon's pid file (honors `ARVOLO_CONFIG_DIR`) — what a frontend
/// reads to stop a stale daemon so its supervisor can start a fresh one.
pub fn pid_path() -> PathBuf {
    config_dir().join("daemon.pid")
}

/// Path to the `daemon stop` marker (honors `ARVOLO_CONFIG_DIR`).
///
/// `arvolo daemon stop` writes it before the daemon goes down; `daemon run`
/// removes it on startup. Every supervisor loop that would respawn a dead daemon
/// — the GUI's event pump above all — must check it first: a stop the user asked
/// for has to stay stopped, not come back within seconds.
pub fn stop_marker_path() -> PathBuf {
    config_dir().join("daemon.stopped")
}

/// Path to the daemon's log (honors `ARVOLO_CONFIG_DIR`).
///
/// Here rather than in whoever launches the daemon, because the two must agree:
/// a frontend that spawns the daemon points its output at this file, and anyone
/// diagnosing a problem later has one place to look. A daemon launched from a
/// terminal writes to that terminal instead, which is what a person watching it
/// wants.
pub fn log_path() -> PathBuf {
    config_dir().join("daemon.log")
}

/// Keep the daemon log bounded: past `max_bytes`, the current file becomes
/// `daemon.log.1` (replacing any previous one) and logging continues in a fresh
/// file. Called by whoever is about to *open* the log, since rotating a file
/// somebody already holds open would leave them writing to the renamed one.
///
/// One generation, not many: the point is that the log is readable and cannot
/// grow without bound, not that a month of it is archived. Best-effort — a log
/// that can't be rotated is not a reason to fail to start a daemon.
pub fn rotate_log(max_bytes: u64) {
    let path = log_path();
    let too_big = std::fs::metadata(&path).map(|m| m.len() > max_bytes);
    if matches!(too_big, Ok(true)) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
}

/// One lock for every test in this crate that points `ARVOLO_CONFIG_DIR` at a
/// tempdir of its own.
///
/// The variable is process-global and *everything* here derives its paths from it
/// — the socket, the fd-passing socket, the log. Two tests setting it at once do
/// not get a directory each: the last writer wins and the other quietly works
/// inside a tempdir that is about to be deleted, which is why this suite failed on
/// a different test each run (`fdpass` one time, log rotation the next) while
/// every test passed on its own. A per-module lock cannot fix that — the race is
/// between modules — so the lock has to live here, where all of them can take it.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod log_rotation_tests {
    use super::*;

    #[test]
    fn a_big_log_is_rotated_to_one_generation() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let log = log_path();
        std::fs::write(&log, vec![b'x'; 100]).unwrap();
        rotate_log(10);
        assert!(!log.exists(), "the oversized log makes way for a fresh one");
        assert_eq!(
            std::fs::read(dir.path().join("daemon.log.1"))
                .unwrap()
                .len(),
            100,
            "and is kept, once, as the previous generation"
        );

        // A second rotation replaces that generation rather than accumulating.
        std::fs::write(&log, vec![b'y'; 50]).unwrap();
        rotate_log(10);
        assert_eq!(
            std::fs::read(dir.path().join("daemon.log.1")).unwrap(),
            vec![b'y'; 50]
        );
        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// A log under the cap is left alone: rotating a small one would throw away
    /// the only history there is.
    #[test]
    fn a_small_log_is_left_alone() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let log = log_path();
        std::fs::write(&log, b"short").unwrap();
        rotate_log(1024);
        assert_eq!(std::fs::read(&log).unwrap(), b"short");
        assert!(!dir.path().join("daemon.log.1").exists());
        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// No log yet is the normal first run, not a failure.
    #[test]
    fn a_missing_log_is_not_an_error() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        rotate_log(10);
        assert!(!log_path().exists());
        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
