//! arvolo CLI.
//!
//! Sending splits on one question — *do I know who gets this?* If you do, `send`
//! delivers to them; if you don't, you name the artefact you want to hand around:
//!   arvolo send <who> <file>       to a contact: live if online, else mailbox + `arvm…`
//!   arvolo link <file>             public browser download link
//!   arvolo code <file>             short pairing code to read out loud
//!   arvolo ticket <file>           self-contained `arvc…` ticket to paste
//!   arvolo recv <arvc…|arvm…|code|link>   fetch — one verb, auto-detects which
//!
//! P2P transport is encrypted by QUIC and each chunk is end-to-end encrypted;
//! the offline path is end-to-end encrypted with HPKE. The relay only ever sees
//! ciphertext. All transfer orchestration lives in `arvolo_core::flow`; this CLI
//! just drives it and renders progress.

use std::io::IsTerminal;
use std::sync::atomic::Ordering;

use anyhow::Result;
use clap::FromArgMatches;

mod book;
mod deposits;
mod history;
// The daemon speaks over a Unix-domain socket; Windows (a future GUI target)
// will get a named-pipe transport behind the same protocol later.
#[cfg(unix)]
mod ipc;
#[cfg(unix)]
mod notify;
mod sessions;
mod sync;

mod args;
mod commands;
mod completions;
mod output;
#[cfg(test)]
pub(crate) mod testlock;
mod ui;
mod util;

use args::{build_cli, Cli, Command, DeviceAction, MeAction};
use commands::cancel::cancel_cmd;
use commands::contacts::contacts_cmd;
#[cfg(unix)]
use commands::daemon::{accept_cmd, daemon, pause_cmd, reject_cmd};
use commands::history::history_cmd;
use commands::identity::{me, name_cmd};
use commands::receive::{listen, recv};
use commands::resume::resume_cmd;
use commands::send::{code_cmd, link, send_to, ticket_cmd};
use commands::status::status_cmd;
use output::{init_tracing, VERBOSITY};
use ui::*;
use util::*;

fn main() -> Result<()> {
    restore_default_sigpipe();
    // Answer the shell first, before anything else exists to get in the way — no
    // tokio runtime, no first-run wizard, and above all nothing written to stdout,
    // which at this point belongs to the completion protocol. `complete()` returns
    // immediately on a normal run and exits the process on a completion request.
    //
    // Running outside the runtime is also what lets a candidate provider block on
    // a short daemon query of its own; see `completions::with_daemon`.
    clap_complete::CompleteEnv::with_factory(build_cli).complete();
    run()
}

