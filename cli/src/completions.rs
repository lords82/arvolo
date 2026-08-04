//! Shell completion: the integration script, and the candidates that make it
//! worth having.
//!
//! Completion here is **computed by arvolo at the moment you press TAB**, not
//! baked into a script at install time. That is the whole point: a static script
//! can only ever offer the command names, while this one offers the things you
//! actually type — your contact names after `arvolo send`, and the live ids from
//! `arvolo status` after `cancel`, `resume`, `pause`, `accept` and `reject`.
//!
//! The price is that every candidate function runs *inside your shell's TAB*, so
//! two rules are absolute:
//!
//! * **Never touch the network.** Note that the obvious source for contact names,
//!   [`crate::commands::contacts`]'s listing, probes the relay for presence — so
//!   we read the address book file directly instead.
//! * **Never block.** The daemon is asked over the control socket with a hard
//!   [`DAEMON_TIMEOUT`], and anything that fails, hangs or isn't there degrades
//!   silently to what's on disk. A completer that errors is worse than useless:
//!   the shell prints the noise straight into the user's command line.

use clap_complete::engine::CompletionCandidate;

use crate::args::CompletionShell;
use crate::{book, deposits, sessions};

/// How long a TAB is allowed to wait on the daemon before we give up and answer
/// from disk alone. Generous for a socket on the same machine, still far below
/// the ~200 ms where a shell starts to feel stuck.
#[cfg(unix)]
const DAEMON_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(150);

/// `arvolo completions <shell>` — print the shell-side integration.
///
/// This is the same registration [`clap_complete::CompleteEnv`] emits for
/// `COMPLETE=<shell> arvolo`; giving it a real subcommand just means the user
/// has one documented thing to run and redirect.
pub(crate) fn completions_cmd(shell: CompletionShell) -> anyhow::Result<()> {
    let shells = clap_complete::env::Shells::builtins();
    let completer = shells
        .completer(shell.as_str())
        // Unreachable in practice: `CompletionShell` is exactly the builtin set.
        .ok_or_else(|| anyhow::anyhow!("no completion support for {}", shell.as_str()))?;
    completer.write_registration(
        "COMPLETE",
        "arvolo",
        "arvolo",
        "arvolo",
        &mut std::io::stdout(),
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Candidates
// ---------------------------------------------------------------------------

/// Saved contact names, for `arvolo send <WHO>` and the `contacts` subcommands.
///
/// Read straight from the address book: no relay, no presence probe.
pub(crate) fn contact_candidates() -> Vec<CompletionCandidate> {
    book::contact_list()
        .into_iter()
        .map(|(name, id)| {
            // The mark that matters when you're about to send to someone is
            // whether you ever checked their key out-of-band.
            let mark = if book::is_verified(&id) {
                if book::is_trusted(&id) {
                    "verified, trusted"
                } else {
                    "verified"
                }
            } else {
                "unverified"
            };
            CompletionCandidate::new(name).help(Some(mark.into()))
        })
        .collect()
}

/// Ids for `arvolo cancel`: everything that command can take back — live
/// transfers, files still sitting on a relay, and resumable sends.
pub(crate) fn cancelable_candidates() -> Vec<CompletionCandidate> {
    let mut out = live_transfer_candidates(|status| status == "active" || status == "paused");
    out.extend(deposit_candidates());
    out.extend(session_candidates());
    out
}

/// Ids for `arvolo resume`: saved sessions, plus whatever the daemon is holding.
pub(crate) fn resumable_candidates() -> Vec<CompletionCandidate> {
    let mut out = live_transfer_candidates(|status| status == "paused");
    out.extend(session_candidates());
    out
}

/// Ids for `arvolo pause`: only a transfer that is actually running.
pub(crate) fn transfer_candidates() -> Vec<CompletionCandidate> {
    live_transfer_candidates(|status| status == "active")
}

/// Offer ids for `arvolo accept` / `arvolo reject`. Both are daemon-only verbs,
/// so off unix there is nothing to offer.
#[cfg(unix)]
pub(crate) fn offer_candidates() -> Vec<CompletionCandidate> {
    let Some(offers) = with_daemon(|mut c| async move { c.list_pending().await }) else {
        return Vec::new();
    };
    offers
        .into_iter()
        .map(|o| {
            let who = if o.sender_name.is_empty() {
                o.from
            } else {
                o.sender_name
            };
            CompletionCandidate::new(o.id).help(Some(format!("{} from {who}", o.name).into()))
        })
        .collect()
}

/// Deposits are local records, so they complete with or without a daemon.
fn deposit_candidates() -> Vec<CompletionCandidate> {
    deposits::list()
        .into_iter()
        .map(|d| {
            // `kind` is the discriminator, not the presence of `link`: a record
            // written before the URL was stored is still a link.
            let kind = if d.kind == deposits::KIND_LINK {
                "link"
            } else {
                "mailbox"
            };
            let expired = if d.expired() { ", expired" } else { "" };
            CompletionCandidate::new(d.id).help(Some(format!("{kind}: {}{expired}", d.name).into()))
        })
        .collect()
}

/// Likewise resumable sends: the session file on disk is the whole record.
fn session_candidates() -> Vec<CompletionCandidate> {
    sessions::list()
        .into_iter()
        .map(|s| CompletionCandidate::new(s.id).help(Some(format!("resumable: {}", s.name).into())))
        .collect()
}

/// Transfer rows from the daemon, kept to the ones the caller can act on.
///
/// Empty when there is no daemon — which is not an error worth reporting inside
/// a TAB, just fewer candidates.
#[cfg(unix)]
fn live_transfer_candidates(wanted: fn(&str) -> bool) -> Vec<CompletionCandidate> {
    let Some(transfers) = with_daemon(|mut c| async move { c.list().await }) else {
        return Vec::new();
    };
    transfers
        .into_iter()
        .filter(|t| wanted(&t.status))
        .map(|t| {
            let arrow = if t.direction == "send" { "→" } else { "←" };
            CompletionCandidate::new(t.id.to_string())
                .help(Some(format!("{arrow} {} ({})", t.name, t.status).into()))
        })
        .collect()
}

#[cfg(not(unix))]
fn live_transfer_candidates(_wanted: fn(&str) -> bool) -> Vec<CompletionCandidate> {
    Vec::new()
}

/// Run one short request against the daemon, or give up.
///
/// Deliberately *not* [`crate::commands::daemon::daemon_client`]: that one also
/// negotiates versions and writes advice to stderr when they differ, which would
/// dump a paragraph into the middle of the user's half-typed command. Here a
/// mismatched, busy or absent daemon all mean the same thing — no candidates.
///
/// The work happens on its own thread with its own runtime. Completion runs
/// before `main` enters the async runtime, but borrowing a thread costs
/// microseconds and keeps that ordering from becoming a trap for later.
#[cfg(unix)]
fn with_daemon<T, F, Fut>(f: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce(crate::ipc::client::DaemonClient) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        rt.block_on(async move {
            tokio::time::timeout(DAEMON_TIMEOUT, async move {
                let client = crate::ipc::client::DaemonClient::connect().await.ok()?;
                f(client).await.ok()
            })
            .await
            .ok()
            .flatten()
        })
    })
    .join()
    .ok()
    .flatten()
}
