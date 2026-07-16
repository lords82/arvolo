//! arvolo CLI.
//!
//! One `send` picks the channel by situation; one `recv` takes any ticket/code:
//!   arvolo send <file>                 P2P: prints an `arvc…` ticket to share
//!   arvolo send <file> --to <id>       to a contact: live if online, else mailbox + `arvm…`
//!   arvolo send <file> --to <id> --ticket   force the mailbox/`arvm…` path
//!   arvolo send <file> --link          public browser download link
//!   arvolo recv <arvc…|arvm…|code>     fetch — auto-detects P2P vs mailbox
//!
//! P2P transport is encrypted by QUIC and each chunk is end-to-end encrypted;
//! the offline path is end-to-end encrypted with HPKE. The relay only ever sees
//! ciphertext. All transfer orchestration lives in `arvolo_core::flow`; this CLI
//! just drives it and renders progress.

use std::io::IsTerminal;
use std::sync::atomic::Ordering;

use anyhow::Result;
use clap::Parser;

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
mod output;
#[cfg(test)]
pub(crate) mod testlock;
mod ui;
mod util;

use args::{Cli, Command, DeviceAction, SyncAction};
use commands::cancel::cancel_cmd;
use commands::contacts::contacts_cmd;
#[cfg(unix)]
use commands::daemon::{accept_cmd, daemon, pause_cmd, reject_cmd, resume_cmd};
use commands::identity::{id, name_cmd, version_cmd};
use commands::offline::revoke;
use commands::receive::{listen, recv};
use commands::send::send;
use commands::transfers::transfers_cmd;
use output::{init_tracing, VERBOSITY};
use ui::*;
use util::*;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    VERBOSITY.store(cli.verbose, Ordering::Relaxed);
    init_tracing(cli.verbose);

    // First run: if there's no config yet, guide an interactive user through a
    // one-question setup and write a self-documenting config.toml.
    maybe_first_run_wizard();
    // Bridge config.toml → ARVOLO_* (env still wins) and pin the scratch dir.
    book::apply_config_to_env();

    match cli.command {
        Command::Send {
            paths,
            resume,
            code,
            relay,
            use_http,
            to,
            ticket,
            note,
            link,
            ttl,
            max,
            password,
            foreground,
            qr,
        } => {
            send(
                paths, resume, code, relay, use_http, to, ticket, note, link, ttl, max, password,
                foreground, qr,
            )
            .await
        }
        Command::Recv {
            ticket,
            out,
            password,
        } => recv(ticket, out, password).await,
        Command::Id => id(),
        Command::Name { name } => name_cmd(name),
        Command::Version => version_cmd().await,
        Command::Contacts { action } => contacts_cmd(action).await,
        Command::Device { action } => match action {
            DeviceAction::Pair {
                relay,
                use_http,
                qr,
            } => sync::device_pair(relay, use_http, qr).await,
            DeviceAction::Join { code, yes } => sync::device_join(code, yes).await,
        },
        Command::Sync { action } => match action.unwrap_or(SyncAction::Now) {
            SyncAction::Now => sync::sync_now(None, false).await,
            SyncAction::Status => sync::sync_status().await,
        },
        Command::Transfers { watch, action } => transfers_cmd(watch, action).await,
        Command::Revoke { target, token } => revoke(target, token).await,
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
        Command::Cancel { id } => cancel_cmd(id).await,
        #[cfg(unix)]
        Command::Pause { id } => pause_cmd(id).await,
        #[cfg(unix)]
        Command::Resume { id } => resume_cmd(id).await,
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
