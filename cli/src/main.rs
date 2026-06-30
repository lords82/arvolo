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
//! P2P transport is encrypted by QUIC; the offline path is end-to-end encrypted
//! with HPKE so the relay only ever sees ciphertext.

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use arvolo_core::backfill::{fetch_from_providers, parse_relay_addr, ProviderTicket, RelayRelease};
use arvolo_core::crypto::{open, seal, Identity, PublicId, Sealed};
use arvolo_core::offline::OfflineTicket;
use arvolo_core::transfer::{fetch_to_path, Provider, RelayChoice};
use clap::{Parser, Subcommand};

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
    /// Fetch a file from a P2P ticket (or a multi-provider `arvp…` ticket).
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

// ---- P2P ------------------------------------------------------------------

async fn send(path: PathBuf, seed_relay: Option<String>) -> Result<()> {
    anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
    let provider = Provider::from_path(&path).await.context("start provider")?;
    eprintln!("Connecting to relay…");
    let ticket = provider.ticket_online().await;

    match seed_relay {
        None => {
            println!("\nFile ready to send: {}", path.display());
            println!("On the other device run:\n");
            println!("    arvolo recv {ticket}\n");
            println!("Keep this running until the transfer completes. Ctrl-C to stop.");
        }
        Some(url) => {
            let url = url.trim_end_matches('/');
            eprintln!("Seeding to relay (so the recipient can finish even if you go offline)…");
            let resp = reqwest::Client::new()
                .post(format!("{url}/v1/seed"))
                .body(ticket.to_string())
                .send()
                .await
                .context("seed request")?
                .error_for_status()
                .context("relay rejected seed")?
                .text()
                .await
                .context("read seed response")?;
            let mut lines = resp.lines();
            let addr_enc = lines.next().context("missing relay address")?;
            let token = lines.next().context("missing release token")?;
            let relay_addr = parse_relay_addr(addr_enc.trim()).context("relay address")?;
            let pt = ProviderTicket {
                hash: provider.hash(),
                providers: vec![ticket.addr().clone(), relay_addr],
                relay: Some(RelayRelease {
                    http: url.to_string(),
                    token: token.trim().to_string(),
                }),
            };
            println!("\nSeeded to relay ✓  The recipient can fetch from you OR the relay:\n");
            println!("    arvolo recv {}\n", pt.encode()?);
            println!("Stay online for fast P2P, or close this — the relay can still deliver. Ctrl-C to stop.");
        }
    }

    tokio::signal::ctrl_c().await.ok();
    provider.shutdown().await;
    Ok(())
}

async fn recv(ticket: String, out: Option<PathBuf>) -> Result<()> {
    // Multi-provider ticket (P2P + relay backfill).
    if ProviderTicket::looks_like(&ticket) {
        let pt = ProviderTicket::decode(&ticket).context("invalid provider ticket")?;
        let out = out.unwrap_or_else(|| default_out(&pt.hash.to_string()));
        eprintln!("Fetching (P2P, relay fallback)…");
        fetch_from_providers(&pt, &out, RelayChoice::from_env())
            .await
            .context("fetch")?;
        // Delete-after-delivery: tell the relay we're done so it drops its copy.
        if let Some(r) = &pt.relay {
            let _ = reqwest::Client::new()
                .post(format!(
                    "{}/v1/release/{}",
                    r.http.trim_end_matches('/'),
                    r.token
                ))
                .send()
                .await;
        }
        println!("Saved to {}", out.display());
        return Ok(());
    }

    let ticket = arvolo_core::reexport::BlobTicket::from_str(ticket.trim())
        .map_err(|e| anyhow::anyhow!("invalid ticket: {e}"))?;
    let out = out.unwrap_or_else(|| default_out(&ticket.hash().to_string()));
    eprintln!("Fetching…");
    fetch_to_path(&ticket, &out, RelayChoice::from_env())
        .await
        .context("fetch")?;
    println!("Saved to {}", out.display());
    Ok(())
}

// ---- offline mailbox ------------------------------------------------------

async fn send_offline(path: PathBuf, to: String, relay: String, ttl: u64, max: u32) -> Result<()> {
    anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
    let me = my_identity()?;
    let recipient = decode_id(&to)?;
    let plaintext = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;

    let sealed = seal(&plaintext, &recipient, &me, b"").context("encrypt")?;

    let relay = relay.trim_end_matches('/').to_string();
    let url = format!("{relay}/v1/deposit?ttl={ttl}&max={max}");
    let client = reqwest::Client::new();
    let claim = client
        .post(&url)
        .header(
            "x-arvolo-encapped-key",
            data_encoding::BASE32_NOPAD.encode(&sealed.encapped_key),
        )
        .body(sealed.ciphertext)
        .send()
        .await
        .context("deposit request")?
        .error_for_status()
        .context("relay rejected deposit")?
        .text()
        .await
        .context("read claim")?;

    let ticket = OfflineTicket {
        relay,
        claim: claim.trim().to_string(),
        sender: me.public().to_bytes(),
    };
    println!("\nEncrypted and deposited (expires in {ttl}s, {max} download(s)).");
    println!("Send this ticket to the recipient:\n");
    println!("    arvolo recv-offline {}\n", ticket.encode());
    Ok(())
}

async fn recv_offline(ticket: String, out: Option<PathBuf>) -> Result<()> {
    let me = my_identity()?;
    let t = OfflineTicket::decode(&ticket)?;
    let sender = PublicId::from_bytes(&t.sender).context("invalid sender in ticket")?;

    let url = format!("{}/v1/fetch/{}", t.relay.trim_end_matches('/'), t.claim);
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .context("fetch request")?
        .error_for_status()
        .context("relay rejected fetch (expired or already claimed?)")?;

    let encapped = resp
        .headers()
        .get("x-arvolo-encapped-key")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            data_encoding::BASE32_NOPAD
                .decode(s.to_uppercase().as_bytes())
                .ok()
        })
        .context("missing encapped key from relay")?;
    let ciphertext = resp.bytes().await.context("read ciphertext")?.to_vec();

    let plaintext = open(
        &Sealed {
            encapped_key: encapped,
            ciphertext,
        },
        &me,
        &sender,
        b"",
    )
    .context("decrypt (wrong identity, sender, or tampered)")?;

    let out = out.unwrap_or_else(|| default_out(&t.claim));
    std::fs::write(&out, &plaintext).with_context(|| format!("write {}", out.display()))?;
    println!("Saved {} bytes to {}", plaintext.len(), out.display());
    Ok(())
}

fn default_out(seed: &str) -> PathBuf {
    PathBuf::from(format!("received-{}.bin", &seed[..seed.len().min(16)]))
}
