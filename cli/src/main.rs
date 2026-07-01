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

use anyhow::{Context, Result};
use arvolo_core::backfill::RelayRelease;
use arvolo_core::chunked::{ChunkReceiver, ChunkSender, ChunkTicket};
use arvolo_core::crypto::{open, seal, Identity, PublicId, Sealed};
use arvolo_core::offline::OfflineTicket;
use arvolo_core::transfer::RelayChoice;
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

// ---- P2P ------------------------------------------------------------------

async fn send(path: PathBuf, seed_relay: Option<String>) -> Result<()> {
    anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
    eprintln!("Splitting and serving chunks…");
    let sender = ChunkSender::serve(&path, RelayChoice::from_env())
        .await
        .context("start sender")?;
    let client = reqwest::Client::new();

    // With --seed-relay we DON'T upload anything yet (lazy): we just learn the
    // relay's address + a token, and backfill only the undelivered tail if the
    // receiver drops.
    let mut relay = None;
    if let Some(url) = seed_relay {
        let url = url.trim_end_matches('/').to_string();
        let resp = client
            .get(format!("{url}/v1/addr"))
            .send()
            .await
            .context("relay /v1/addr")?
            .error_for_status()
            .context("relay rejected addr")?
            .text()
            .await
            .context("read relay addr")?;
        let mut lines = resp.lines();
        let addr = lines
            .next()
            .context("missing relay address")?
            .trim()
            .to_string();
        let token = lines.next().context("missing token")?.trim().to_string();
        relay = Some(RelayRelease {
            http: url,
            addr,
            token,
        });
    }

    let ticket = ChunkTicket {
        total_size: sender.total_size(),
        chunk_size: sender.chunk_size(),
        chunks: sender.chunks().to_vec(),
        providers: vec![sender.addr()],
        relay: relay.clone(),
        key: sender.key().to_vec(),
    };
    println!(
        "\nFile ready ({} chunks). On the other device:\n",
        ticket.chunks.len()
    );
    println!("    arvolo recv {}\n", ticket.encode()?);
    if relay.is_some() {
        println!("P2P-first; if the receiver drops, only the missing chunks are backfilled to the relay.");
    }
    println!("Ctrl-C to stop.");

    // Orchestration loop: on receiver-drop, backfill the undelivered tail.
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            undelivered = sender.receiver_gone() => {
                let Some(r) = &relay else { continue };
                if undelivered.is_empty() { continue; }
                let chunks: Vec<_> = undelivered.iter().map(|&i| sender.chunks()[i]).collect();
                let req = arvolo_core::chunked::SeedRequest {
                    sender: sender.addr(),
                    chunks,
                    token: r.token.clone(),
                };
                eprintln!("Receiver dropped — backfilling {} missing chunks to the relay…", undelivered.len());
                match client.post(format!("{}/v1/seed", r.http)).body(req.encode()?).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        sender.mark_on_relay(&undelivered);
                        eprintln!("Backfilled. You can close this; the relay can finish the delivery.");
                    }
                    Ok(resp) => eprintln!("Relay backfill rejected: {}", resp.status()),
                    Err(e) => eprintln!("Relay backfill failed: {e}"),
                }
            }
        }
    }
    sender.shutdown().await;
    Ok(())
}

