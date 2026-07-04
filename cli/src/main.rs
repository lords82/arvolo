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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use arvolo_core::chunked::{ChunkTicket, KeyDelivery};
use arvolo_core::code;
use arvolo_core::crypto::{Identity, PublicId};
use arvolo_core::flow::{self, ChunkSource, RecvEvent, SendEvent};
use arvolo_core::manager::{Direction, ManagerEvent, TransferManager};
use arvolo_core::transfer::RelayChoice;
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use tokio_util::sync::CancellationToken;

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

/// Serializes tests that mutate the process-global `ARVOLO_CONFIG_DIR`, which
/// several stores read; without this they race under the parallel test runner.
#[cfg(test)]
pub(crate) mod testlock {
    use std::sync::Mutex;
    pub static ENV: Mutex<()> = Mutex::new(());
}

/// Process-global verbosity, set once from the `-v/--verbose` count in `main`.
/// Read by [`verbosity`] and the [`vprintln!`] macro so command functions and
/// their event callbacks can narrate extra detail without threading a flag
/// through every signature.
static VERBOSITY: AtomicU8 = AtomicU8::new(0);

/// How many `-v` the user passed (0 = quiet, 1 = steps, 2+ = network internals).
fn verbosity() -> u8 {
    VERBOSITY.load(Ordering::Relaxed)
}

/// `eprintln!` that only fires at `-v` or higher — a step explanation for users
/// who want to follow (or debug) what a command is doing. Prefixed so verbose
/// narration is easy to tell apart from normal output.
macro_rules! vprintln {
    ($($arg:tt)*) => {
        if crate::verbosity() >= 1 {
            eprintln!("· {}", format!($($arg)*));
        }
    };
}

/// Like [`vprintln!`] but only at `-vv` or higher — finer-grained detail (e.g.
/// per-source transitions) that would be too chatty at a single `-v`.
macro_rules! vvprintln {
    ($($arg:tt)*) => {
        if crate::verbosity() >= 2 {
            eprintln!("·· {}", format!($($arg)*));
        }
    };
}

