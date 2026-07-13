//! Local daemon IPC. The wire contract ([`protocol`]) and the dialer ([`client`])
//! now live in the shared `arvolo-ipc` crate so the desktop GUI can drive the
//! same daemon; they are re-exported here for the CLI's existing call sites.
//!
//! * [`server`] — the daemon accept-loop and request dispatch (stays here: it
//!   owns the [`arvolo_core::manager::TransferManager`] and the CLI's policy).
//!
//! The socket lives at [`socket_path`] under the config dir; filesystem
//! permissions (owner-only) are the access control, so no token scheme is needed.
//! Windows (for the GUI) would swap the listener/dialer for a named pipe behind
//! this same protocol.

pub use arvolo_ipc::{client, protocol};

pub mod server;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

/// Path to the daemon control socket (honors `ARVOLO_CONFIG_DIR`). Delegates to
/// the shared crate so the CLI and GUI always resolve the same path.
pub fn socket_path() -> PathBuf {
    arvolo_ipc::socket_path()
}
