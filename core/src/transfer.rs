//! Endpoint binding and relay selection for the P2P transport.
//!
//! Chunk transfer itself is our own content-addressed protocol (see
//! [`crate::chunked`]); this module just binds an iroh endpoint with the right
//! relay choice and advertises our ALPNs.

use anyhow::Result;
use iroh::{Endpoint, RelayMode};

use crate::chunked::{CHUNK_ALPN, CTRL_ALPN};
use crate::node::generate_secret_key;

/// Which iroh relays to use for NAT traversal / fallback. The relay only ever
/// carries encrypted QUIC traffic; it never sees plaintext.
#[derive(Clone, Debug)]
pub enum RelayChoice {
    /// n0's public relays (shared; suitable for dev/test).
    N0Default,
    /// No relay — direct only (LAN / tests).
    Disabled,
    /// A self-hosted iroh relay URL (production / full sovereignty).
    Custom(String),
}

impl RelayChoice {
    /// Read from `ARVOLO_IROH_RELAY`: a self-hosted relay URL if set, else n0 defaults.
    pub fn from_env() -> Self {
        match std::env::var("ARVOLO_IROH_RELAY") {
            Ok(u) if !u.trim().is_empty() => RelayChoice::Custom(u.trim().to_string()),
            _ => RelayChoice::N0Default,
        }
    }
}

/// Bind an endpoint that speaks our chunk + control ALPNs, with the given relay.
pub async fn bind_endpoint(relay: RelayChoice) -> Result<Endpoint> {
    let builder = match relay {
        RelayChoice::N0Default => Endpoint::builder(),
        RelayChoice::Disabled => Endpoint::empty_builder(RelayMode::Disabled),
        RelayChoice::Custom(url) => {
            let parsed: iroh::RelayUrl = url
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid ARVOLO_IROH_RELAY url {url:?}: {e}"))?;
            Endpoint::empty_builder(RelayMode::Custom(iroh::RelayMap::from(parsed)))
        }
    };
    builder
        .secret_key(generate_secret_key())
        .alpns(vec![CHUNK_ALPN.to_vec(), CTRL_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| anyhow::anyhow!("bind endpoint: {e}"))
}
