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
    let state = AppState {
        mailbox: mailbox.clone(),
        blobs: blob_node,
    };

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
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("arvolo relay listening on {addr}");
    axum::serve(listener, router(state)).await?;
    Ok(())
}
