//! arvolo CLI (`lss`).
//!
//! P2P (both online):
//!   arvolo send <file>            serve a file; prints a ticket
//!   arvolo recv <ticket>          fetch a file from a ticket
//!
//! Offline mailbox (recipient away — store-and-forward via a relay):
//!   arvolo id                     show your public id
//!   arvolo send-offline <file> --to <id> --relay <url>
//!   arvolo recv-offline <ticket>
//!
//! P2P transport is encrypted by QUIC and each chunk is end-to-end encrypted;
//! the offline path is end-to-end encrypted with HPKE. The relay only ever sees
//! ciphertext. All transfer orchestration lives in `arvolo_core::flow`; this CLI
//! just drives it and renders progress.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
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
mod history;
mod sessions;

#[derive(Parser)]
#[command(
    name = "arvolo",
    version,
    about = "arvolo — secure cross-platform file sending"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve one or more files/folders P2P; prints a ticket (or short code).
    /// Multiple paths or a folder are packed into one archive automatically.
    Send {
        #[arg(num_args = 0..)]
        paths: Vec<PathBuf>,
        /// Resume a send interrupted by the sender restarting: re-serve <path>
        /// under the ticket you already handed out, so it stays valid. Works for
        /// plain (non---to) tickets, which carry their own key. Needs the file.
        #[arg(long, value_name = "TICKET")]
        resume_ticket: Option<String>,
        /// Resume a saved send by its session id (see `arvolo sessions list`).
        /// Recovers `--to` sends too; no need to re-supply the file or ticket.
        #[arg(long, value_name = "ID")]
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
        /// Encrypt so only this recipient can receive (a saved contact name or a
        /// public id). Authenticates you as the sender.
        #[arg(long)]
        to: Option<String>,
        /// Also render the ticket/code as a scannable QR code.
        #[arg(long)]
        qr: bool,
    },
    /// Fetch a file from a chunked ticket (`arvc…`) or a pairing code
    /// (`N-word-word[@relay]`); resumes if interrupted.
    Recv {
        ticket: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
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
    /// View or clear the history of past transfers.
    Transfers {
        #[command(subcommand)]
        action: TransferAction,
    },
    /// Show your public id (creates an identity on first use).
    Id,
    /// Encrypt a file for a recipient and deposit it on a relay (offline send).
    SendOffline {
        path: PathBuf,
        /// Recipient: a saved contact name or a public id (from their `arvolo id`).
        #[arg(long)]
        to: String,
        /// Relay host or URL, e.g. relay.example.com (https assumed; pass
        /// --use-http for plaintext). Defaults to ARVOLO_RELAY / config `relay`.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`
        /// (LAN / dev / plaintext relays). Explicit schemes are always kept.
        #[arg(long)]
        use_http: bool,
        /// Time-to-live in seconds (default 7 days).
        #[arg(long, default_value_t = 7 * 24 * 3600)]
        ttl: u64,
        /// Max downloads before deletion (default 1 = burn-after-read).
        #[arg(long, default_value_t = 1)]
        max: u32,
        /// Protect the link with a password (E2E — required to decrypt, even by
        /// the intended recipient). Share it out-of-band, not with the ticket.
        #[arg(long)]
        password: Option<String>,
        /// Also render the ticket as a scannable QR code.
        #[arg(long)]
        qr: bool,
    },
    /// Fetch and decrypt an offline ticket (`arvm…`).
    RecvOffline {
        ticket: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Password for a password-protected link.
        #[arg(long)]
        password: Option<String>,
    },
    /// Revoke a previously sent offline ticket, deleting it from the relay.
    Revoke {
        /// The offline ticket (`arvm…`) you sent.
        ticket: String,
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
    /// Push one or more files/folders to an online contact: they get a popup and
    /// the transfer starts on accept — no ticket to copy. Needs a relay.
    Push {
        #[arg(num_args = 1..)]
        paths: Vec<PathBuf>,
        /// Recipient: a saved contact name or a public id (from their `arvolo id`).
        #[arg(long)]
        to: String,
        /// Relay host or URL (https assumed; pass --use-http for plaintext).
        /// Defaults to ARVOLO_RELAY / config `relay`.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`.
        #[arg(long)]
        use_http: bool,
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
}

#[derive(Subcommand)]
enum TransferAction {
    /// List past transfers (most recent first).
    List,
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    match Cli::parse().command {
        Command::Send {
            paths,
            resume_ticket,
            resume,
            seed_relay,
            code,
            relay,
            use_http,
            to,
            qr,
        } => {
            send(
                paths,
                resume_ticket,
                resume,
                seed_relay,
                code,
                relay,
                use_http,
                to,
                qr,
            )
            .await
        }
        Command::Recv { ticket, out } => recv(ticket, out).await,
        Command::Id => id(),
        Command::Contacts { action } => contacts_cmd(action).await,
        Command::Sessions { action } => sessions_cmd(action),
        Command::Transfers { action } => transfers_cmd(action),
        Command::SendOffline {
            path,
            to,
            relay,
            use_http,
            ttl,
            max,
            password,
            qr,
        } => send_offline(path, to, relay, use_http, ttl, max, password, qr).await,
        Command::RecvOffline {
            ticket,
            out,
            password,
        } => recv_offline(ticket, out, password).await,
        Command::Revoke { ticket, token } => revoke(ticket, token).await,
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
        Command::Push {
            paths,
            to,
            relay,
            use_http,
        } => push(paths, to, relay, use_http).await,
    }
}

fn transfers_cmd(action: TransferAction) -> Result<()> {
    match action {
        TransferAction::List => {
            let list = history::list();
            if list.is_empty() {
                eprintln!("(no transfers yet)");
                return Ok(());
            }
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
                    "{arrow} {peer}\t{}\t{}\t{}",
                    rec.name,
                    human_size(rec.transferred),
                    rec.status
                );
            }
        }
        TransferAction::Clear => {
            let n = history::clear()?;
            println!("Cleared {n} transfer record(s).");
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
                    Some(fp) => println!("{status} {name}{verified}\t{id}\t({fp})"),
                    None => println!("{status} {name}{verified}\t{id}"),
                }
            }
        }
        ContactAction::Remove { name } => {
            if book::contact_remove(&name)? {
                book::unmark_verified(&name).ok();
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
    }
    Ok(())
}

