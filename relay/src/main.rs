//! arvolo relay / mailbox server.
//!
//! Zero-knowledge store-and-forward: serves the deposit/fetch API and reaps
//! expired blobs in the background.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use arvolo_relay::{now_unix, router, Mailbox};

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
    let mailbox =
        Arc::new(Mailbox::open(&db, &blobs).map_err(|e| anyhow::anyhow!("open mailbox: {e}"))?);
    tracing::info!(%db, %blobs, "mailbox storage ready");

    // Background reaper: delete expired blobs every minute.
    {
        let mailbox = mailbox.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                match mailbox.reap(now_unix()) {
                    Ok(n) if n > 0 => tracing::info!(removed = n, "reaped expired blobs"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("reaper error: {e}"),
                }
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("arvolo relay listening on {addr}");
    axum::serve(listener, router(mailbox)).await?;
    Ok(())
}