#[derive(Parser)]
#[command(
    name = "arvolo",
    version,
    about = "arvolo — secure cross-platform file sending"
)]
struct Cli {
    /// Explain each step as it happens, in arvolo's own words. `-v` narrates the
    /// transfer (relay chosen, ticket, receiver connected, chunk sources) and
    /// silences iroh's low-level networking noise; `-vv` adds finer detail (e.g.
    /// when chunks switch between the sender and the relay), still without iroh.
    /// `-vvv` opens iroh's raw logs for deep network debugging. An explicit
    /// `RUST_LOG` always wins.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

/// Build the tracing filter from the `-v` count, unless `RUST_LOG` is set (an
/// explicit filter always wins).
///
/// The transfer narration at `-v`/`-vv` is printed by the CLI itself (see
/// [`vprintln!`]), not by tracing — so tracing's job here is the opposite of
/// noisy: keep iroh's chatty transport logs (relay probing, the IPv6
/// "no route to host" warning, per-datagram errors) from drowning that
/// narration. iroh is muted at `-v`/`-vv` and only comes back at `-vvv`, where
/// you're explicitly asking for the raw networking firehose.
fn init_tracing(verbose: u8) {
    let default = match verbose {
        // No flag: unchanged — warnings from every crate (incl. iroh) surface.
        0 => "warn",
        // -v / -vv: arvolo's narration carries the story; drop iroh below warn so
        // its transport chatter doesn't bury it. Genuine iroh *errors* still pass.
        1 | 2 => {
            "warn,iroh=error,iroh_quinn=error,iroh_quinn_udp=error,\
             iroh_quinn_proto=error,iroh_relay=error,iroh_net=error,iroh_base=error"
        }
        // -vvv+: raw iroh networking logs for deep debugging.
        _ => "info,iroh=debug",
    };
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| default.into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

#[derive(Subcommand)]
enum Command {
    /// Send files/folders — the tool picks the channel. Without `--to`: a P2P
    /// `arvc…` ticket to share. With `--to <id>`: live to their daemon if online,
    /// else mailbox + an `arvm…` ticket. `--link` makes a public browser link.
    /// Multiple paths or a folder are packed into one archive automatically.
    Send {
        #[arg(num_args = 0..)]
        paths: Vec<PathBuf>,
        /// Resume an interrupted send so the ticket you already shared stays valid.
        /// Pass a **session id** (see `arvolo sessions list`) — recovers `--to`
        /// sends too, no file needed — OR a plain **`arvc…` ticket** together with
        /// its file (re-serves it; the receiver uses the reprinted ticket).
        #[arg(long, value_name = "ID|TICKET")]
        resume: Option<String>,
        /// Also seed the file to a relay so the recipient can finish even if you
        /// go offline (backfill). Relay host or URL, e.g. relay.example.com
        /// (https assumed; pass --use-http for plaintext).
        #[arg(long)]
        seed_relay: Option<String>,
        /// Show a short pairing code (e.g. 4821-crater-mango) instead of the long
        /// ticket. Needs a relay: --relay, or the ARVOLO_RELAY env var.
        #[arg(long)]
        code: bool,
        /// Rendezvous relay for --code. When given, it is embedded in the code so
        /// the receiver needs no configuration. Host or URL, e.g.
        /// relay.example.com (https assumed; pass --use-http for plaintext).
        #[arg(long)]
        relay: Option<String>,
        /// Treat bare relay addresses as `http://` instead of `https://`
        /// (LAN / dev / plaintext relays). Explicit schemes are always kept.
        #[arg(long)]
        use_http: bool,
        /// Send to a known recipient (a saved contact name or public id). The
        /// tool picks the channel: if they're online it's delivered live to their
        /// daemon; if offline it's deposited on the relay (mailbox) + an `arvm…`
        /// ticket is printed so you can also hand it over. Without `--to` you get
        /// a P2P `arvc…` ticket to share.
        #[arg(long)]
        to: Option<String>,
        /// With `--to`: force the mailbox/`arvm…` path even if the recipient is
        /// online (send-and-forget with a shareable code).
        #[arg(long)]
        ticket: bool,
        /// Produce a public browser **download link** instead (anyone with the
        /// link decrypts it client-side). `--to` is not used.
        #[arg(long)]
        link: bool,
        /// Mailbox/link time-to-live in seconds (default 7 days).
        #[arg(long, default_value_t = 7 * 24 * 3600)]
        ttl: u64,
        /// Mailbox/link max downloads before deletion (default 1 for a sealed
        /// send; unlimited for `--link`).
        #[arg(long)]
        max: Option<u32>,
        /// Password-protect a mailbox/link send (E2E — required to decrypt).
        #[arg(long)]
        password: Option<String>,
        /// Serve the P2P ticket **in this terminal** (blocking, Ctrl-C to stop)
        /// instead of handing it to the daemon. Default: if a daemon is running, a
        /// plain ticket send is served by it in the background (track with `arvolo
        /// transfers`). No effect with `--to`/`--link`/`--code`.
        #[arg(long)]
        foreground: bool,
        /// Also render the ticket/code as a scannable QR code.
        #[arg(long)]
        qr: bool,
    },
    /// Receive from any arvolo ticket or code — the tool picks how automatically:
    /// a P2P ticket (`arvc…`) or pairing code (`N-word-word[@relay]`) fetches live,
    /// an offline/mailbox ticket (`arvm…`) or download link decrypts from the relay.
    Recv {
        ticket: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Password for a password-protected offline ticket / link.
        #[arg(long)]
        password: Option<String>,
    },
    /// Manage your address book of recipients (used by --to).
    Contacts {
        #[command(subcommand)]
        action: ContactAction,
    },
    /// List or delete resumable send sessions (used by `send --resume`).
    Sessions {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Show all transfers — live (in/out) and past — plus offers awaiting
    /// approval. With a daemon running it includes its live state; otherwise it
    /// shows the persisted history. `clear` wipes the history.
    Transfers {
        /// Keep the view open and redraw as transfers progress (needs a daemon).
        #[arg(long)]
        watch: bool,
        #[command(subcommand)]
        action: Option<TransferAction>,
    },
    /// Show your public id (creates an identity on first use).
    Id,
    /// Show the CLI version and whether the daemon is running (and its version).
    Version,
    /// Revoke a mailbox send or a browser link, deleting its blob from the relay.
    Revoke {
        /// The offline ticket (`arvm…`) or download link (`…/dl/<claim>#…`) you
        /// sent. For a link the `#key` part is ignored.
        target: String,
        /// The revoke token printed when you sent it.
        #[arg(long)]
        token: String,
    },
    /// Stay online and receive files pushed to you by contacts: shows each
    /// incoming offer (sender, name, size) and downloads accepted ones
    /// transparently. Needs a relay (--relay / ARVOLO_RELAY / config).
    Listen {
        /// Directory to save accepted downloads into (default: current dir).
        #[arg(long)]
        download_dir: Option<PathBuf>,
        /// Relay host or URL (https assumed; pass --use-http for plaintext).
        /// Defaults to ARVOLO_RELAY / config `relay`.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`.
        #[arg(long)]
        use_http: bool,
        /// Auto-accept offers from saved contacts (still prompt for unknown senders).
        #[arg(long)]
        auto_accept_contacts: bool,
        /// Auto-accept offers from verified contacts only (safer than
        /// --auto-accept-contacts). Still prompts for everyone else.
        #[arg(long)]
        auto_accept_verified: bool,
        /// Accept every incoming offer without prompting.
        #[arg(long)]
        yes: bool,
    },
    /// Run the always-on background engine: stays online, receives files, and
    /// exposes a local control socket so `send`/`transfers`/etc. drive one shared
    /// instance. Meant to run under systemd/launchd. Needs a relay.
    #[cfg(unix)]
    Daemon {
        /// Directory to save accepted downloads into
        /// (default: <config>/downloads).
        #[arg(long)]
        download_dir: Option<PathBuf>,
        /// Relay host or URL (https assumed; pass --use-http for plaintext).
        /// Defaults to ARVOLO_RELAY / config `relay`.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`.
        #[arg(long)]
        use_http: bool,
    },
    /// Accept a parked offer by its id (see `arvolo transfers`) and download it.
    #[cfg(unix)]
    Accept {
        /// The offer id shown by `arvolo transfers`.
        offer_id: String,
        /// Save to this path instead of the daemon's download dir.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Reject a parked offer by its id.
    #[cfg(unix)]
    Reject {
        /// The offer id shown by `arvolo transfers`.
        offer_id: String,
    },
    /// Cancel a running transfer by its id (see `arvolo transfers`).
    #[cfg(unix)]
    Cancel {
        /// The transfer id shown by `arvolo transfers`.
        id: u64,
    },
}

#[derive(Subcommand)]
enum ContactAction {
    /// Save (or update) a contact: a name and their public id.
    Add { name: String, id: String },
    /// List saved contacts (with online status if a relay is configured).
    List,
    /// Remove a saved contact.
    Remove { name: String },
    /// Mark a contact verified after comparing its fingerprint out-of-band.
    Verify { name: String },
    /// Remove a contact's verified mark.
    Unverify { name: String },
    /// Trust a contact so the daemon auto-downloads their files without asking.
    Trust { name: String },
    /// Stop auto-downloading from a contact (their files will ask again).
    Untrust { name: String },
}

#[derive(Subcommand)]
enum TransferAction {
    /// Delete all transfer history.
    Clear,
}

#[derive(Subcommand)]
enum SessionAction {
    /// List resumable send sessions.
    List,
    /// Delete a saved session by id.
    Rm { id: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    VERBOSITY.store(cli.verbose, Ordering::Relaxed);
    init_tracing(cli.verbose);

    match cli.command {
        Command::Send {
            paths,
            resume,
            seed_relay,
            code,
            relay,
            use_http,
            to,
            ticket,
            link,
            ttl,
            max,
            password,
            foreground,
            qr,
        } => {
            send(
                paths, resume, seed_relay, code, relay, use_http, to, ticket, link, ttl, max,
                password, foreground, qr,
            )
            .await
        }
        Command::Recv {
            ticket,
            out,
            password,
        } => recv(ticket, out, password).await,
        Command::Id => id(),
        Command::Version => version_cmd().await,
        Command::Contacts { action } => contacts_cmd(action).await,
        Command::Sessions { action } => sessions_cmd(action).await,
        Command::Transfers { watch, action } => transfers_cmd(watch, action).await,
        Command::Revoke { target, token } => revoke(target, token).await,
        Command::Listen {
            download_dir,
            relay,
            use_http,
            auto_accept_contacts,
            auto_accept_verified,
            yes,
        } => {
            listen(
                download_dir,
                relay,
                use_http,
                auto_accept_contacts,
                auto_accept_verified,
                yes,
            )
            .await
        }
        #[cfg(unix)]
        Command::Daemon {
            download_dir,
            relay,
            use_http,
        } => daemon(download_dir, relay, use_http).await,
        #[cfg(unix)]
        Command::Accept { offer_id, out } => accept_cmd(offer_id, out).await,
        #[cfg(unix)]
        Command::Reject { offer_id } => reject_cmd(offer_id).await,
        #[cfg(unix)]
        Command::Cancel { id } => cancel_cmd(id).await,
    }
}

/// `arvolo transfers` — the unified view: with a daemon running, its live
/// transfers (in/out) + pending offers, then the persisted history below;
/// without one, just the history. `clear` wipes the history.
async fn transfers_cmd(watch: bool, action: Option<TransferAction>) -> Result<()> {
    if let Some(TransferAction::Clear) = action {
        let n = history::clear()?;
        println!("Cleared {n} transfer record(s).");
        return Ok(());
    }

    #[cfg(unix)]
    {
        if let Some(client) = daemon_client().await {
            return show_transfers_live(client, watch).await;
        }
        if watch {
            eprintln!(
                "(no daemon running — showing history only; start `arvolo daemon` for a live view)"
            );
        }
    }
    #[cfg(not(unix))]
    let _ = watch;

    print_history();
    Ok(())
}

/// Print the persisted transfer history (the no-daemon / below-the-live view).
fn print_history() {
    let list = history::list();
    if list.is_empty() {
        println!("history: (none)");
        return;
    }
    println!("history:");
    for rec in list {
        let arrow = if rec.direction == "send" {
            "→"
        } else {
            "←"
        };
        let peer = rec
            .peer_id
            .as_deref()
            .map(|id| book::resolve_name(id).unwrap_or_else(|| id.to_string()))
            .unwrap_or_else(|| "anonymous".into());
        println!(
            "  {arrow} {peer}\t{}\t{}\t{}",
            rec.name,
            human_size(rec.transferred),
            rec.status
        );
    }
}

/// The live daemon view (transfers in/out + pending offers), with the history
/// below and an optional `--watch` redraw loop.
#[cfg(unix)]
async fn show_transfers_live(mut client: ipc::client::DaemonClient, watch: bool) -> Result<()> {
    async fn render(client: &mut ipc::client::DaemonClient) -> Result<()> {
        let st = client.status().await?;
        let transfers = client.list().await?;
        let pending = client.list_pending().await?;
        let ver = if st.version.is_empty() {
            "?".to_string()
        } else {
            st.version.clone()
        };
        println!(
            "daemon {ver}: {}  relay: {}",
            st.public_id,
            st.relay.as_deref().unwrap_or("-")
        );
        if transfers.is_empty() {
            println!("transfers: (none)");
        } else {
            println!("transfers:");
            for t in &transfers {
                print_transfer_dto(t);
            }
        }
        if !pending.is_empty() {
            println!("pending offers (awaiting approval):");
            for o in &pending {
                let who = book::resolve_name(&o.from).unwrap_or_else(|| o.from.clone());
                println!(
                    "  ? {}  {} ({})  — arvolo accept {}",
                    who,
                    o.name,
                    human_size(o.size),
                    o.id
                );
            }
        }
        Ok(())
    }

    render(&mut client).await?;
    print_history();

    if !watch {
        return Ok(());
    }
    let mut events = daemon_events().await?;
    let cancel = cancel_on_ctrl_c();
    println!("\n(watching — Ctrl-C to stop)");
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            ev = events.next() => match ev {
                Ok(Some(_)) => {
                    println!("\n---");
                    render(&mut client).await?;
                }
                Ok(None) => break,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

async fn contacts_cmd(action: ContactAction) -> Result<()> {
    match action {
        ContactAction::Add { name, id } => {
            let key_change = book::contact_add(&name, &id)?;
            println!("Saved contact '{name}'.");
            if let Some(kc) = key_change {
                eprintln!(
                    "\n⚠  The key for '{name}' CHANGED — this could be a reinstall, or a MITM."
                );
                eprintln!("      was fingerprint: {}", kc.old_fingerprint);
                eprintln!("      now fingerprint: {}", kc.new_fingerprint);
                eprintln!(
                    "   The 'verified' mark was cleared. Confirm the new fingerprint out-of-band,"
                );
                eprintln!("   then: arvolo contacts verify {name}");
            }
        }
        ContactAction::List => {
            let list = book::contact_list();
            if list.is_empty() {
                eprintln!("(no contacts yet — add one: arvolo contacts add <name> <id>)");
            }
            // Query presence per contact, if a relay is configured.
            let relay = book::default_relay();
            let client = reqwest::Client::new();
            for (name, id) in list {
                let verified = if book::is_verified(&id) { " ✓" } else { "" };
                let trusted = if book::is_trusted(&id) {
                    " ⬇trusted"
                } else {
                    ""
                };
                let status = match (&relay, book::resolve_recipient(&id).ok()) {
                    (Some(r), Some(pk)) => {
                        if arvolo_core::presence::check_online(&client, r, &pk)
                            .await
                            .unwrap_or(false)
                        {
                            "●"
                        } else {
                            "○"
                        }
                    }
                    _ => "?",
                };
                match book::fingerprint_of(&id) {
                    Some(fp) => println!("{status} {name}{verified}{trusted}\t{id}\t({fp})"),
                    None => println!("{status} {name}{verified}{trusted}\t{id}"),
                }
            }
        }
        ContactAction::Remove { name } => {
            // Clear the ledgers first — they resolve the id via the contact name,
            // which is gone once removed.
            book::unmark_verified(&name).ok();
            book::unmark_trusted(&name).ok();
            if book::contact_remove(&name)? {
                println!("Removed contact '{name}'.");
            } else {
                eprintln!("No such contact '{name}'.");
            }
        }
        ContactAction::Verify { name } => {
            let id = book::mark_verified(&name)?;
            let fp = book::fingerprint_of(&id).unwrap_or_default();
            println!("Marked '{name}' verified.");
            eprintln!("Confirm out-of-band that their fingerprint is: {fp}");
        }
        ContactAction::Unverify { name } => {
            book::unmark_verified(&name)?;
            println!("Cleared verified mark for '{name}'.");
        }
        ContactAction::Trust { name } => {
            let id = book::mark_trusted(&name)?;
            let fp = book::fingerprint_of(&id).unwrap_or_default();
            println!("Trusting '{name}' — files from them auto-download without a prompt.");
            eprintln!("   (fingerprint: {fp})");
            if !book::is_verified(&id) {
                eprintln!(
                    "   tip: they're not verified yet — consider `arvolo contacts verify {name}` first."
                );
            }
        }
        ContactAction::Untrust { name } => {
            book::unmark_trusted(&name)?;
            println!("Cleared trust for '{name}' — their files will ask for approval again.");
        }
    }
    Ok(())
}

async fn sessions_cmd(action: SessionAction) -> Result<()> {
    match action {
        SessionAction::List => {
            let dep_list = deposits::list();
            let resumable = sessions::list();
            if dep_list.is_empty() && resumable.is_empty() {
                eprintln!(
                    "(no sessions yet — saved automatically when you `arvolo send` to the mailbox or as a link)"
                );
                return Ok(());
            }

            // Relay deposits (public links + sealed offline sends). We poll the
            // relay for each one's live status (bounded, so a dead relay can't
            // hang the listing).
            if !dep_list.is_empty() {
                println!("Relay deposits — `arvolo sessions rm <id>` deletes from the relay:\n");
                for r in dep_list {
                    let on_relay = match tokio::time::timeout(
                        Duration::from_secs(5),
                        flow::claim_status(&r.relay, &r.claim),
                    )
                    .await
                    {
                        Ok(Ok(flow::ClaimStatus::Pending)) => "present",
                        Ok(Ok(flow::ClaimStatus::Gone)) => "gone (downloaded / expired / revoked)",
                        _ => "unknown (relay unreachable)",
                    };
                    let expiry = if r.expired() {
                        "EXPIRED".to_string()
                    } else {
                        format!(
                            "in {}",
                            human_duration(r.expires.saturating_sub(now_unix()))
                        )
                    };
                    let kind = if r.kind == deposits::KIND_LINK {
                        "link"
                    } else {
                        "sealed"
                    };
                    println!("● {}  [{kind}]  {}  ({})", r.id, r.name, human_size(r.size));
                    println!("    relay:      {}", r.relay);
                    if let Some(l) = &r.link {
                        println!("    link:       {l}");
                    }
                    if let Some(rcpt) = &r.recipient {
                        let disp = book::resolve_name(rcpt).unwrap_or_else(|| rcpt.clone());
                        println!("    to:         {disp}");
                    }
                    println!("    downloads:  {}", r.max_label());
                    println!(
                        "    created:    {} ago",
                        human_duration(now_unix().saturating_sub(r.created))
                    );
                    println!("    expires:    {expiry}");
                    println!("    on relay:   {on_relay}");
                    println!("    remove:     arvolo sessions rm {}\n", r.id);
                }
            }

            // Resumable P2P send sessions.
            if !resumable.is_empty() {
                println!("Resumable sends — `arvolo send --resume <id>`:\n");
                for rec in resumable {
                    let kind = if rec.archive { "archive" } else { "file" };
                    println!(
                        "  {}  {}  {} chunk(s), {} bytes  [{kind}]",
                        rec.id, rec.name, rec.chunks, rec.total_size
                    );
                }
            }
        }
        SessionAction::Rm { id } => {
            // A relay-deposit session (link / sealed offline): revoke it on the
            // relay first, then drop the local record — so the file and link
            // stop existing, not just the local bookkeeping.
            if let Some(r) = deposits::load(&id) {
                match flow::revoke_offline(&r.relay, &r.claim, &r.revoke_token).await {
                    Ok(()) => println!(
                        "Revoked on the relay — '{}' is deleted; the link/ticket no longer works.",
                        r.name
                    ),
                    Err(e) => {
                        eprintln!("⚠ relay revoke failed ({e}); removing the local session anyway.")
                    }
                }
                deposits::remove(&id)?;
                println!("Removed session '{id}'.");
                return Ok(());
            }
            sessions::remove(&id)?;
            println!("Removed session '{id}'.");
        }
    }
    Ok(())
}

// ---- identity -------------------------------------------------------------

fn identity_path() -> PathBuf {
    if let Ok(p) = std::env::var("ARVOLO_IDENTITY") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/arvolo/identity.key")
}

fn my_identity() -> Result<Identity> {
    Identity::load_or_create(&identity_path()).context("load identity")
}

fn id() -> Result<()> {
    let id = my_identity()?;
    let pubid = id.public();
    println!("{}", encode_id(&pubid));
    eprintln!("fingerprint: {}", pubid.fingerprint());
    eprintln!("(identity stored at {})", identity_path().display());
    Ok(())
}

fn encode_id(p: &PublicId) -> String {
    data_encoding::BASE32_NOPAD
        .encode(&p.to_bytes())
        .to_lowercase()
}

/// `arvolo version` — CLI version + whether a daemon is running (and its version).
async fn version_cmd() -> Result<()> {
    println!("arvolo {} (cli)", env!("CARGO_PKG_VERSION"));
    #[cfg(unix)]
    {
        match daemon_client().await {
            Some(mut c) => match c.status().await {
                Ok(st) => {
                    let ver = if st.version.is_empty() {
                        "unknown (older daemon — restart it to pick up this binary)".to_string()
                    } else {
                        format!("v{}", st.version)
                    };
                    println!(
                        "daemon:  running — {ver}  (relay {}, {} active, {} pending)",
                        st.relay.as_deref().unwrap_or("-"),
                        st.transfers,
                        st.pending
                    );
                }
                Err(e) => println!("daemon:  reachable but status failed: {e:#}"),
            },
            None => println!("daemon:  not running  (start it with `arvolo daemon`)"),
        }
    }
    #[cfg(not(unix))]
    println!("daemon:  not supported on this platform");
    Ok(())
}

/// A cancellation token that fires on Ctrl-C.
fn cancel_on_ctrl_c() -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        t.cancel();
    });
    token
}

/// Announce who a received transfer is from, and record it in the TOFU ledger.
///
/// `id` is `None` for a plain (anonymous, unauthenticated) ticket, or the
/// sender's HPKE-authenticated public-key bytes for a sealed one. Prints to
/// stderr before the progress bar starts.
fn print_sender_banner(id: Option<&[u8]>) {
    let bytes = match id {
        None => {
            eprintln!(
                "⚠  From: anonymous sender — this ticket is not authenticated; anyone \
                 holding it could have created it."
            );
            return;
        }
        Some(b) => b,
    };
    let Ok(pubid) = PublicId::from_bytes(bytes) else {
        eprintln!("⚠  From: a sender with an unreadable identity in the ticket.");
        return;
    };
    let id_b32 = encode_id(&pubid);
    let fp = pubid.fingerprint();
    let status = book::sender_status(&id_b32);
    match (status.name, status.seen_before) {
        (Some(name), _) if status.verified => {
            eprintln!("✓ From: {name}  (verified — fingerprint: {fp})");
        }
        (Some(name), _) => {
            eprintln!("From: {name}  (saved, not verified — fingerprint: {fp})");
            eprintln!("      verify out-of-band, then: arvolo contacts verify {name}");
        }
        (None, true) => {
            eprintln!("From: known sender  (fingerprint: {fp})");
            eprintln!("      id: {id_b32}");
            eprintln!("      not in contacts — save with: arvolo contacts add <name> {id_b32}");
        }
        (None, false) => {
            eprintln!("⚠  From: NEW sender (first time you receive from this identity)");
            eprintln!("      fingerprint: {fp}");
            eprintln!("      id: {id_b32}");
            eprintln!(
                "      Verify the fingerprint out-of-band, then: arvolo contacts add <name> {id_b32}"
            );
        }
    }
    book::record_seen(&id_b32);
}

/// Render a ticket as a QR code on stdout (best-effort).
fn print_qr(data: &str) {
    match qrcode::QrCode::new(data) {
        Ok(code) => {
            let art = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build();
            println!("{art}");
        }
        Err(e) => eprintln!("(could not render QR: {e})"),
    }
}

// ---- P2P ------------------------------------------------------------------

/// Resolve the send inputs to a single payload file: a lone file is sent as-is;
/// a folder or several paths are packed into a temp tar. Returns
/// `(payload, suggested_name, is_archive, temp_to_cleanup)`.
fn resolve_payload(paths: &[PathBuf]) -> Result<(PathBuf, String, bool, Option<PathBuf>)> {
    if paths.len() == 1 && paths[0].is_file() {
        let name = paths[0]
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        return Ok((paths[0].clone(), name, false, None));
    }
    for p in paths {
        anyhow::ensure!(p.exists(), "{} does not exist", p.display());
    }
    let name = if paths.len() == 1 {
        paths[0]
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "bundle".into())
    } else {
        "bundle".into()
    };
    let temp = std::env::temp_dir().join(format!("arvolo-send-{}.tar", std::process::id()));
    flow::pack_tar(paths, &temp).context("pack archive")?;
    Ok((temp.clone(), name, true, Some(temp)))
}

// ---- presence: stay-online receive (listen) and push-to-contact -----------

/// Human-readable byte size for offer/progress display.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Current unix time in seconds.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A compact human duration, largest two units (e.g. "7d", "3h 20m", "45s").
fn human_duration(secs: u64) -> String {
    if secs == 0 {
        return "0s".into();
    }
    let units = [("d", 86400u64), ("h", 3600), ("m", 60), ("s", 1)];
    let mut parts = Vec::new();
    let mut rem = secs;
    for (label, size) in units {
        if rem >= size {
            parts.push(format!("{}{label}", rem / size));
            rem %= size;
        }
        if parts.len() == 2 {
            break;
        }
    }
    parts.join(" ")
}

/// Resolve the relay to use for presence, requiring one (offers can't work P2P).
fn require_relay(relay: Option<String>, use_http: bool) -> Result<String> {
    let resolved = relay
        .map(|r| book::normalize_relay(&r, use_http))
        .or_else(book::default_relay)
        .context(
            "a relay is required: pass --relay <host>, set ARVOLO_RELAY, or configure `relay`",
        )?;
    vprintln!("using relay: {resolved}");
    Ok(resolved)
}

/// Ask the user y/n on stdin (blocking), defaulting to no on EOF/error.
async fn confirm(prompt: String) -> bool {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        eprint!("{prompt} [y/N] ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return false;
        }
        matches!(line.trim(), "y" | "Y" | "yes" | "YES")
    })
    .await
    .unwrap_or(false)
}

async fn listen(
    download_dir: Option<PathBuf>,
    relay: Option<String>,
    use_http: bool,
    auto_accept_contacts: bool,
    auto_accept_verified: bool,
    yes: bool,
) -> Result<()> {
    // If a daemon is already receiving, attach to it as a viewer/approver instead
    // of standing up a second engine (which would fight over presence/inbox).
    #[cfg(unix)]
    {
        if let Some(client) = daemon_client().await {
            if download_dir.is_some() {
                eprintln!(
                    "note: --download-dir is ignored when attaching to the daemon \
                     (it saves to its own configured dir)."
                );
            }
            return listen_attached(client, auto_accept_contacts, auto_accept_verified, yes).await;
        }
    }

    let relay = require_relay(relay, use_http)?;
    let me = my_identity()?;
    let my_id = encode_id(&me.public());
    let download_dir = download_dir
        .or_else(book::default_download_dir)
        .unwrap_or_else(|| PathBuf::from("."));

    let manager = TransferManager::new(me, Some(relay.clone()), download_dir.clone());
    let mut events = manager.subscribe();
    let inbox = manager.spawn_inbox()?;
    vprintln!("inbox poller started — publishing presence and watching for offers on the relay");
    if yes {
        vprintln!("auto-accepting ALL offers (--yes)");
    } else if auto_accept_verified {
        vprintln!("auto-accepting offers from verified contacts only");
    } else if auto_accept_contacts {
        vprintln!("auto-accepting offers from saved contacts");
    }

    eprintln!("Listening as {my_id}");
    eprintln!("Fingerprint: {}", manager.public_id().fingerprint());
    eprintln!("Saving accepted files to {}", download_dir.display());
    eprintln!("Relay: {relay}");
    eprintln!("Ctrl-C to stop.\n");

    let cancel = cancel_on_ctrl_c();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            ev = events.recv() => {
                let ev = match ev {
                    Ok(ev) => ev,
                    // Dropped some events under load — keep going.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                match ev {
                    ManagerEvent::OfferReceived { id, from, name, size } => {
                        let from_b32 = encode_id(&from);
                        let status = book::sender_status(&from_b32);
                        eprintln!("\n📨 Incoming file offer:");
                        eprintln!("   from: {}{}", status.name.clone().unwrap_or_else(|| from_b32.clone()),
                                  if status.verified { " ✓ verified" } else { "" });
                        eprintln!("   fingerprint: {}", from.fingerprint());
                        eprintln!("   file: {name}  ({})", human_size(size));

                        let accept = if yes {
                            true
                        } else if auto_accept_verified && status.verified {
                            eprintln!("   (auto-accepting: verified contact)");
                            true
                        } else if auto_accept_contacts && status.name.is_some() {
                            eprintln!("   (auto-accepting: saved contact)");
                            true
                        } else {
                            confirm(format!("   Accept from {}?", status.name.unwrap_or(from_b32))).await
                        };

                        if accept {
                            match manager.accept_offer(&id, None).await {
                                Ok(_) => eprintln!("   ✓ accepted — downloading…"),
                                Err(e) => eprintln!("   ✗ could not accept: {e:#}"),
                            }
                        } else {
                            manager.reject_offer(&id).await;
                            eprintln!("   ✗ rejected");
                        }
                    }
                    ManagerEvent::Started { direction: Direction::Recv, name, total_size, .. } => {
                        eprintln!("↓ receiving {name} ({})", human_size(total_size));
                    }
                    ManagerEvent::Completed { id, path } => {
                        if let Some(p) = &path {
                            eprintln!("✓ saved {}", p.display());
                        }
                        record_history(&manager, id, "completed");
                    }
                    ManagerEvent::Failed { id, error } => {
                        eprintln!("✗ transfer failed: {error}");
                        record_history(&manager, id, &format!("failed: {error}"));
                    }
                    ManagerEvent::Cancelled { id } => record_history(&manager, id, "cancelled"),
                    _ => {}
                }
            }
        }
    }
    inbox.cancel();
    Ok(())
}

