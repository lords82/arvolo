//! Local daemon IPC: a Unix-domain socket carrying newline-delimited JSON so the
//! CLI (and a future GUI) drive one long-lived [`arvolo_core::manager::TransferManager`].
//!
//! * [`protocol`] — the wire contract (requests, responses, event DTOs).
//! * [`server`] — the daemon accept-loop and request dispatch.
//!
//! The socket lives at [`socket_path`] under the config dir; filesystem
//! permissions (owner-only) are the access control, so no token scheme is needed.
//! Windows (for a future GUI) would swap the listener/dialer for a named pipe
//! behind this same protocol.

pub mod client;
pub mod protocol;
pub mod server;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

/// Path to the daemon control socket (honors `ARVOLO_CONFIG_DIR`).
pub fn socket_path() -> PathBuf {
    crate::book::config_dir().join("daemon.sock")
}
