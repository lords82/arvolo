//! arvolo relay / mailbox server.
//!
//! Zero-knowledge store-and-forward: serves the deposit/fetch API and reaps
//! expired blobs in the background.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use arvolo_core::backfill::BlobNode;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{now_unix, router, AppState, Mailbox};

#[tokio::main]
async fn main() -> Result<()> {
    // The relay takes no positional args — it's configured entirely via the
    // ARVOLO_RELAY_* env vars below. Still, handle --version/--help (and reject
    // stray flags) so an accidental `arvolo-relay --version` prints and exits
    // instead of silently starting the server and grabbing the listen port.
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-V" | "--version" => {
                println!("arvolo-relay {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-h" | "--help" => {
                println!("arvolo-relay {}", env!("CARGO_PKG_VERSION"));
                println!("Zero-knowledge store-and-forward mailbox server.");
                println!();
                println!("Usage: arvolo-relay   (no arguments; configured via environment)");
                println!();
                println!("Options:");
                println!("  -V, --version   print version and exit");
                println!("  -h, --help      print this help and exit");
                println!();
                println!("Environment:");
                println!("  ARVOLO_RELAY_ADDR       listen address (default 0.0.0.0:8787)");
                println!("  ARVOLO_RELAY_DB         mailbox db path (default arvolo-relay.db)");
                println!("  ARVOLO_RELAY_BLOBS      blob directory (default arvolo-blobs)");
                println!("  ARVOLO_RELAY_BLOBSTORE  blobstore directory (default arvolo-blobstore)");
                return Ok(());
            }
            other => {
                eprintln!("arvolo-relay: unexpected argument '{other}' (try --help)");
                std::process::exit(2);
            }
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let addr = std::env::var("ARVOLO_RELAY_ADDR").unwrap_or_else(|_| "0.0.0.0:8787".to_string());
    let db = std::env::var("ARVOLO_RELAY_DB").unwrap_or_else(|_| "arvolo-relay.db".to_string());
    let blobs = std::env::var("ARVOLO_RELAY_BLOBS").unwrap_or_else(|_| "arvolo-blobs".to_string());
    let blobstore =
        std::env::var("ARVOLO_RELAY_BLOBSTORE").unwrap_or_else(|_| "arvolo-blobstore".to_string());
    let mailbox =
        Arc::new(Mailbox::open(&db, &blobs).map_err(|e| anyhow::anyhow!("open mailbox: {e}"))?);
    tracing::info!(%db, %blobs, "mailbox storage ready");

    // Blob-store node for seed-to-relay backfill (durable P2P delivery).
    let blob_node = Arc::new(
        BlobNode::spawn(std::path::Path::new(&blobstore), RelayChoice::from_env())
            .await
            .map_err(|e| anyhow::anyhow!("start blob node: {e}"))?,
    );
    tracing::info!(%blobstore, "blob-store node ready (backfill)");
    let mut state = AppState::new(mailbox.clone(), blob_node);
    state.links_enabled = !arvolo_relay::links_disabled_from_env();
    tracing::info!(
        links_enabled = state.links_enabled,
        "browser download links"
    );

    // Background reaper: delete expired mailbox blobs AND expired seeded blobs.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                let now = now_unix();
                match state.mailbox.reap(now) {
                    Ok(n) if n > 0 => tracing::info!(removed = n, "reaped expired mailbox blobs"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("reaper error: {e}"),
                }
                // TTL backstop for seeded chunks not released by the receiver.
                for (token, hash) in state.mailbox.expired_seeds(now) {
                    if let Err(e) = state.blobs.release_hex(&hash).await {
                        tracing::warn!("seed reaper release error: {e}");
                    }
                    let _ = state.mailbox.delete_seed_one(&token, &hash);
                    tracing::info!(%hash, "reaped expired seeded chunk");
                }
                // Expired pairing rendezvous slots.
                match state.mailbox.rz_reap(now) {
                    n if n > 0 => tracing::info!(removed = n, "reaped expired rendezvous rows"),
                    _ => {}
                }
                // Expired (unaccepted) inbox offers.
                match state.mailbox.inbox_reap(now) {
                    n if n > 0 => tracing::info!(removed = n, "reaped expired inbox offers"),
                    _ => {}
                }
                // Stale presence beacons.
                match state.mailbox.beacon_reap(now) {
                    n if n > 0 => tracing::info!(removed = n, "reaped stale presence beacons"),
                    _ => {}
                }
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("arvolo relay listening on {addr}");
    axum::serve(listener, router(state)).await?;
    Ok(())
}
