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
