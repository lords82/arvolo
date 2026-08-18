//! arvolo CLI.
//!
//! One verb sends, one verb receives:
//!   arvolo send <file>                 writes <file>.arvolo — share it like a .torrent
//!   arvolo send <file> --to alice      to a contact: live if online, else mailbox + `arvm…`
//!   arvolo send <file> --link          public browser download URL
//!   arvolo send <file> --code          short code to read out loud (payload stays P2P)
//!   arvolo recv <.arvolo|arvc…|arvm…|code|link|handle>   fetch — auto-detects which
//!   arvolo recv                        with nothing to paste: what's waiting for you
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
mod handles;
mod history;
// The daemon speaks over a local control channel: a Unix-domain socket where
// there is one, a named pipe on Windows. Which of the two is decided inside
// `arvolo_ipc` and in `commands::daemon::open_control`; everything above that —
// this module, the protocol, every command that drives a daemon — is the same
// code on both.
mod ipc;
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

use args::{build_cli, Cli, Command, DaemonAction, DeviceAction, MeAction};
use commands::cancel::cancel_cmd;
use commands::contacts::contacts_cmd;
use commands::daemon::{daemon, daemon_start, daemon_status_cmd, daemon_stop_cmd, pause_cmd};
use commands::history::history_cmd;
use commands::identity::{me, name_cmd};
use commands::receive::{decline_cmd, listen, recv};
use commands::resume::resume_cmd;
use commands::send::{send_cmd, SendOpts};
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
    // A proxy is checked here, before any request is built, so a typo is one clear
    // error at the top instead of every relay operation failing further down (the
    // client refuses to connect directly when a proxy is set but unusable, which
    // is right but says nothing about why).
    if let Some(p) = arvolo_core::http::configured_proxy() {
        if let Err(e) = arvolo_core::http::check_proxy(&p) {
            anyhow::bail!(
                "proxy {p:?} is not usable ({e}). Fix `proxy` in config.toml or \
                 {}, or unset it to connect directly.",
                arvolo_core::http::PROXY_ENV
            );
        }
    }
    // Windows only, and once per process: narrow the config directory's access
    // list so the records written inside it — identity, revoke tokens, resume
    // records — inherit it. On unix each of those files is chmod'd where it is
    // written instead; see the `restrict` helpers by the record stores.
    book::restrict_config_dir();

    match cli.command {
        Command::Send {
            paths,
            to,
            mailbox,
            link,
            code,
            ticket,
            note,
            ttl,
            max,
            password,
            keep,
            foreground,
            qr,
            relay,
        } => {
            send_cmd(SendOpts {
                paths,
                to,
                mailbox,
                link,
                code,
                ticket,
                note,
                ttl,
                max,
                password,
                keep,
                foreground,
                qr,
                relay: relay.relay,
            })
            .await
        }
        Command::Recv {
            what,
            out,
            password,
        } => recv(what, out, password).await,
        Command::Decline { handle } => decline_cmd(handle).await,
        Command::Me { action } => match action {
            None => me(),
            Some(MeAction::Name { name }) => name_cmd(name),
        },
        Command::Completions { shell } => completions::completions_cmd(shell),
        Command::Contacts { action } => contacts_cmd(action).await,
        Command::Device { action } => match action {
            DeviceAction::Pair { qr, relay } => sync::device_pair(relay.relay, qr).await,
            DeviceAction::Join { code, yes } => sync::device_join(code, yes).await,
            DeviceAction::Sync => sync::sync_now(None, false).await,
            DeviceAction::Status => sync::sync_status().await,
        },
        Command::Status { watch, action } => status_cmd(watch, action).await,
        Command::History { action } => history_cmd(action),
        Command::Listen {
            accept,
            no_sync,
            relay,
        } => listen(accept, no_sync, relay.relay).await,
        Command::Daemon { action } => match action {
            DaemonAction::Run {
                download_dir,
                no_sync,
                relay,
            } => daemon(download_dir, relay.relay, no_sync).await,
            DaemonAction::Start {
                download_dir,
                no_sync,
                relay,
            } => daemon_start(download_dir, relay.relay, no_sync).await,
            DaemonAction::Stop => daemon_stop_cmd().await,
            DaemonAction::Status => daemon_status_cmd().await,
        },
        Command::Cancel { id } => cancel_cmd(id).await,
        Command::Pause { id } => pause_cmd(id).await,
        Command::Resume { id, path } => resume_cmd(id, path).await,
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
                // Skipping is not a degraded state: relay-requiring features fall
                // back to the compiled-in default. Say which host that is — it's
                // a third-party server this install will now talk to.
                None if !book::BUILTIN_RELAY.trim().is_empty() => println!(
                    "  No relay set — codes/mailbox/links use the built-in default \
                     ({}).\n  Set `relay` in the file (or pass --relay) to use your own.\n",
                    book::BUILTIN_RELAY
                ),
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
    println!("Relay URL: brokers pairing codes, sends to a contact, the mailbox,");
    println!("download links and the swarm. Leave empty to use the built-in default");
    if book::BUILTIN_RELAY.trim().is_empty() {
        println!("(none in this build — plain P2P `arvc…` tickets still work).");
    } else {
        println!("({}) — plain P2P `arvc…` tickets work without any.", book::BUILTIN_RELAY);
    }
    println!("  • Production (TLS):  just the hostname, e.g. relay.example.com");
    println!("  • LAN/dev (no TLS):  http://host:6282");
    print!("Relay [built-in]: ");
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
