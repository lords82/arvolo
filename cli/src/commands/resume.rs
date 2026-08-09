//! `arvolo resume <id> [file]` — pick up where something left off.
//!
//! There used to be two resumes: `arvolo resume <n>` for a transfer the daemon
//! had paused, and `arvolo send --resume <id|ticket>` for replaying an
//! interrupted send. Same word, same user intent, two places to look — so this
//! is one verb over the three things that can be resumed, told apart by the
//! shape of the id exactly the way [`crate::commands::cancel`] does it:
//!
//! * a **saved session** (8 hex chars) — replays a send under the ticket already
//!   handed out; recovers a delivery to a contact too, with no file to re-supply;
//! * an **`arvc…` ticket** — re-serves a plain P2P send, and needs its file
//!   alongside, since a plain ticket carries no record of what it was serving;
//! * a **paused transfer** (a plain number) — the daemon owns it;
//! * a **partial download** (a path that exists) — the receiving end, resumed
//!   from the ticket recorded beside it. This is the one case that used to have
//!   no way back: a download started from a pairing code kept its ticket only in
//!   memory, and the code itself is consumed on first use, so an interruption
//!   left a perfectly good partial that nothing could finish.
//!
//! The order matters and is the same trap [`crate::commands::cancel`] documents:
//! an 8-hex session id is all digits about one time in forty, so `parse::<u64>()`
//! must not get first refusal or `73406183` would be read as a transfer number
//! and the user sent to "no such transfer" for a session sitting in their list.
//!
//! Only the transfer branch needs a daemon; a session replay is pure P2P and
//! works from a bare CLI on any platform, which is why this module isn't
//! unix-gated.

use std::path::PathBuf;

use anyhow::{bail, Result};
use arvolo_core::chunked::ChunkTicket;

use crate::commands::send::{resume_by_id, resume_by_ticket};
use crate::sessions;

pub(crate) async fn resume_cmd(id: String, path: Option<PathBuf>, qr: bool) -> Result<()> {
    // `sessions::load` 404s with its own message; we only want to know if it's there.
    if sessions::load(&id).is_ok() {
        anyhow::ensure!(
            path.is_none(),
            "resuming session '{id}' takes no file — the saved session remembers what it was sending"
        );
        return resume_by_id(&id, qr).await;
    }
    if ChunkTicket::looks_like(&id) {
        let Some(path) = path else {
            bail!(
                "resuming from an `arvc…` ticket needs the file it was serving: \
                 `arvolo resume <ticket> <file>`"
            )
        };
        return resume_by_ticket(&id, &path, qr).await;
    }
    if let Ok(tid) = id.parse::<u64>() {
        anyhow::ensure!(
            path.is_none(),
            "resuming transfer {tid} takes no file — the daemon still has it"
        );
        return resume_transfer(tid).await;
    }
    // A path to a partial download: the receiving side. Checked last, and only
    // when the path actually exists, so it can never shadow the id shapes above.
    let partial = PathBuf::from(&id);
    if partial.exists() {
        anyhow::ensure!(
            path.is_none(),
            "resuming a partial download takes no second argument — \
             `arvolo resume {id}`"
        );
        return resume_download(&partial).await;
    }
    bail!("no paused transfer or resumable send with id '{id}' — see `arvolo status`")
}

/// Finish a partial download from the ticket recorded next to it.
async fn resume_download(partial: &std::path::Path) -> Result<()> {
    let Some(ticket) = arvolo_core::flow::read_ticket(partial) else {
        bail!(
            "no resume record beside {} — nothing here remembers where this came from.\n\
             (Only downloads started by a recent arvolo leave one; ask the sender for a new code.)",
            partial.display()
        )
    };
    eprintln!(
        "Resuming {} — reconnecting to the sender…",
        partial.display()
    );
    // Hand back the same destination it was going to, so the existing partial and
    // its piece bitfield are the ones picked up.
    crate::commands::receive::recv_ticket(ticket, Some(partial.to_path_buf()), None).await
}

/// A paused transfer: only the daemon can restart one, since only the daemon is
/// running it.
#[cfg(unix)]
async fn resume_transfer(id: u64) -> Result<()> {
    crate::commands::daemon::resume_cmd(id).await
}

#[cfg(not(unix))]
async fn resume_transfer(id: u64) -> Result<()> {
    bail!("transfer {id} needs a running daemon, which this platform doesn't support yet")
}

#[cfg(test)]
mod tests {
    //! Each test holds the process-global `testlock::ENV` guard across its awaits
    //! on purpose: it keeps `ARVOLO_CONFIG_DIR` — and so the stores these commands
    //! read — pointed at this test's temp dir for the whole body.
    #![allow(clippy::await_holding_lock)]

    use super::*;

    /// The all-digit trap, on this verb too: an 8-hex session id that happens to
    /// be all digits must reach its session, not `parse::<u64>()`.
    #[tokio::test]
    async fn an_all_digit_session_id_is_not_mistaken_for_a_transfer() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        // Written straight to disk: `save` derives the id from the ticket, so an
        // all-digit one can't be requested, only stumbled into.
        let toml = r#"
id = "73406183"
key_hex = "0707070707070707070707070707070707070707070707070707070707070707"
node_key_hex = "0909090909090909090909090909090909090909090909090909090909090909"
sources = ["/nonexistent/video.mp4"]
name = "video.mp4"
archive = false
total_size = 190
chunks = 12
created = 1700000000
ticket = "arvc-not-a-real-ticket"
"#;
        let sessions_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(sessions_dir.join("73406183.toml"), toml).unwrap();
        assert!(sessions::load("73406183").is_ok());

        // The stored ticket is a stub, so the replay stops at parsing it — and that
        // *message* is what proves the dispatch: only `resume_by_id` reads a saved
        // ticket. Down the transfer path this id would produce "no daemon" or "no
        // such transfer" instead, which is the regression being pinned.
        let err = resume_cmd("73406183".into(), None, false)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("saved ticket"),
            "an all-digit id must reach the session it names; got: {err}"
        );

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// An `arvc…` ticket carries no record of what it served, so the file is not
    /// optional — and saying so beats a confusing failure deeper in.
    #[tokio::test]
    async fn a_ticket_without_its_file_says_what_is_missing() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let err = resume_cmd("arvc-not-a-real-ticket".into(), None, false)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("needs the file"), "got: {err}");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// An id nothing claims points at the one list that would show it.
    #[tokio::test]
    async fn an_unknown_id_says_where_to_look() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let err = resume_cmd("zzzznotanid".into(), None, false)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("arvolo status"), "got: {err}");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
