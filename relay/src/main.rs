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

/// Build the tracing filter from `-v` count. arvolo (`arvolo_relay`,
/// `arvolo_core`) gets louder with each `-v`; dependencies (iroh, hyper, …) stay
/// quiet at `warn` until `-vvv`, which raises the base to `debug` so their logs
/// show too.
fn relay_log_filter(verbosity: u8) -> String {
    match verbosity {
        0 => "warn,arvolo_relay=info,arvolo_core=info".into(),
        1 => "warn,arvolo_relay=debug,arvolo_core=debug".into(),
        2 => "warn,arvolo_relay=trace,arvolo_core=trace".into(),
        _ => "debug,arvolo_relay=trace,arvolo_core=trace".into(),
    }
}

/// Whether `addr` binds only the loopback interface (so plaintext HTTP never leaves
/// the host). A bare unspecified/public bind (`0.0.0.0:…`, a LAN/public IP) or an
/// unparseable host is treated as non-loopback → warn.
fn is_loopback_bind(addr: &str) -> bool {
    addr.parse::<std::net::SocketAddr>()
        .map(|s| s.ip().is_loopback())
        .unwrap_or(false)
}

#[tokio::main]
async fn main() -> Result<()> {
    // The relay takes no positional args — it's configured entirely via the
    // ARVOLO_RELAY_* env vars below. It does accept -v/-vv/-vvv (verbosity) and
    // --version/--help; anything else is rejected so an accidental flag prints and
    // exits instead of silently starting the server and grabbing the listen port.
    let mut verbosity: u8 = 0;
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
                println!(
                    "  -v, -vv, -vvv   log verbosity: arvolo debug/trace; -vvv also shows iroh"
                );
                println!("  -V, --version   print version and exit");
                println!("  -h, --help      print this help and exit");
                println!();
                println!("Environment:");
                println!("  RUST_LOG                explicit log filter (overrides -v)");
                println!("  ARVOLO_RELAY_ADDR       listen address (default 0.0.0.0:6282)");
                println!("  ARVOLO_RELAY_DB         mailbox db path (default arvolo-relay.db)");
                println!("  ARVOLO_RELAY_BLOBS      blob directory (default arvolo-blobs)");
                println!(
                    "  ARVOLO_RELAY_BLOBSTORE  blobstore directory (default arvolo-blobstore)"
                );
                return Ok(());
            }
            "--verbose" => verbosity = verbosity.saturating_add(1),
            // -v, -vv, -vvv, … (stacked v's).
            s if s.len() >= 2 && s.starts_with("-v") && s[1..].bytes().all(|b| b == b'v') => {
                verbosity = verbosity.saturating_add((s.len() - 1) as u8);
            }
            other => {
                eprintln!("arvolo-relay: unexpected argument '{other}' (try --help)");
                std::process::exit(2);
            }
        }
    }

    // Default: only arvolo's own logs (deps like iroh stay at warn). -v/-vv raise
    // arvolo to debug/trace; -vvv also surfaces iroh and the rest. An explicit
    // RUST_LOG always wins.
    let filter = std::env::var("RUST_LOG")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| relay_log_filter(verbosity));
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .init();

    let addr = std::env::var("ARVOLO_RELAY_ADDR").unwrap_or_else(|_| "0.0.0.0:6282".to_string());
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
                // Per-session offload meters whose TTL has passed (so a genuinely
                // new transfer of the same file later starts with a fresh cap).
                state.mailbox.reap_session_bytes(now);
            }
        });
    }

    // Deploy-safety warnings (F2/F9). The relay speaks plaintext HTTP and relies on
    // an upstream TLS terminator; and by default it meters neither blob size nor the
    // per-transfer seed offload. Warn loudly when exposed without those bounds so a
    // misconfigured public relay is obvious in the logs. `ARVOLO_INSECURE=1`
    // acknowledges an intentional plaintext bind (TLS handled upstream).
    let insecure_ok = matches!(
        std::env::var("ARVOLO_INSECURE")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    );
    if !is_loopback_bind(&addr) && !insecure_ok {
        tracing::warn!(
            %addr,
            "relay is listening in PLAINTEXT HTTP on a non-loopback address — it MUST sit behind \
             a TLS-terminating reverse proxy, or capability tokens (claim / revoke / inbox \
             session) travel in cleartext. Set ARVOLO_INSECURE=1 to silence this once TLS is \
             handled upstream."
        );
    }
    if arvolo_relay::max_blob_bytes() == 0 {
        tracing::warn!(
            "ARVOLO_MAX_BLOB_BYTES is set to 0 (unlimited): any unauthenticated client can deposit \
             arbitrarily large blobs. Set a finite value on a public relay (default 16 GiB)."
        );
    }
    if arvolo_relay::max_total_blob_bytes() == 0 {
        tracing::warn!(
            "ARVOLO_MAX_TOTAL_BLOB_BYTES is unset (unlimited): the per-blob cap alone does not \
             stop many deposits from filling the disk. Set it to the disk budget you are willing \
             to lend on a public relay."
        );
    }
    if arvolo_relay::max_session_relay_bytes() == 0 {
        tracing::warn!(
            "ARVOLO_MAX_SESSION_RELAY_BYTES is unset (unlimited): the seed/backfill path has no \
             per-transfer offload cap. Set a finite value on a public relay."
        );
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("arvolo relay listening on {addr}");
    // Serve with connect info so the rendezvous rate limiter can key on the
    // client IP (behind a reverse proxy, set ARVOLO_TRUST_PROXY to use
    // X-Forwarded-For instead).
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
