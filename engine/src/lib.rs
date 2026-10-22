//! Everything the Arvolo daemon knows, minus the socket it is usually reached
//! through and the terminal it usually prints to.
//!
//! `arvolo-core` is the transfer engine: identities, tickets, chunks, the relay.
//! It deliberately knows nothing about *this* installation — who is in your
//! address book, which relay you chose, what this machine is called. That
//! knowledge lived in the CLI, because for a long time the CLI was the only thing
//! that had it.
//!
//! A phone has no daemon to talk to. The engine has to run inside the app, and
//! the app has to reach it by a function call rather than a Unix socket — so
//! anything the daemon does on behalf of a client belongs on this side of the
//! line, and only argument parsing, progress bars and the socket itself stay in
//! `arvolo-cli`. The desktop keeps its daemon and loses nothing: the socket
//! server becomes a thin wrapper around the same calls the phone makes directly.

pub mod announce;
pub mod book;
pub mod daemon;
pub mod deposits;
pub mod handles;
pub mod history;
pub mod identity;
pub mod pairing;
pub mod payload;
pub mod sessions;
pub mod start;
pub mod sync;
pub mod text;

/// Current unix time in seconds.
///
/// At the crate root because everything here stamps something with it — a
/// deposit's expiry, when a contact was last dealt with, a history row — and none
/// of those owns it.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
/// Serializes tests that mutate the process-global `ARVOLO_CONFIG_DIR`.
///
/// Not behind `#[cfg(test)]`: `arvolo-cli` has tests that move the same
/// directory underneath the same stores, and a lock that is only compiled for
/// this crate's own tests is invisible to them. Two lines of always-compiled
/// code buys one lock both crates can hold.
pub mod testlock;