#[tokio::main]
async fn run() -> Result<()> {
    // Built rather than derived-parsed, so `--help` shows the grouped command
    // listing; `e.exit()` keeps clap's own exit codes and stream choices.
    let matches = build_cli().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    VERBOSITY.store(cli.verbose, Ordering::Relaxed);
    init_tracing(cli.verbose);

    // First run: if there's no config yet, guide an interactive user through a
    // one-question setup and write a self-documenting config.toml.
    maybe_first_run_wizard();
    // Bridge config.toml → ARVOLO_* (env still wins) and pin the scratch dir.
    book::apply_config_to_env();

    match cli.command {
        Command::Send {
            who,
            paths,
            deposit,
            note,
            relay,
            use_http,
            ttl,
            max,
            password,
            qr,
        } => {
            send_to(
                who, paths, deposit, note, relay, use_http, ttl, max, password, qr,
            )
            .await
        }
        Command::Link {
            paths,
            relay,
            use_http,
            ttl,
            max,
            password,
            qr,
        } => link(paths, relay, use_http, ttl, max, password, qr).await,
        Command::Code {
            paths,
            relay,
            use_http,
            keep,
            foreground,
            qr,
        } => code_cmd(paths, relay, use_http, keep, foreground, qr).await,
        Command::Ticket {
            paths,
            relay,
            use_http,
            foreground,
            qr,
        } => ticket_cmd(paths, relay, use_http, foreground, qr).await,
        Command::Recv {
            ticket,
            out,
            password,
        } => recv(ticket, out, password).await,
        Command::Me { action } => match action {
            None => me(),
            Some(MeAction::Name { name }) => name_cmd(name),
        },
        Command::Completions { shell } => completions::completions_cmd(shell),
        Command::Contacts { action } => contacts_cmd(action).await,
        Command::Device { action } => match action {
            DeviceAction::Pair {
                relay,
                use_http,
                qr,
            } => sync::device_pair(relay, use_http, qr).await,
            DeviceAction::Join { code, yes } => sync::device_join(code, yes).await,
            DeviceAction::Sync => sync::sync_now(None, false).await,
            DeviceAction::Status => sync::sync_status().await,
        },
        Command::Status { watch, action } => status_cmd(watch, action).await,
        Command::History { all, action } => history_cmd(all, action),
        Command::Listen {
            download_dir,
            relay,
            use_http,
            auto_accept_contacts,
            auto_accept_verified,
            yes,
            no_sync,
        } => {
            listen(
                download_dir,
                relay,
                use_http,
                auto_accept_contacts,
                auto_accept_verified,
                yes,
                no_sync,
            )
            .await
        }
        #[cfg(unix)]
        Command::Daemon {
            download_dir,
            relay,
            use_http,
            no_sync,
        } => daemon(download_dir, relay, use_http, no_sync).await,
        #[cfg(unix)]
        Command::Accept { offer_id, out } => accept_cmd(offer_id, out).await,
        #[cfg(unix)]
        Command::Reject { offer_id } => reject_cmd(offer_id).await,
        Command::Cancel { id, token } => cancel_cmd(id, token).await,
        #[cfg(unix)]
        Command::Pause { id } => pause_cmd(id).await,
        Command::Resume { id, path, qr } => resume_cmd(id, path, qr).await,
    }
}

/// Die quietly when the reader goes away, the way every other Unix tool does.
///
/// Rust's runtime ignores `SIGPIPE` so that a failed write surfaces as an
/// `io::Error` — sound for a library, wrong for a CLI: `println!` has nowhere to
/// return that error to, so it panics. `arvolo history | head` would end in a
/// backtrace about a broken pipe rather than simply stopping, and any command
/// that prints a long list invites exactly that pipe.
///
/// Restoring the default disposition makes the process take the signal and exit,
/// which is what `head` closing its end is asking for.
fn restore_default_sigpipe() {
    #[cfg(unix)]
    // SAFETY: `signal` with `SIG_DFL` on `SIGPIPE` is async-signal-safe and this
    // runs before any thread is spawned, so no other thread can observe the
    // handler mid-change.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// First-run setup: when no `config.toml` exists yet and we're attached to an
/// interactive terminal, ask the one thing that matters (the relay) and write a
/// self-documenting config. Silently skipped when non-interactive (scripts,
/// systemd) or disabled via `ARVOLO_NO_WIZARD`, so nothing ever blocks headless.
fn maybe_first_run_wizard() {
    if book::config_exists() || std::env::var_os("ARVOLO_NO_WIZARD").is_some() {
        return;
    }
    // Need a real TTY on both ends to prompt; otherwise leave the config absent
    // (commands that need a relay error normally with a clear message).
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return;
    }
    let relay = prompt_relay();
    match book::write_default_config(relay.as_deref()) {
        Ok(()) => {
            println!("\n✓ Saved {}", book::config_path().display());
            match relay {
                Some(r) => println!("  relay = {r}\n"),
                None => println!(
                    "  No relay set — codes/mailbox/links need one. Edit the file or \
                     pass --relay.\n"
                ),
            }
        }
        Err(e) => eprintln!("warning: could not write config: {e:#}"),
    }
}

/// Prompt for the relay URL (the only required setting). Empty = skip.
fn prompt_relay() -> Option<String> {
    use std::io::Write;
    println!("\nWelcome to Arvolo — no configuration found, quick one-time setup.\n");
    println!("Relay URL: brokers pairing codes, `send --to`, the mailbox, download");
    println!("links and the swarm. Leave empty to skip (plain P2P `arvc…` tickets");
    println!("still work without a relay).");
    println!("  • Production (TLS):  just the hostname, e.g. relay.example.com");
    println!("  • LAN/dev (no TLS):  http://host:6282");
    print!("Relay [none]: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return None;
    }
    let t = line.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}