/// How an attached `listen` decides whether to auto-accept an incoming offer.
#[cfg(unix)]
#[derive(Clone, Copy)]
struct AcceptPolicy {
    auto_accept_contacts: bool,
    auto_accept_verified: bool,
    yes: bool,
}

/// Decide + act on one offer over the daemon IPC.
#[cfg(unix)]
async fn handle_attached_offer(
    client: &mut ipc::client::DaemonClient,
    offer: ipc::protocol::OfferDto,
    policy: AcceptPolicy,
) {
    let status = book::sender_status(&offer.from);
    let who = status.name.clone().unwrap_or_else(|| offer.from.clone());
    // Trusted senders are auto-downloaded by the daemon itself; don't also prompt
    // for them here (the daemon already accepted or will).
    if status.trusted {
        eprintln!("⬇ auto-downloading {} from trusted {who}", offer.name);
        return;
    }
    eprintln!("\n📨 Incoming file offer:");
    eprintln!(
        "   from: {who}{}",
        if status.verified { " ✓ verified" } else { "" }
    );
    eprintln!("   file: {}  ({})", offer.name, human_size(offer.size));

    let accept = if policy.yes {
        true
    } else if policy.auto_accept_verified && status.verified {
        eprintln!("   (auto-accepting: verified contact)");
        true
    } else if policy.auto_accept_contacts && status.name.is_some() {
        eprintln!("   (auto-accepting: saved contact)");
        true
    } else {
        confirm(format!("   Accept from {who}?")).await
    };

    if accept {
        match client.accept(offer.id, None).await {
            Ok(tid) => eprintln!("   ✓ accepted — downloading (transfer {tid})…"),
            Err(e) => eprintln!("   ✗ could not accept: {e:#}"),
        }
    } else {
        match client.reject(offer.id).await {
            Ok(()) => eprintln!("   ✗ rejected"),
            Err(e) => eprintln!("   ✗ could not reject: {e:#}"),
        }
    }
}

