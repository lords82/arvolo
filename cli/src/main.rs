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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use arvolo_core::code;
use arvolo_core::crypto::{Identity, PublicId};
use arvolo_core::flow::{self, ChunkSource, RecvEvent, SendEvent};
use arvolo_core::transfer::RelayChoice;
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use tokio_util::sync::CancellationToken;

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
    /// Serve a file P2P; prints a ticket (or short code) and stays running.
    Send {
        path: PathBuf,
        /// Also seed the file to a relay so the recipient can finish even if you
        /// go offline (backfill). Relay base URL, e.g. http://relay:8787
        #[arg(long)]
        seed_relay: Option<String>,
        /// Show a short pairing code (e.g. 4821-crater-mango) instead of the long
        /// ticket. Needs a relay: --relay, or the ARVOLO_RELAY env var.
        #[arg(long)]
        code: bool,
        /// Rendezvous relay for --code. When given, it is embedded in the code so
        /// the receiver needs no configuration.
        #[arg(long)]
        relay: Option<String>,
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
    /// Show your public id (creates an identity on first use).
    Id,
    /// Encrypt a file for a recipient and deposit it on a relay (offline send).
    SendOffline {
        path: PathBuf,
        /// Recipient's public id (from their `arvolo id`).
        #[arg(long)]
        to: String,
        /// Relay base URL, e.g. https://relay.example:8787
        #[arg(long)]
        relay: String,
        /// Time-to-live in seconds (default 7 days).
        #[arg(long, default_value_t = 7 * 24 * 3600)]
        ttl: u64,
        /// Max downloads before deletion (default 1 = burn-after-read).
        #[arg(long, default_value_t = 1)]
        max: u32,
        /// Also render the ticket as a scannable QR code.
        #[arg(long)]
        qr: bool,
    },
    /// Fetch and decrypt an offline ticket (`arvm…`).
    RecvOffline {
        ticket: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
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
            path,
            seed_relay,
            code,
            relay,
            qr,
        } => send(path, seed_relay, code, relay, qr).await,
        Command::Recv { ticket, out } => recv(ticket, out).await,
        Command::Id => id(),
        Command::SendOffline {
            path,
            to,
            relay,
            ttl,
            max,
            qr,
        } => send_offline(path, to, relay, ttl, max, qr).await,
        Command::RecvOffline { ticket, out } => recv_offline(ticket, out).await,
    }
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
    println!("{}", encode_id(&id.public()));
    eprintln!("(identity stored at {})", identity_path().display());
    Ok(())
}

fn encode_id(p: &PublicId) -> String {
    data_encoding::BASE32_NOPAD
        .encode(&p.to_bytes())
        .to_lowercase()
}

fn decode_id(s: &str) -> Result<PublicId> {
    let bytes = data_encoding::BASE32_NOPAD
        .decode(s.trim().to_uppercase().as_bytes())
        .context("invalid public id (base32)")?;
    PublicId::from_bytes(&bytes)
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

async fn send(
    path: PathBuf,
    seed_relay: Option<String>,
    code_mode: bool,
    relay: Option<String>,
    qr: bool,
) -> Result<()> {
    if code_mode {
        return send_with_code(path, relay, qr).await;
    }
    eprintln!("Splitting and serving chunks…");
    let session = flow::prepare_send(&path, seed_relay, RelayChoice::from_env()).await?;
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

/// `send --code`: hand the ticket to the receiver via a short pairing code over a
/// relay rendezvous, serving the file (and using the relay for backfill) meanwhile.
async fn send_with_code(path: PathBuf, relay: Option<String>, qr: bool) -> Result<()> {
    // An explicit --relay is embedded in the code (works with no receiver config);
    // otherwise fall back to ARVOLO_RELAY (short code, shared default).
    let (relay_url, embed) = match relay {
        Some(r) => (r, true),
        None => match std::env::var("ARVOLO_RELAY") {
            Ok(r) if !r.trim().is_empty() => (r, false),
            _ => anyhow::bail!("--code needs a relay: pass --relay <url> or set ARVOLO_RELAY"),
        },
    };

    eprintln!("Splitting and serving chunks…");
    let session = flow::prepare_send(&path, Some(relay_url.clone()), RelayChoice::from_env()).await?;
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
            SendEvent::Ready { .. } => {} // code already printed
        })
        .await;
    pairing.abort();
    result
}

async fn recv(ticket: String, out: Option<PathBuf>) -> Result<()> {
    // A short pairing code is resolved to the real ticket over a rendezvous first.
    let ticket = if code::looks_like_code(&ticket) {
        eprintln!("Pairing… (waiting for the sender)");
        let default_relay = std::env::var("ARVOLO_RELAY").ok();
        code::resolve_code(&ticket, default_relay.as_deref())
            .await
            .context("pairing")?
    } else {
        ticket
    };

    let cancel = cancel_on_ctrl_c();
    let tty = std::io::stderr().is_terminal();
    let bar: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
    let b = bar.clone();
    flow::recv_chunked(&ticket, out, RelayChoice::from_env(), cancel, move |ev| {
        let mut slot = b.lock().unwrap();
        match ev {
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
                    if connected { "connected" } else { "unavailable" }
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
    })
    .await?;
    Ok(())
}

// ---- offline mailbox ------------------------------------------------------

async fn send_offline(
    path: PathBuf,
    to: String,
    relay: String,
    ttl: u64,
    max: u32,
    qr: bool,
) -> Result<()> {
    let me = my_identity()?;
    let recipient = decode_id(&to)?;
    let ticket = flow::deposit_offline(&path, &recipient, &me, &relay, ttl, max).await?;
    let encoded = ticket.encode();
    println!("\nEncrypted and deposited (expires in {ttl}s, {max} download(s)).");
    println!("Send this ticket to the recipient:\n");
    println!("    arvolo recv-offline {encoded}\n");
    if qr {
        print_qr(&encoded);
    }
    Ok(())
}

async fn recv_offline(ticket: String, out: Option<PathBuf>) -> Result<()> {
    let me = my_identity()?;
    let (path, n) = flow::fetch_offline(&ticket, out, &me).await?;
    println!("Saved {n} bytes to {}", path.display());
    Ok(())
}