async fn recv(ticket: String, out: Option<PathBuf>) -> Result<()> {
    use std::collections::HashSet;
    use std::io::{Seek, SeekFrom, Write};
    use std::sync::{Arc, Mutex};

    let t = ChunkTicket::decode(&ticket).context("invalid ticket")?;
    let out = out.unwrap_or_else(|| {
        default_out(&t.chunks.first().map(|h| h.to_string()).unwrap_or_default())
    });
    let sender_addr = t.providers.first().cloned();
    let relay_addr = match &t.relay {
        Some(r) => Some(arvolo_core::chunked::decode_addr(&r.addr).context("relay address")?),
        None => None,
    };
    let key: [u8; arvolo_core::crypto::CHUNK_KEY_LEN] = t
        .key
        .as_slice()
        .try_into()
        .context("ticket has no/invalid content key")?;
    let total_chunks = t.chunks.len() as u32;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&out)
        .with_context(|| format!("open {}", out.display()))?;
    let existing = file.metadata()?.len();
    let start = ((existing / t.chunk_size as u64) as usize).min(t.chunks.len());
    if start > 0 {
        eprintln!("Resuming from chunk {start}/{}…", t.chunks.len());
    } else {
        eprintln!("Fetching {} chunks…", t.chunks.len());
    }

    let receiver = ChunkReceiver::open(RelayChoice::from_env()).await?;
    let client = reqwest::Client::new();

    // Control channel to the sender; the sender advertises on-relay chunks here,
    // and our acks let it detect a drop and backfill only the undelivered tail.
    // The first connection to the sender pays the full cold-start cost (relay
    // handshake + hole punching), which can exceed a single timeout.
    //
    // How patient we are scales with whether we have a fallback: if the ticket
    // carries a relay we can complete even with the sender offline, so one short
    // attempt is enough (don't burn ~45s dialing a dead sender). With no relay
    // the sender is the ONLY source, so keep retrying to ride out a cold start.
    let on_relay: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut control = None;
    if let Some(s) = &sender_addr {
        let attempts = if t.relay.is_some() { 1 } else { 3 };
        for attempt in 1..=attempts {
            match tokio::time::timeout(
                std::time::Duration::from_secs(12),
                receiver.open_control(s, on_relay.clone()),
            )
            .await
            {
                Ok(Some(c)) => {
                    control = Some(c);
                    break;
                }
                _ if attempt < attempts => {
                    eprintln!("control channel attempt {attempt}/{attempts} failed; retrying…")
                }
                _ => {}
            }
        }
    }
    eprintln!(
        "control channel to sender: {}",
        if control.is_some() {
            "connected"
        } else {
            "unavailable"
        }
    );
    // Give the sender a moment to send its RelayHas snapshot before we choose sources.
    if control.is_some() {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    // If the control channel never came up, the sender is most likely offline
    // (it backfilled the tail and quit). Don't waste a connect timeout per chunk
    // dialing the dead sender first — prefer the relay.
    let sender_offline = control.is_none();

    for i in start..t.chunks.len() {
        let on_relay_chunk = on_relay.lock().unwrap().contains(&(i as u32));
        // Anti-double-send: chunks the sender pushed to the relay are pulled from
        // the relay; everything else is pulled from the sender (relay fallback).
        let mut providers = Vec::new();
        if on_relay_chunk || sender_offline {
            if let Some(a) = &relay_addr {
                providers.push(a.clone());
            }
            if let Some(a) = &sender_addr {
                providers.push(a.clone());
            }
        } else {
            if let Some(a) = &sender_addr {
                providers.push(a.clone());
            }
            if let Some(a) = &relay_addr {
                providers.push(a.clone());
            }
        }

        let bytes = receiver
            .fetch_chunk(&providers, t.chunks[i])
            .await
            .with_context(|| format!("fetch chunk {}", i + 1))?;
        // The fetched bytes are ciphertext (the relay/providers never see
        // plaintext); decrypt before writing to disk.
        let plain = arvolo_core::crypto::open_chunk(&key, i as u32, total_chunks, &bytes)
            .with_context(|| format!("decrypt chunk {}", i + 1))?;
        file.seek(SeekFrom::Start(i as u64 * t.chunk_size as u64))?;
        file.write_all(&plain)?;

        if let Some(c) = control.as_mut() {
            let _ = c.ack(i as u32).await;
        }
        // Free relay-backfilled chunks as we get them. We attempt release for
        // every fetched chunk (not just ones the control channel flagged): when
        // the sender is offline there is no control channel to learn `on_relay`,
        // yet those are exactly the chunks the relay is holding. The relay's
        // (token, hash) guard makes release a no-op for anything not seeded.
        if let Some(r) = &t.relay {
            let _ = client
                .post(format!(
                    "{}/v1/release/{}/{}",
                    r.http.trim_end_matches('/'),
                    r.token,
                    t.chunks[i]
                ))
                .send()
                .await;
        }
    }
    if let Some(c) = control {
        let _ = c.finish().await;
    }
    file.set_len(t.total_size)?;
    receiver.close().await;
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