#[cfg(unix)]
async fn listen_attached(
    mut client: ipc::client::DaemonClient,
    auto_accept_contacts: bool,
    auto_accept_verified: bool,
    yes: bool,
) -> Result<()> {
    use ipc::protocol::{EventDto, OfferDto};

    let policy = AcceptPolicy {
        auto_accept_contacts,
        auto_accept_verified,
        yes,
    };

    let st = client.status().await?;
    eprintln!("Attached to daemon {}", st.public_id);
    eprintln!("Relay: {}", st.relay.as_deref().unwrap_or("-"));
    eprintln!("Ctrl-C to detach (the daemon keeps receiving).\n");

    // Drain any offers already parked before we attached.
    for o in client.list_pending().await? {
        handle_attached_offer(&mut client, o, policy).await;
    }

    let mut events = daemon_events().await?;
    let cancel = cancel_on_ctrl_c();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            ev = events.next() => {
                match ev {
                    Ok(Some(EventDto::OfferReceived { id, from, name, size })) => {
                        handle_attached_offer(
                            &mut client,
                            OfferDto { id, from, name, size },
                            policy,
                        ).await;
                    }
                    Ok(Some(EventDto::Completed { path: Some(p), .. })) => {
                        eprintln!("✓ saved {p}");
                    }
                    Ok(Some(EventDto::Failed { error, .. })) => {
                        eprintln!("✗ transfer failed: {error}");
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        eprintln!("(daemon closed the connection)");
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(())
}

/// Persist a finished transfer to the history store (best-effort).
fn record_history(manager: &TransferManager, id: u64, status: &str) {
    let Some(t) = manager.get(id) else { return };
    let direction = match t.direction {
        Direction::Send => "send",
        Direction::Recv => "recv",
    };
    let peer_id = t.peer.as_ref().map(encode_id);
    let _ = history::record(
        direction,
        peer_id,
        &t.name,
        t.total_size,
        t.transferred,
        status,
    );
}

/// Run the persistent background engine behind a local control socket.
#[cfg(unix)]
async fn daemon(
    download_dir: Option<PathBuf>,
    relay: Option<String>,
    use_http: bool,
) -> Result<()> {
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    let relay = require_relay(relay, use_http)?;
    let me = my_identity()?;
    let my_id = encode_id(&me.public());
    let download_dir = download_dir
        .or_else(book::default_download_dir)
        .unwrap_or_else(|| book::config_dir().join("downloads"));
    std::fs::create_dir_all(&download_dir).context("create download dir")?;

    let manager = TransferManager::new(me, Some(relay.clone()), download_dir.clone());
    let inbox = manager.spawn_inbox()?;

    // Single-instance guard: if the socket answers, a daemon is already up.
    let sock = ipc::socket_path();
    if let Some(parent) = sock.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    if tokio::net::UnixStream::connect(&sock).await.is_ok() {
        anyhow::bail!(
            "a daemon is already running (socket {} answers)",
            sock.display()
        );
    }
    // Stale socket from a previous crash — bind() would fail on an existing path.
    if sock.exists() {
        std::fs::remove_file(&sock).ok();
    }
    let listener = tokio::net::UnixListener::bind(&sock)
        .with_context(|| format!("bind control socket {}", sock.display()))?;
    // Owner-only: the filesystem permission is the access control.
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).ok();

    // Advisory pidfile for service tooling (not the guard).
    let pidfile = book::config_dir().join("daemon.pid");
    std::fs::write(&pidfile, format!("{}\n", std::process::id())).ok();

    // Offers awaiting the user's approval. In M1 every offer parks here (no trust
    // policy yet); a subscribed front-end lists and accepts/rejects them.
    let pending: Arc<Mutex<HashMap<String, ipc::protocol::OfferDto>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Engine task: park incoming offers and persist finished transfers to history,
    // whether or not a front-end is attached.
    {
        let mut events = manager.subscribe();
        let manager = manager.clone();
        let pending = pending.clone();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(ManagerEvent::OfferReceived {
                        id,
                        from,
                        name,
                        size,
                    }) => {
                        let from_b32 = encode_id(&from);
                        // Supersede any older parked offer for the same (sender, file).
                        // A live→mailbox fallback posts a fresh offer and retracts the
                        // stale live one, but we may already have pulled that stale copy
                        // into the inbox; accepting it would download nothing. Drop the
                        // older one(s) and keep only this newest offer.
                        let superseded: Vec<String> = {
                            let map = pending.lock().unwrap();
                            map.values()
                                .filter(|o| o.from == from_b32 && o.name == name)
                                .map(|o| o.id.clone())
                                .collect()
                        };
                        for old in &superseded {
                            pending.lock().unwrap().remove(old);
                            manager.reject_offer(old).await;
                        }
                        let status = book::sender_status(&from_b32);
                        let who = status.name.clone().unwrap_or_else(|| from_b32.clone());
                        // Trusted sender → auto-download. Everyone else parks and
                        // waits for the user's approval (the default).
                        if status.trusted {
                            let size_h = human_size(size);
                            eprintln!("⬇ auto-downloading {name} ({size_h}) from trusted {who}");
                            // Auto-accept, but still surface a notification so the
                            // user knows a trusted download is happening.
                            notify::auto_downloading(&name, &who, &size_h);
                            if let Err(e) = manager.accept_offer(&id, None).await {
                                eprintln!("   ✗ could not auto-accept: {e:#}");
                            }
                        } else {
                            let size_h = human_size(size);
                            eprintln!(
                                "📨 offer parked: {name} ({size_h}) from {who} — approve with `arvolo accept {id}`"
                            );
                            // Nudge the user with a desktop notification (best-effort;
                            // no-op on headless hosts, where the log line above stands in).
                            notify::offer_awaiting(&name, &who, &size_h);
                            pending.lock().unwrap().insert(
                                id.clone(),
                                ipc::protocol::OfferDto {
                                    id,
                                    from: from_b32,
                                    name,
                                    size,
                                },
                            );
                        }
                    }
                    Ok(ManagerEvent::Completed { id, path }) => {
                        if let Some(p) = &path {
                            eprintln!("✓ saved {}", p.display());
                        }
                        record_history(&manager, id, "completed");
                    }
                    Ok(ManagerEvent::Deposited { id }) => record_history(&manager, id, "deposited"),
                    Ok(ManagerEvent::Failed { id, error }) => {
                        record_history(&manager, id, &format!("failed: {error}"))
                    }
                    Ok(ManagerEvent::Cancelled { id }) => record_history(&manager, id, "cancelled"),
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    eprintln!("arvolo daemon up.");
    eprintln!("  identity: {my_id}");
    eprintln!("  relay:    {relay}");
    eprintln!("  socket:   {}", sock.display());
    eprintln!("  saving:   {}", download_dir.display());

    let shutdown = daemon_shutdown_signal();
    let daemon = ipc::server::Daemon {
        manager,
        relay: Some(relay),
        pending,
    };
    let result = ipc::server::run(daemon, listener, shutdown).await;

    inbox.cancel();
    std::fs::remove_file(&sock).ok();
    std::fs::remove_file(&pidfile).ok();
    result
}

/// A cancellation token that fires on SIGINT (Ctrl-C) or SIGTERM (systemd stop).
#[cfg(unix)]
fn daemon_shutdown_signal() -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match term.as_mut() {
                    Some(s) => { s.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
        t.cancel();
    });
    token
}

/// Connect to a running daemon and confirm it answers, else `None` (so callers
/// fall back to running the engine in-process).
///
/// A daemon is long-lived: after upgrading the binary the old process keeps
/// running the old code, and a mismatched daemon speaks a different IPC dialect
/// — it may not answer newer requests at all, hanging the client. So we gate on
/// version here: same version → use it; different (or a pre-versioning daemon
/// that reports none) → refuse loudly and exit, telling the user to restart it.
#[cfg(unix)]
async fn daemon_client() -> Option<ipc::client::DaemonClient> {
    let mut c = ipc::client::DaemonClient::connect().await.ok()?;
    c.ping().await.ok()?;
    // `status` predates the newer requests, so an old daemon still answers it
    // (with an empty version) — safe to probe without risking a hang.
    if let Ok(st) = c.status().await {
        let ours = env!("CARGO_PKG_VERSION");
        if st.version != ours {
            let theirs = if st.version.is_empty() {
                "unknown (older, pre-versioning)".to_string()
            } else {
                st.version.clone()
            };
            eprintln!(
                "✗ version mismatch: this CLI is {ours}, but the running daemon is {theirs}.\n  \
                 The daemon kept running the old binary after the upgrade. Restart it:\n    \
                 kill $(cat ~/.config/arvolo/daemon.pid)   # stop the stale daemon\n    \
                 arvolo daemon                             # start it on {ours}"
            );
            std::process::exit(1);
        }
    }
    Some(c)
}

/// A fresh subscribed event stream from the daemon.
#[cfg(unix)]
async fn daemon_events() -> Result<ipc::client::EventStream> {
    ipc::client::DaemonClient::connect()
        .await?
        .subscribe()
        .await
}

/// Print a one-line summary of a transfer DTO.
#[cfg(unix)]
fn print_transfer_dto(t: &ipc::protocol::TransferDto) {
    let arrow = if t.direction == "send" { "→" } else { "←" };
    let peer = t
        .peer
        .as_deref()
        .map(|id| book::resolve_name(id).unwrap_or_else(|| id.to_string()))
        .unwrap_or_else(|| "anonymous".into());
    let progress = if t.total_size > 0 {
        format!(
            " {}/{}",
            human_size(t.transferred),
            human_size(t.total_size)
        )
    } else {
        String::new()
    };
    println!(
        "  [{}] {arrow} {peer}  {}{progress}  ({})",
        t.id, t.name, t.status
    );
}

/// `arvolo accept <offer_id>` — approve a parked offer and download it.
#[cfg(unix)]
async fn accept_cmd(offer_id: String, out: Option<PathBuf>) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon`)")?;
    let id = client.accept(offer_id, out).await?;
    eprintln!("✓ accepted — downloading (transfer {id}). Track it with `arvolo transfers`.");
    Ok(())
}

/// `arvolo reject <offer_id>` — decline a parked offer.
#[cfg(unix)]
async fn reject_cmd(offer_id: String) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon`)")?;
    client.reject(offer_id).await?;
    eprintln!("✗ rejected.");
    Ok(())
}

