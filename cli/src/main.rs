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

use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::crypto::{Identity, PublicId};
use arvolo_core::flow::{self, RecvEvent, SendEvent};
use arvolo_core::transfer::RelayChoice;
use clap::{Parser, Subcommand};
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
    /// Serve a file P2P; prints a ticket and stays running until Ctrl-C.
    Send {
        path: PathBuf,
        /// Also seed the file to a relay so the recipient can finish even if you
        /// go offline (backfill). Relay base URL, e.g. http://relay:8787
        #[arg(long)]
        seed_relay: Option<String>,
    },
    /// Fetch a file from a chunked ticket (`arvc…`); resumes if interrupted.
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
        Command::Send { path, seed_relay } => send(path, seed_relay).await,
        Command::Recv { ticket, out } => recv(ticket, out).await,
        Command::Id => id(),
        Command::SendOffline {
            path,
            to,
            relay,
            ttl,
            max,
        } => send_offline(path, to, relay, ttl, max).await,
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

// ---- P2P ------------------------------------------------------------------

async fn send(path: PathBuf, seed_relay: Option<String>) -> Result<()> {
    eprintln!("Splitting and serving chunks…");
    let session = flow::prepare_send(&path, seed_relay, RelayChoice::from_env()).await?;
    let cancel = cancel_on_ctrl_c();
    session
        .serve(cancel, |ev| match ev {
            SendEvent::Ready {
                chunks,
                ticket,
                has_relay,
                ..
            } => {
                println!("\nFile ready ({chunks} chunks). On the other device:\n");
                println!("    arvolo recv {ticket}\n");
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

async fn recv(ticket: String, out: Option<PathBuf>) -> Result<()> {
    let cancel = cancel_on_ctrl_c();
    flow::recv_chunked(&ticket, out, RelayChoice::from_env(), cancel, |ev| match ev {
        RecvEvent::Started {
            total,
            resuming_from,
        } => {
            if resuming_from > 0 {
                eprintln!("Resuming from chunk {resuming_from}/{total}…");
            } else {
                eprintln!("Fetching {total} chunks…");
            }
        }
        RecvEvent::Control { connected } => eprintln!(
            "control channel to sender: {}",
            if connected { "connected" } else { "unavailable" }
        ),
        RecvEvent::Chunk { .. } => {}
        RecvEvent::Saved { path } => println!("Saved to {}", path.display()),
        RecvEvent::Warning { message } => eprintln!("{message}"),
    })
    .await?;
    Ok(())
}

// ---- offline mailbox ------------------------------------------------------

async fn send_offline(path: PathBuf, to: String, relay: String, ttl: u64, max: u32) -> Result<()> {
    let me = my_identity()?;
    let recipient = decode_id(&to)?;
    let ticket = flow::deposit_offline(&path, &recipient, &me, &relay, ttl, max).await?;
    println!("\nEncrypted and deposited (expires in {ttl}s, {max} download(s)).");
    println!("Send this ticket to the recipient:\n");
    println!("    arvolo recv-offline {}\n", ticket.encode());
    Ok(())
}

async fn recv_offline(ticket: String, out: Option<PathBuf>) -> Result<()> {
    let me = my_identity()?;
    let (path, n) = flow::fetch_offline(&ticket, out, &me).await?;
    println!("Saved {n} bytes to {}", path.display());
    Ok(())
}
