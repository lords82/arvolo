//! Endpoint binding and relay selection for the P2P transport.
//!
//! Chunk transfer itself is our own content-addressed protocol (see
//! [`crate::chunked`]); this module just binds an iroh endpoint with the right
//! relay choice and advertises our ALPNs.

use anyhow::Result;
use iroh::{Endpoint, RelayMode, SecretKey};

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

/// Bind an endpoint that speaks our chunk + control ALPNs, with the given relay
/// and a fresh random node id.
pub async fn bind_endpoint(relay: RelayChoice) -> Result<Endpoint> {
    bind_endpoint_with_key(relay, generate_secret_key()).await
}

/// Like [`bind_endpoint`] but with a caller-supplied transport `secret` — so a
/// resumed send can rebind the exact same node id its ticket was issued under.
pub async fn bind_endpoint_with_key(relay: RelayChoice, secret: SecretKey) -> Result<Endpoint> {
    let mut builder = match relay {
        RelayChoice::N0Default => Endpoint::builder(),
        RelayChoice::Disabled => Endpoint::empty_builder(RelayMode::Disabled),
        RelayChoice::Custom(url) => {
            let parsed: iroh::RelayUrl = url
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid ARVOLO_IROH_RELAY url {url:?}: {e}"))?;
            Endpoint::empty_builder(RelayMode::Custom(iroh::RelayMap::from(parsed)))
        }
    };
    // With `ARVOLO_IPV4_ONLY=1`, keep the IPv6 socket on loopback so iroh never
    // discovers or advertises a public IPv6 address. Useful when one peer has a
    // routable IPv6 the other can't reach (a dual-stack server ↔ IPv4-only client):
    // otherwise a dead IPv6 candidate is advertised and dialing it wastes the
    // connection window before falling back. Direct IPv4 + relay still work.
    if ipv4_only() {
        use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
        builder = builder
            .bind_addr_v4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
            .bind_addr_v6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 0, 0, 0));
    }
    builder
        .secret_key(secret)
        .alpns(vec![CHUNK_ALPN.to_vec(), CTRL_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| anyhow::anyhow!("bind endpoint: {e}"))
}

/// Whether to run the iroh endpoint IPv4-only. `ARVOLO_IPV4_ONLY` overrides
/// explicitly (`1`/`true`/`yes` = on, `0`/`false`/`no` = off); otherwise it is
/// auto-detected: a host that has no usable IPv6 route would otherwise advertise
/// a dead IPv6 candidate that peers waste time dialing (see the loopback v6 bind
/// in [`bind_endpoint_with_key`]).
fn ipv4_only() -> bool {
    match std::env::var("ARVOLO_IPV4_ONLY").ok().as_deref() {
        Some("1") | Some("true") | Some("yes") => true,
        Some("0") | Some("false") | Some("no") => false,
        _ => *IPV4_ONLY_AUTO.get_or_init(no_ipv6_route),
    }
}

/// Memoized auto-detection result (the probe is cheap but `bind_endpoint_*` runs
/// per transfer).
static IPV4_ONLY_AUTO: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// True if the host has no usable IPv6 route. Probes by `connect`-ing a UDP
/// socket to a global IPv6 address: `connect` sends no packets, it only checks
/// the routing table, so this is side-effect-free and instant.
fn no_ipv6_route() -> bool {
    use std::net::{Ipv6Addr, SocketAddrV6, UdpSocket};
    let Ok(sock) = UdpSocket::bind(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0)) else {
        return true; // can't even bind v6 -> definitely IPv4-only
    };
    // Cloudflare public DNS over IPv6; any routable global v6 works.
    let target = SocketAddrV6::new(
        Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111),
        443,
        0,
        0,
    );
    sock.connect(target).is_err()
}
