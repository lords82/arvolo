//! Local daemon IPC. The wire contract ([`protocol`]) and the dialer ([`client`])
//! now live in the shared `arvolo-ipc` crate so the desktop GUI can drive the
//! same daemon; they are re-exported here for the CLI's existing call sites.
//!
//! * [`server`] — the daemon accept-loop and request dispatch (stays here: it
//!   owns the [`arvolo_core::manager::TransferManager`] and the CLI's policy).
//!
//! On unix the channel is a socket at [`socket_path`] under the config dir, and
//! the filesystem is the access control: owner-only permissions, so no token
//! scheme is needed. On Windows it is a named pipe — see `pipe_name` — guarded by
//! its security descriptor instead, and explicitly closed to remote clients.

pub use arvolo_ipc::{client, protocol};

pub mod pairing;
pub mod server;

#[cfg(test)]
mod tests;

#[cfg(unix)]
use std::path::PathBuf;

/// Path to the daemon control socket (honors `ARVOLO_CONFIG_DIR`). Delegates to
/// the shared crate so the CLI and GUI always resolve the same path.
///
/// Unix only, symmetrically with `pipe_name` below: on Windows nothing here has a
/// socket to name, and an unused re-export is a claim that something exists when
/// it does not.
#[cfg(unix)]
pub fn socket_path() -> PathBuf {
    arvolo_ipc::socket_path()
}

/// The Windows named pipe the daemon listens on. Same delegation, same reason:
/// the CLI and the GUI must resolve the same channel, and only one place should
/// decide how its name is built.
#[cfg(windows)]
pub fn pipe_name() -> String {
    arvolo_ipc::pipe_name()
}