fn sessions_cmd(action: SessionAction) -> Result<()> {
    match action {
        SessionAction::List => {
            let list = sessions::list();
            if list.is_empty() {
                eprintln!(
                    "(no resumable sessions — they're saved automatically when you `arvolo send`)"
                );
                return Ok(());
            }
            eprintln!("Resume with: arvolo send --resume <id>\n");
            for rec in list {
                let kind = if rec.archive { "archive" } else { "file" };
                println!(
                    "{}\t{}\t{} chunk(s), {} bytes\t{}",
                    rec.id, rec.name, rec.chunks, rec.total_size, kind
                );
            }
        }
        SessionAction::Rm { id } => {
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

/// Resolve the relay to use for presence, requiring one (offers can't work P2P).
fn require_relay(relay: Option<String>, use_http: bool) -> Result<String> {
    relay
        .map(|r| book::normalize_relay(&r, use_http))
        .or_else(book::default_relay)
        .context("a relay is required: pass --relay <host>, set ARVOLO_RELAY, or configure `relay`")
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
    let relay = require_relay(relay, use_http)?;
    let me = my_identity()?;
    let my_id = encode_id(&me.public());
    let download_dir = download_dir.unwrap_or_else(|| PathBuf::from("."));

    let manager = TransferManager::new(me, Some(relay.clone()), download_dir.clone());
    let mut events = manager.subscribe();
    let inbox = manager.spawn_inbox()?;

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
    let relay = require_relay(relay, use_http)?;
    let recipient = book::resolve_recipient(&to)?;
    let me = my_identity()?;
    let (payload, name, archive, temp) = resolve_payload(&paths)?;
    if archive {
        eprintln!("Packing {} item(s) into an archive…", paths.len());
    }

    let manager = TransferManager::new(me, Some(relay.clone()), PathBuf::from("."));
    let mut events = manager.subscribe();

    // `send_to` decides live-vs-mailbox itself (with a presence grace window and a
    // watchdog); the up-front check is only a hint for the opening line.
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
    resume_ticket: Option<String>,
    resume: Option<String>,
    seed_relay: Option<String>,
    code_mode: bool,
    relay: Option<String>,
    use_http: bool,
    to: Option<String>,
    qr: bool,
) -> Result<()> {
    // Resume paths short-circuit the normal flow: re-serve a previous send so
    // the ticket you already handed out stays valid after the sender restarted.
    // Recovery is pure P2P and independent of --seed-relay.
    if let Some(id) = resume {
        anyhow::ensure!(
            resume_ticket.is_none(),
            "--resume and --resume-ticket are mutually exclusive"
        );
        anyhow::ensure!(
            paths.is_empty() && to.is_none() && !code_mode,
            "--resume takes no paths, --to, or --code (it replays a saved session)"
        );
        return resume_by_id(&id, qr).await;
    }
    if let Some(ticket) = resume_ticket {
        anyhow::ensure!(
            !code_mode && to.is_none(),
            "--resume-ticket re-serves a plain ticket over P2P (no --code / --to)"
        );
        anyhow::ensure!(
            paths.len() == 1,
            "--resume-ticket needs exactly one path: the file to re-serve"
        );
        return resume_by_ticket(&ticket, &paths[0], qr).await;
    }

    anyhow::ensure!(
        !paths.is_empty(),
        "provide at least one file or folder to send (or use --resume / --resume-ticket)"
    );

    // A bare relay host gets a scheme (https by default, http with --use-http).
    let seed_relay = seed_relay.map(|r| book::normalize_relay(&r, use_http));
    let relay = relay.map(|r| book::normalize_relay(&r, use_http));
    let (payload, name, archive, temp) = resolve_payload(&paths)?;
    if archive {
        eprintln!("Packing {} item(s) into an archive…", paths.len());
    }
    // Resolve --to into a (sender identity, recipient) pair we can borrow from.
    let to_owned: Option<(Identity, PublicId)> = match &to {
        Some(t) => Some((my_identity()?, book::resolve_recipient(t)?)),
        None => None,
    };
    let to_ref = to_owned.as_ref().map(|(me, r)| (me, r));

    if code_mode {
        return send_with_code(payload, name, archive, temp, relay, to_ref, qr).await;
    }
    eprintln!("Splitting and serving chunks…");
    let session = flow::prepare_send(
        &payload,
        &name,
        archive,
        to_ref,
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
    session
        .serve(cancel, move |ev| match ev {
            SendEvent::Ready {
                chunks,
                ticket,
                has_relay,
                ..
            } => {
                println!("\nFile ready ({chunks} chunks). On the other device:\n");
                println!("    arvolo recv {ticket}\n");
                if qr {
                    print_qr(&ticket);
                }
                if has_relay {
                    println!("P2P-first; if the receiver drops, only the missing chunks are backfilled to the relay.");
                }
                println!("Ctrl-C to stop.");
            }
            SendEvent::ReceiverConnected => {}
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

/// `send --resume-ticket`: re-serve `path` under an existing *plain* ticket so it
/// stays valid after the sender restarted. The key rides in the ticket, so no
/// saved session is needed. Sealed (`--to`) tickets must use `--resume` instead.
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
            SendEvent::ReceiverConnected => {}
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

async fn recv(ticket: String, out: Option<PathBuf>) -> Result<()> {
    // A short pairing code is resolved to the real ticket over a rendezvous first.
    let ticket = if code::looks_like_code(&ticket) {
        eprintln!("Pairing… (waiting for the sender)");
        let default_relay = book::default_relay();
        code::resolve_code(&ticket, default_relay.as_deref())
            .await
            .context("pairing")?
    } else {
        ticket
    };
    // Our identity is needed to open a ticket sealed to us (--to); harmless
    // otherwise (created on first use).
    let me = my_identity()?;

    let cancel = cancel_on_ctrl_c();
    let tty = std::io::stderr().is_terminal();
    let bar: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
    let b = bar.clone();
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
                    if let Some(pb) = slot.as_ref() {
                        pb.inc(bytes);
                        let src = match source {
                            ChunkSource::Relay => "relay",
                            ChunkSource::Sender => "sender",
                        };
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

#[allow(clippy::too_many_arguments)]
async fn send_offline(
    path: PathBuf,
    to: String,
    relay: Option<String>,
    use_http: bool,
    ttl: u64,
    max: u32,
    password: Option<String>,
    qr: bool,
) -> Result<()> {
    let me = my_identity()?;
    let recipient = book::resolve_recipient(&to)?;
    let relay = relay
        .map(|r| book::normalize_relay(&r, use_http))
        .or_else(book::default_relay)
        .context("no relay: pass --relay <host>, set ARVOLO_RELAY, or configure `relay`")?;
    let deposited = flow::deposit_offline(
        &path,
        &recipient,
        &me,
        &relay,
        ttl,
        max,
        password.as_deref(),
    )
    .await?;
    let encoded = deposited.ticket.encode();
    println!("\nEncrypted and deposited (expires in {ttl}s, {max} download(s)).");
    if password.is_some() {
        println!("Password-protected — share the password out-of-band (not with the ticket).");
    }
    println!("Send this ticket to the recipient:\n");
    println!("    arvolo recv-offline {encoded}\n");
    println!("Keep this revoke token to cancel the delivery later:\n");
    println!(
        "    arvolo revoke {encoded} --token {}\n",
        deposited.revoke_token
    );
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
    // A successful fetch means HPKE auth passed, so the sender in the ticket is
    // genuine — surface it (offline tickets are always sealed to a recipient).
    let (path, n) = flow::fetch_offline(&ticket, out, &me, password.as_deref()).await?;
    if let Ok(t) = arvolo_core::offline::OfflineTicket::decode(&ticket) {
        print_sender_banner(Some(&t.sender));
    }
    println!("Saved {n} bytes to {}", path.display());
    Ok(())
}

async fn revoke(ticket: String, token: String) -> Result<()> {
    let t = arvolo_core::offline::OfflineTicket::decode(&ticket)
        .context("not a valid offline ticket (arvm…)")?;
    flow::revoke_offline(&t.relay, &t.claim, &token).await?;
    println!("Revoked — the blob is no longer available on the relay.");
    Ok(())
}