/// `arvolo cancel <id>` — stop a transfer running in the daemon.
#[cfg(unix)]
async fn cancel_cmd(id: u64) -> Result<()> {
    let mut client = daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon`)")?;
    client.cancel(id).await?;
    eprintln!("cancelled transfer {id}.");
    Ok(())
}

/// Hand a plain ticket send to the daemon: it serves in the background. Prints the
/// `arvc…` ticket and returns immediately; the transfer is tracked in the daemon.
#[cfg(unix)]
async fn serve_ticket_via_daemon(
    mut client: ipc::client::DaemonClient,
    paths: Vec<PathBuf>,
    seed_relay: Option<String>,
    qr: bool,
) -> Result<()> {
    // The daemon resolves paths on its own cwd — absolutize relative to ours.
    let paths_s: Vec<String> = paths
        .iter()
        .map(|p| {
            std::fs::canonicalize(p)
                .with_context(|| format!("{}", p.display()))
                .map(|abs| abs.to_string_lossy().into_owned())
        })
        .collect::<Result<Vec<_>>>()
        .context("no such file or folder to serve")?;
    let (id, ticket) = client
        .serve_ticket(paths_s, seed_relay)
        .await
        .context("daemon rejected the serve")?;
    println!("\nServing via the daemon. On the other device:\n");
    println!("    arvolo recv {ticket}\n");
    if qr {
        print_qr(&ticket);
    }
    println!(
        "Tracked as transfer {id} — follow it with `arvolo transfers`, stop it with `arvolo cancel {id}`."
    );
    Ok(())
}

/// Submit a push to the running daemon and render its progress. Ctrl-C detaches
/// (the daemon keeps sending); it does not cancel.
#[cfg(unix)]
async fn push_via_daemon(
    mut client: ipc::client::DaemonClient,
    paths: Vec<PathBuf>,
    to: String,
) -> Result<()> {
    use ipc::protocol::EventDto;

    // The daemon resolves paths on *its own* cwd (e.g. `/` under systemd), not
    // ours — so absolutize here, relative to the client's cwd, and validate the
    // files exist now with a clear error instead of a confusing daemon-side one.
    let paths_s: Vec<String> = paths
        .iter()
        .map(|p| {
            std::fs::canonicalize(p)
                .with_context(|| format!("{}", p.display()))
                .map(|abs| abs.to_string_lossy().into_owned())
        })
        .collect::<Result<Vec<_>>>()
        .context("no such file or folder to push")?;
    // Subscribe before submitting so an early terminal event isn't missed.
    let mut events = daemon_events().await?;
    eprintln!("Handing off to the daemon (sending to {to})…");
    let id = client
        .push(to, paths_s)
        .await
        .context("daemon rejected the push")?;
    eprintln!("queued as transfer {id}. (Ctrl-C detaches; the daemon keeps sending.)\n");

    let cancel = cancel_on_ctrl_c();
    let mut last_pct = u64::MAX;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                eprintln!("\n(detached — transfer {id} continues in the daemon; `arvolo cancel {id}` to stop it)");
                break;
            }
            ev = events.next() => {
                match ev {
                    Ok(Some(EventDto::Progress { id: eid, transferred, total_size })) if eid == id && total_size > 0 => {
                        let pct = transferred * 100 / total_size;
                        if pct != last_pct {
                            last_pct = pct;
                            use std::io::Write;
                            eprint!("\r  {pct}% ({}/{})   ", human_size(transferred), human_size(total_size));
                            let _ = std::io::stderr().flush();
                        }
                    }
                    Ok(Some(EventDto::Completed { id: eid, .. })) if eid == id => {
                        eprintln!("\n✓ delivered.");
                        break;
                    }
                    Ok(Some(EventDto::Deposited { id: eid })) if eid == id => {
                        eprintln!("✓ deposited to the mailbox (delivered when they return).");
                        break;
                    }
                    Ok(Some(EventDto::Failed { id: eid, error })) if eid == id => {
                        eprintln!("\n✗ failed: {error}");
                        break;
                    }
                    Ok(Some(EventDto::Cancelled { id: eid })) if eid == id => {
                        eprintln!("\n(cancelled)");
                        break;
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        eprintln!("\n(daemon closed the connection)");
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(())
}

async fn push(
    paths: Vec<PathBuf>,
    to: String,
    relay: Option<String>,
    use_http: bool,
) -> Result<()> {
    anyhow::ensure!(
        !paths.is_empty(),
        "provide at least one file or folder to push"
    );

    // If a daemon is running, hand the send off to it (concurrent, survives our
    // exit); otherwise fall back to a one-shot in-process send.
    #[cfg(unix)]
    {
        if let Some(client) = daemon_client().await {
            return push_via_daemon(client, paths, to).await;
        }
    }

    let relay = require_relay(relay, use_http)?;
    let recipient = book::resolve_recipient(&to)?;
    vprintln!(
        "recipient {to} resolved (fingerprint {})",
        recipient.fingerprint()
    );
    let me = my_identity()?;
    let (payload, name, archive, temp) = resolve_payload(&paths)?;
    if archive {
        eprintln!("Packing {} item(s) into an archive…", paths.len());
    }
    vprintln!(
        "payload: {} ({}){}",
        name,
        human_size(std::fs::metadata(&payload).map(|m| m.len()).unwrap_or(0)),
        if archive { ", packed archive" } else { "" }
    );

    let manager = TransferManager::new(me, Some(relay.clone()), PathBuf::from("."));
    let mut events = manager.subscribe();

    // `send_to` decides live-vs-mailbox itself (with a presence grace window and a
    // watchdog); the up-front check is only a hint for the opening line.
    vprintln!("checking {to}'s presence on the relay…");
    if manager.is_online(&recipient).await {
        eprintln!("{to} looks online — trying a direct transfer…");
    } else {
        eprintln!("{to} looks offline — will deposit to the mailbox…");
    }
    eprintln!("Sending offer to {to}… (Ctrl-C to abort)\n");
    let id = manager
        .send_to(&recipient, payload, name.clone(), archive)
        .await
        .inspect_err(|_| {
            if let Some(t) = &temp {
                let _ = std::fs::remove_file(t);
            }
        })?;

    let cancel = cancel_on_ctrl_c();
    let mut last_pct = u64::MAX;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                manager.cancel(id);
                break;
            }
            ev = events.recv() => {
                match ev {
                    Ok(ManagerEvent::Progress { id: eid, transferred, total_size }) if eid == id && total_size > 0 => {
                        let pct = transferred * 100 / total_size;
                        if pct != last_pct {
                            last_pct = pct;
                            eprint!("\r  {pct}% ({}/{})   ", human_size(transferred), human_size(total_size));
                            use std::io::Write;
                            let _ = std::io::stderr().flush();
                        }
                    }
                    Ok(ManagerEvent::Completed { id: eid, .. }) if eid == id => {
                        // The offline path emits Deposited first (handled below); a
                        // Completed here is a live P2P delivery.
                        eprintln!("\n✓ delivered.");
                        record_history(&manager, id, "completed");
                        break;
                    }
                    Ok(ManagerEvent::Deposited { id: eid }) if eid == id => {
                        eprintln!("✓ deposited to the mailbox (delivered when they return).");
                        record_history(&manager, id, "deposited");
                        break;
                    }
                    Ok(ManagerEvent::Failed { id: eid, error }) if eid == id => {
                        eprintln!("\n✗ failed: {error}");
                        record_history(&manager, id, &format!("failed: {error}"));
                        break;
                    }
                    Ok(ManagerEvent::Cancelled { id: eid }) if eid == id => {
                        record_history(&manager, id, "cancelled");
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }
    }
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn send(
    paths: Vec<PathBuf>,
    resume: Option<String>,
    seed_relay: Option<String>,
    code_mode: bool,
    relay: Option<String>,
    use_http: bool,
    to: Option<String>,
    ticket_mode: bool,
    link: bool,
    ttl: u64,
    max: Option<u32>,
    password: Option<String>,
    foreground: bool,
    qr: bool,
) -> Result<()> {
    // Resume short-circuits the normal flow: re-serve a previous send so the
    // ticket you already handed out stays valid after the sender restarted.
    // Recovery is pure P2P and independent of --seed-relay. The argument is either
    // a plain `arvc…` ticket (re-serve, needs the file) or a saved session id.
    if let Some(arg) = resume {
        anyhow::ensure!(
            !code_mode && to.is_none() && !link && !ticket_mode,
            "--resume replays a saved P2P send (no --to/--code/--link/--ticket)"
        );
        if ChunkTicket::looks_like(&arg) {
            anyhow::ensure!(
                paths.len() == 1,
                "resuming from an `arvc…` ticket needs exactly one path: the file to re-serve"
            );
            return resume_by_ticket(&arg, &paths[0], qr).await;
        }
        anyhow::ensure!(
            paths.is_empty(),
            "resuming a session by id takes no paths (the saved session remembers the file)"
        );
        return resume_by_id(&arg, qr).await;
    }

    anyhow::ensure!(
        !paths.is_empty(),
        "provide at least one file or folder to send (or use --resume)"
    );

    // --link: a public, browser-openable download URL (no recipient).
    if link {
        anyhow::ensure!(to.is_none(), "--link makes a public link; drop --to");
        anyhow::ensure!(
            !code_mode && !ticket_mode,
            "--link can't be combined with --code / --ticket"
        );
        anyhow::ensure!(
            seed_relay.is_none(),
            "--seed-relay applies to a P2P ticket send, not --link"
        );
        return send_offline(
            paths, None, true, relay, use_http, ttl, max, password, qr, false,
        )
        .await;
    }

    // --to: deliver to a known recipient. Online (a live daemon) → delivered live
    // (push); offline or --ticket → deposited on the mailbox + an inbox offer, and
    // an `arvm…` ticket is printed so you can also hand it over.
    if let Some(to) = to {
        anyhow::ensure!(
            !code_mode,
            "--code makes a shareable P2P ticket; it doesn't apply with --to"
        );
        anyhow::ensure!(
            seed_relay.is_none(),
            "--seed-relay applies to a P2P ticket send (no --to)"
        );
        let relay_url = require_relay(relay, use_http)?;
        let recipient = book::resolve_recipient(&to)?;
        let online = if ticket_mode {
            false
        } else {
            vprintln!("checking {to}'s presence on the relay…");
            arvolo_core::presence::check_online(&reqwest::Client::new(), &relay_url, &recipient)
                .await
                .unwrap_or(false)
        };
        if online {
            if max.is_some() || password.is_some() {
                eprintln!(
                    "note: --max/--password apply to a mailbox send; ignored for a live delivery."
                );
            }
            return push(paths, to, Some(relay_url), use_http).await;
        }
        return send_offline(
            paths,
            Some(to),
            false,
            Some(relay_url),
            use_http,
            ttl,
            max,
            password,
            qr,
            true,
        )
        .await;
    }

    // No --to: a shareable P2P ticket (arvc…) or pairing code. Mailbox-only flags
    // don't apply here.
    anyhow::ensure!(
        max.is_none() && password.is_none(),
        "--max/--password apply to a mailbox send or --link (use --to or --link)"
    );

    // A bare relay host gets a scheme (https by default, http with --use-http).
    let seed_relay = seed_relay.map(|r| book::normalize_relay(&r, use_http));
    let relay = relay.map(|r| book::normalize_relay(&r, use_http));
    if let Some(r) = &seed_relay {
        vprintln!("seed relay (backfill): {r}");
    }

    // By default hand a plain ticket send to a running daemon: it serves in the
    // background, observable via `arvolo transfers` and surviving this terminal.
    // `--foreground` (or `--code`) keep it inline in this process.
    #[cfg(unix)]
    {
        if !code_mode && !foreground {
            if let Some(client) = daemon_client().await {
                return serve_ticket_via_daemon(client, paths, seed_relay, qr).await;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = foreground;

    let (payload, name, archive, temp) = resolve_payload(&paths)?;
    if archive {
        eprintln!("Packing {} item(s) into an archive…", paths.len());
    }
    vprintln!(
        "payload: {} ({}){}",
        name,
        human_size(std::fs::metadata(&payload).map(|m| m.len()).unwrap_or(0)),
        if archive { ", packed archive" } else { "" }
    );
    vprintln!("plain ticket: the ticket itself is the capability (anonymous, unauthenticated)");

    if code_mode {
        return send_with_code(payload, name, archive, temp, relay, None, qr).await;
    }
    eprintln!("Splitting and serving chunks…");
    let session = flow::prepare_send(
        &payload,
        &name,
        archive,
        None,
        seed_relay,
        RelayChoice::from_env(),
    )
    .await?;
    // Persist a resumable session (best-effort: a save failure must not abort the
    // send). The content key is stored so an interrupted send can be recovered —
    // see `arvolo sessions` and `send --resume`.
    if let Err(e) = sessions::save(
        session.content_key(),
        session.node_seed(),
        &paths,
        &name,
        archive,
        session.total_size,
        session.chunks,
        &session.ticket,
    ) {
        eprintln!("(note: could not save resumable session: {e:#})");
    }
    let result = serve_session(session, qr).await;
    // The payload (a packed archive) had to stay readable for the whole session,
    // since chunks are produced on the fly; clean it up now that serving ended.
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    result
}

/// Drive a prepared/resumed [`flow::SendSession`] to completion, printing the
/// ticket when it's ready. Shared by fresh sends and both resume paths.
async fn serve_session(session: flow::SendSession, qr: bool) -> Result<()> {
    let cancel = cancel_on_ctrl_c();
    // Last progress percent we narrated, so `-v` shows a few milestones instead
    // of one line per ack. Shared because `serve`'s callback is `Fn`, not `FnMut`.
    let last_pct = Arc::new(AtomicU8::new(u8::MAX));
    session
        .serve(cancel, move |ev| match ev {
            SendEvent::Ready {
                chunks,
                total_size,
                ticket,
                has_relay,
            } => {
                println!("\nFile ready ({chunks} chunks). On the other device:\n");
                println!("    arvolo recv {ticket}\n");
                if qr {
                    print_qr(&ticket);
                }
                if has_relay {
                    println!("P2P-first; if the receiver drops, only the missing chunks are backfilled to the relay.");
                }
                vprintln!(
                    "serving {chunks} chunk(s), {} total; waiting for a receiver to connect",
                    human_size(total_size)
                );
                println!("Ctrl-C to stop.");
            }
            SendEvent::ReceiverConnected => vprintln!("receiver connected — chunk pull started"),
            SendEvent::Progress { transferred, total } if total > 0 => {
                let pct = (transferred * 100 / total) as u8;
                // Narrate every ~10% step, once each.
                if verbosity() >= 1 && pct / 10 != last_pct.load(Ordering::Relaxed) / 10 {
                    last_pct.store(pct, Ordering::Relaxed);
                    vprintln!(
                        "acked {pct}% ({}/{})",
                        human_size(transferred),
                        human_size(total)
                    );
                }
            }
            SendEvent::Progress { .. } => {}
            SendEvent::Delivered => eprintln!("✓ A receiver got the whole file."),
            SendEvent::ReceiverDropped { missing } => {
                eprintln!("Receiver dropped — backfilling {missing} missing chunks to the relay…")
            }
            SendEvent::Backfilled => {
                eprintln!("Backfilled. You can close this; the relay can finish the delivery.")
            }
            SendEvent::BackfillFailed { reason } => eprintln!("Relay backfill failed: {reason}"),
        })
        .await
}

/// `send --resume <arvc…> <file>`: re-serve `path` under an existing *plain*
/// ticket so it stays valid after the sender restarted. The key rides in the
/// ticket, so no saved session is needed. Sealed (`--to`) tickets resume by
/// session id instead (`send --resume <id>`).
async fn resume_by_ticket(ticket: &str, path: &Path, qr: bool) -> Result<()> {
    let expected = ChunkTicket::decode(ticket).context("parse ticket")?;
    let key: [u8; 32] = match &expected.key {
        KeyDelivery::Plain(bytes) => bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("ticket content key has the wrong size"))?,
        KeyDelivery::Sealed { .. } => anyhow::bail!(
            "this ticket is sealed to a recipient (--to), so its key isn't in the ticket — \
             resume it with `arvolo send --resume <id>` (see `arvolo sessions list`)"
        ),
    };
    // A plain ticket carries no transport secret, so we can't rebind the original
    // node id — the sender comes back under a fresh id and the receiver must use
    // the reprinted ticket (their partial download still resumes: same chunks).
    eprintln!(
        "Resuming send — re-serving the file. NOTE: use the reprinted ticket below on the receiver \
         (the original ticket's address is stale after a restart). For full old-ticket recovery, \
         start sends normally and resume with `arvolo send --resume <id>`."
    );
    let session = flow::resume_send(path, key, None, &expected, RelayChoice::from_env()).await?;
    serve_session(session, qr).await
}

/// `send --resume <id>`: replay a saved session (covers `--to` sends too). A
/// single file is re-served in place; an archive is repacked deterministically.
async fn resume_by_id(id: &str, qr: bool) -> Result<()> {
    let rec = sessions::load(id)?;
    let expected = ChunkTicket::decode(&rec.ticket).context("parse saved ticket")?;
    let key = rec.key()?;
    let node_seed = rec.node_seed()?;
    for s in &rec.sources {
        anyhow::ensure!(
            s.exists(),
            "source is gone, cannot resume session '{id}': {}",
            s.display()
        );
    }
    eprintln!(
        "Resuming session {id} ({}) — re-serving under the saved ticket…",
        rec.name
    );
    // For an archive, materialize the deterministic tar once and serve from it;
    // for a single file, serve it directly.
    let temp = if rec.archive {
        let t = std::env::temp_dir().join(format!("arvolo-resume-{}.tar", std::process::id()));
        flow::pack_tar(&rec.sources, &t).context("repack archive for resume")?;
        Some(t)
    } else {
        None
    };
    let payload = temp.clone().unwrap_or_else(|| rec.sources[0].clone());
    let session = flow::resume_send(
        &payload,
        key,
        Some(node_seed),
        &expected,
        RelayChoice::from_env(),
    )
    .await?;
    let result = serve_session(session, qr).await;
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    result
}

/// `send --code`: hand the ticket to the receiver via a short pairing code over a
/// relay rendezvous, serving the file (and using the relay for backfill) meanwhile.
async fn send_with_code(
    payload: PathBuf,
    name: String,
    archive: bool,
    temp: Option<PathBuf>,
    relay: Option<String>,
    to: Option<(&Identity, &PublicId)>,
    qr: bool,
) -> Result<()> {
    // An explicit --relay is embedded in the code (works with no receiver config);
    // otherwise fall back to the default relay (short code, shared default).
    let (relay_url, embed) = match relay {
        Some(r) => (r, true),
        None => match book::default_relay() {
            Some(r) => (r, false),
            None => anyhow::bail!("--code needs a relay: pass --relay <host>, set ARVOLO_RELAY, or configure `relay` in config.toml"),
        },
    };

    vprintln!(
        "rendezvous relay: {relay_url} ({})",
        if embed {
            "embedded in the code — receiver needs no config"
        } else {
            "shared default — not embedded"
        }
    );
    eprintln!("Splitting and serving chunks…");
    let session = flow::prepare_send(
        &payload,
        &name,
        archive,
        to,
        Some(relay_url.clone()),
        RelayChoice::from_env(),
    )
    .await?;
    vprintln!(
        "serving {} chunk(s), {}; publishing the encrypted ticket under the pairing code…",
        session.chunks,
        human_size(session.total_size)
    );
    let (shown_code, complete) = code::publish_ticket(&session.ticket, &relay_url, embed)
        .await
        .context("start pairing")?;

    println!("\nOn the other device:\n");
    println!("    arvolo recv {shown_code}\n");
    if qr {
        print_qr(&shown_code);
    }
    println!("Ctrl-C to stop.");

    let cancel = cancel_on_ctrl_c();
    // Finish the pairing (publish the encrypted ticket once the receiver shows up)
    // in the background while we serve.
    let pairing = tokio::spawn(async move {
        if let Err(e) = complete.run().await {
            eprintln!("Pairing failed: {e:#}");
        }
    });
    let result = session
        .serve(cancel, |ev| match ev {
            SendEvent::ReceiverDropped { missing } => {
                eprintln!("Receiver dropped — backfilling {missing} missing chunks to the relay…")
            }
            SendEvent::Backfilled => {
                eprintln!("Backfilled. You can close this; the relay can finish the delivery.")
            }
            SendEvent::BackfillFailed { reason } => eprintln!("Relay backfill failed: {reason}"),
            SendEvent::Delivered => eprintln!("✓ A receiver got the whole file."),
            SendEvent::ReceiverConnected => vprintln!("receiver connected — chunk pull started"),
            SendEvent::Progress { .. } => {}
            SendEvent::Ready { .. } => {} // code already printed
        })
        .await;
    pairing.abort();
    // The packed archive had to stay readable for the whole session (chunks are
    // produced on the fly); clean it up now.
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    result
}

async fn recv(ticket: String, out: Option<PathBuf>, password: Option<String>) -> Result<()> {
    // An offline/mailbox ticket (arvm…) is fetched + decrypted from the relay;
    // pairing codes and P2P tickets (arvc…) take the live chunked route. Detect
    // by trying to decode it as an offline ticket (codes never do).
    if !code::looks_like_code(&ticket)
        && arvolo_core::offline::OfflineTicket::decode(&ticket).is_ok()
    {
        return recv_offline(ticket, out, password).await;
    }
    if password.is_some() {
        eprintln!("note: --password applies only to an offline (arvm…) ticket — ignoring it here.");
    }

    // A short pairing code is resolved to the real ticket over a rendezvous first.
    let ticket = if code::looks_like_code(&ticket) {
        eprintln!("Pairing… (waiting for the sender)");
        let default_relay = book::default_relay();
        vprintln!(
            "input looks like a pairing code — resolving to a ticket over rendezvous relay {}",
            default_relay.as_deref().unwrap_or("(embedded in code)")
        );
        code::resolve_code(&ticket, default_relay.as_deref())
            .await
            .context("pairing")?
    } else {
        vprintln!(
            "input is a full ticket — connecting to the sender P2P (relay-assisted if needed)"
        );
        ticket
    };
    // Our identity is needed to open a ticket sealed to us (--to); harmless
    // otherwise (created on first use).
    let me = my_identity()?;

    let cancel = cancel_on_ctrl_c();
    let tty = std::io::stderr().is_terminal();
    let bar: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
    let b = bar.clone();
    // Last chunk source we narrated at -vv (0 = none, 1 = sender, 2 = relay), so
    // we log only when delivery flips between P2P and the relay, not every chunk.
    let last_src = Arc::new(AtomicU8::new(0));
    flow::recv_chunked(
        &ticket,
        out,
        Some(&me),
        RelayChoice::from_env(),
        cancel,
        move |ev| {
            let mut slot = b.lock().unwrap();
            match ev {
                RecvEvent::Sender { id } => {
                    print_sender_banner(id.as_deref());
                }
                RecvEvent::Started {
                    total,
                    resuming_from,
                    total_size,
                    resumed_bytes,
                } => {
                    let head = if resuming_from > 0 {
                        format!("resuming from chunk {resuming_from}/{total}")
                    } else {
                        format!("fetching {total} chunks")
                    };
                    if tty {
                        let pb = ProgressBar::new(total_size);
                        pb.set_style(
                        ProgressStyle::with_template(
                            "{spinner} {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta}) {msg}",
                        )
                        .unwrap(),
                    );
                        pb.set_position(resumed_bytes);
                        pb.set_message(head);
                        pb.enable_steady_tick(Duration::from_millis(120));
                        *slot = Some(pb);
                    } else {
                        eprintln!("{head}…");
                    }
                }
                RecvEvent::Control { connected } => {
                    let msg = format!(
                        "control channel to sender: {}",
                        if connected {
                            "connected"
                        } else {
                            "unavailable"
                        }
                    );
                    match slot.as_ref() {
                        Some(pb) => pb.println(msg),
                        None => eprintln!("{msg}"),
                    }
                }
                RecvEvent::Chunk {
                    index,
                    total,
                    source,
                    bytes,
                } => {
                    let src = match source {
                        ChunkSource::Relay => "relay",
                        ChunkSource::Sender => "sender",
                    };
                    // At -vv, announce only when the source flips (P2P ↔ relay).
                    let code = if matches!(source, ChunkSource::Relay) {
                        2
                    } else {
                        1
                    };
                    if last_src.swap(code, Ordering::Relaxed) != code {
                        let line =
                            format!("chunk {}/{total}: now pulling from the {src}", index + 1);
                        match slot.as_ref() {
                            Some(pb) if verbosity() >= 2 => pb.println(format!("·· {line}")),
                            _ => vvprintln!("{line}"),
                        }
                    }
                    if let Some(pb) = slot.as_ref() {
                        pb.inc(bytes);
                        pb.set_message(format!("chunk {}/{total} from {src}", index + 1));
                    }
                }
                RecvEvent::Warning { message } => match slot.as_ref() {
                    Some(pb) => pb.println(message),
                    None => eprintln!("{message}"),
                },
                RecvEvent::Saved { path } => {
                    if let Some(pb) = slot.take() {
                        pb.finish_and_clear();
                    }
                    println!("Saved to {}", path.display());
                }
            }
        },
    )
    .await?;
    Ok(())
}

// ---- offline mailbox ------------------------------------------------------

/// Deposit `paths` on the relay mailbox (or as a `--link`). Internal helper for
/// the unified `send`: `link` → public browser URL; otherwise HPKE-sealed to
/// `to`, and if `offer` is set an inbox offer is posted too so the recipient's
/// daemon can auto-fetch it (a shareable `arvm…` ticket is printed either way).
#[allow(clippy::too_many_arguments)]
async fn send_offline(
    paths: Vec<PathBuf>,
    to: Option<String>,
    link: bool,
    relay: Option<String>,
    use_http: bool,
    ttl: u64,
    max: Option<u32>,
    password: Option<String>,
    qr: bool,
    offer: bool,
) -> Result<()> {
    anyhow::ensure!(
        !paths.is_empty(),
        "provide at least one file or folder to send"
    );
    let me = my_identity()?;
    let relay = relay
        .map(|r| book::normalize_relay(&r, use_http))
        .or_else(book::default_relay)
        .context("no relay: pass --relay <host>, set ARVOLO_RELAY, or configure `relay`")?;
    vprintln!("using relay: {relay}");
    let (payload, name, archive, temp) = resolve_payload(&paths)?;
    if archive {
        eprintln!("Packing {} item(s) into an archive…", paths.len());
    }
    let size = std::fs::metadata(&payload).map(|m| m.len()).unwrap_or(0);
    vprintln!(
        "mode: {} — TTL {}",
        if link {
            "public browser link (--link)"
        } else {
            "sealed to recipient (--to)"
        },
        human_duration(ttl)
    );

    // Link mode: a public, browser-openable download URL (no recipient). A link
    // has NO download cap by default (it expires only when its session is
    // removed or the TTL lapses); `--max` optionally sets one.
    if link {
        anyhow::ensure!(
            to.is_none(),
            "--link produces a public link; --to is not used (anyone with the link can download)"
        );
        anyhow::ensure!(
            password.is_none(),
            "--password is not yet supported with --link (the browser page can't unwrap it)"
        );
        let max = max.unwrap_or(deposits::UNLIMITED);
        vprintln!("encrypting locally and uploading to the relay (key stays in the link's #fragment; the relay only sees ciphertext)…");
        let out = match arvolo_core::link::deposit_link(&payload, &relay, ttl, max).await {
            Ok(o) => o,
            Err(e) => {
                if let Some(t) = &temp {
                    let _ = std::fs::remove_file(t);
                }
                return Err(e);
            }
        };
        if let Some(t) = &temp {
            let _ = std::fs::remove_file(t);
        }
        let rec = deposits::save(
            deposits::KIND_LINK,
            &relay,
            &out.claim,
            &out.revoke_token,
            &out.name,
            out.size,
            max,
            Some(out.link.clone()),
            None,
            ttl,
        )?;
        let cap = if max == deposits::UNLIMITED {
            "no download limit".to_string()
        } else {
            format!("{max} download(s)")
        };
        println!(
            "\nEncrypted and deposited ({}, expires in {}). File: {} ({}).",
            cap,
            human_duration(ttl),
            out.name,
            human_size(out.size),
        );
        println!("Anyone with this link can download it in a browser — no arvolo needed:\n");
        println!("    {}\n", out.link);
        println!(
            "Session '{}' saved — cancel the link (and delete it from the relay) with:\n",
            rec.id
        );
        println!("    arvolo sessions rm {}\n", rec.id);
        if qr {
            print_qr(&out.link);
        }
        return Ok(());
    }

    let to = to.context("--to <name|id> is required (or pass --link for a public link)")?;
    let max = max.unwrap_or(1);
    let recipient = book::resolve_recipient(&to)?;
    vprintln!(
        "recipient {to} resolved (fingerprint {})",
        recipient.fingerprint()
    );
    vprintln!(
        "HPKE-sealing to the recipient and depositing on the relay ({max} download(s){})…",
        if password.is_some() {
            ", password-protected"
        } else {
            ""
        }
    );
    let deposited = match flow::deposit_offline(
        &payload,
        &recipient,
        &me,
        &relay,
        ttl,
        max,
        password.as_deref(),
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            if let Some(t) = &temp {
                let _ = std::fs::remove_file(t);
            }
            return Err(e);
        }
    };
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    let encoded = deposited.ticket.encode();

    // Also drop an inbox offer so the recipient's daemon can auto-fetch it (the
    // offer carries this same arvm ticket; best-effort — the printed ticket still
    // works if this fails).
    if offer {
        let off = arvolo_core::presence::Offer {
            name: name.clone(),
            size,
            chunks: 0,
            ticket: encoded.clone(),
        };
        if let Err(e) = arvolo_core::presence::post_offer(
            &reqwest::Client::new(),
            &relay,
            &recipient,
            &me,
            &off,
            Some(ttl),
        )
        .await
        {
            eprintln!(
                "(warning: couldn't post an inbox offer, so the recipient's daemon won't auto-fetch: {e:#})"
            );
        }
    }

    let rec = deposits::save(
        deposits::KIND_OFFLINE,
        &relay,
        &deposited.ticket.claim,
        &deposited.revoke_token,
        &name,
        size,
        max,
        None,
        Some(to.clone()),
        ttl,
    )?;
    println!(
        "\nEncrypted and deposited ({max} download(s), expires in {}).",
        human_duration(ttl)
    );
    if password.is_some() {
        println!("Password-protected — share the password out-of-band (not with the ticket).");
    }
    if offer {
        println!("The recipient's daemon will fetch it automatically. To hand it over instead:\n");
    } else {
        println!("Send this ticket to the recipient:\n");
    }
    println!("    arvolo recv {encoded}\n");
    println!(
        "Session '{}' saved — cancel the delivery (and delete it from the relay) with:\n",
        rec.id
    );
    println!("    arvolo sessions rm {}\n", rec.id);
    if qr {
        print_qr(&encoded);
    }
    Ok(())
}

async fn recv_offline(
    ticket: String,
    out: Option<PathBuf>,
    password: Option<String>,
) -> Result<()> {
    let me = my_identity()?;
    if let Ok(t) = arvolo_core::offline::OfflineTicket::decode(&ticket) {
        vprintln!(
            "fetching ciphertext from relay {} and unsealing with your identity…",
            t.relay
        );
    }
    if password.is_some() {
        vprintln!("deriving the decryption key from the supplied password");
    }
    // A successful fetch means HPKE auth passed, so the sender in the ticket is
    // genuine — surface it (offline tickets are always sealed to a recipient).
    let (path, n) = flow::fetch_offline(&ticket, out, &me, password.as_deref()).await?;
    vprintln!("HPKE authentication passed — the sender in the ticket is genuine");
    if let Ok(t) = arvolo_core::offline::OfflineTicket::decode(&ticket) {
        print_sender_banner(Some(&t.sender));
    }
    println!("Saved {n} bytes to {}", path.display());
    Ok(())
}

/// `arvolo revoke <arvm…|link> --token` — delete a mailbox blob or a browser link
/// from the relay. Auto-detects the target: an `arvm…` offline ticket or a
/// `…/dl/<claim>` download link.
async fn revoke(target: String, token: String) -> Result<()> {
    if let Ok(t) = arvolo_core::offline::OfflineTicket::decode(&target) {
        vprintln!("asking relay {} to delete claim {}…", t.relay, t.claim);
        flow::revoke_offline(&t.relay, &t.claim, &token).await?;
        println!("Revoked — the blob is no longer available on the relay.");
        return Ok(());
    }
    if let Ok((relay, claim)) = parse_dl_link(&target) {
        vprintln!("asking relay {relay} to delete claim {claim}…");
        flow::revoke_offline(&relay, &claim, &token).await?;
        println!("Link revoked — the file is deleted from the relay and the link no longer works.");
        return Ok(());
    }
    anyhow::bail!("not an arvolo offline ticket (arvm…) or a download link (…/dl/<claim>)")
}

/// Parse a download link (`https://<relay>/dl/<claim>[#key]`) into its relay base
/// URL and the claim. The `#fragment` (the key) is ignored — revoking needs only
/// the relay and claim.
fn parse_dl_link(link: &str) -> Result<(String, String)> {
    let no_frag = link.split('#').next().unwrap_or(link);
    let (relay, claim) = no_frag
        .rsplit_once("/dl/")
        .context("not an arvolo download link (expected …/dl/<claim>)")?;
    let claim = claim.trim_matches('/');
    anyhow::ensure!(!claim.is_empty(), "download link is missing its claim");
    Ok((relay.trim_end_matches('/').to_string(), claim.to_string()))
}
