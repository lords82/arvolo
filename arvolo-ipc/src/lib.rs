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
//! The socket lives at [`socket_path`] under the config dir; filesystem
//! permissions (owner-only) are the access control, so no token scheme is needed.
//! Windows (for the GUI) would swap the listener/dialer for a named pipe behind
//! this same protocol — a later phase.

pub mod protocol;

#[cfg(unix)]
pub mod client;

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
pub fn socket_path() -> PathBuf {
    config_dir().join("daemon.sock")
}

/// Path to the daemon's pid file (honors `ARVOLO_CONFIG_DIR`) — what a frontend
/// reads to stop a stale daemon so its supervisor can start a fresh one.
pub fn pid_path() -> PathBuf {
    config_dir().join("daemon.pid")
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

#[cfg(test)]
mod log_rotation_tests {
    use super::*;

    /// These read the process-global `ARVOLO_CONFIG_DIR`.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_big_log_is_rotated_to_one_generation() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
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
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
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
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        rotate_log(10);
        assert!(!log_path().exists());
        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
