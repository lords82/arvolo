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
    /// A self-hosted iroh relay URL that *replaces* n0 entirely (full sovereignty
    /// — the user's explicit `ARVOLO_IROH_RELAY`).
    Custom(String),
    /// The compiled-in default relay URL, kept alongside n0's relays as fallback:
    /// prefer our own infrastructure but stay reachable if it is down. This is the
    /// zero-config default, where reliability matters more than strict sovereignty.
    BuiltinPlusN0(String),
}

/// The compiled-in default iroh NAT-traversal relay, so a fresh install gets
/// relay-assisted P2P without configuring `ARVOLO_IROH_RELAY`. Overridable at
/// build time with `ARVOLO_DEFAULT_IROH_RELAY`; build with an empty value to fall
/// back to n0's public relays only.
pub const BUILTIN_IROH_RELAY: &str = match option_env!("ARVOLO_DEFAULT_IROH_RELAY") {
    Some(v) => v,
    None => "https://arvolo.duckdns.org:8443",
};

impl RelayChoice {
    /// Resolve the relay set. An explicit `ARVOLO_IROH_RELAY` wins and replaces n0
    /// (full sovereignty). Otherwise use the compiled-in [`BUILTIN_IROH_RELAY`]
    /// with n0 as fallback — or n0 alone if the builtin was compiled out.
    pub fn from_env() -> Self {
        match std::env::var("ARVOLO_IROH_RELAY") {
            Ok(u) if !u.trim().is_empty() => RelayChoice::Custom(u.trim().to_string()),
            _ => {
                let b = BUILTIN_IROH_RELAY.trim();
                if b.is_empty() {
                    RelayChoice::N0Default
                } else {
                    RelayChoice::BuiltinPlusN0(b.to_string())
                }
            }
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
        RelayChoice::BuiltinPlusN0(url) => {
            use iroh::defaults::prod;
            let parsed: iroh::RelayUrl = url
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid default iroh relay url {url:?}: {e}"))?;
            // Our relay is used for relaying only (`quic: None`) — QUIC address
            // discovery is left to n0's nodes, so a closed QUIC port on our VPS
            // never costs a failed probe. n0's relays stay in the map as fallback.
            let map = iroh::RelayMap::from_iter([
                iroh::RelayConfig {
                    url: parsed,
                    quic: None,
                },
                prod::default_na_east_relay(),
                prod::default_na_west_relay(),
                prod::default_eu_relay(),
                prod::default_ap_relay(),
            ]);
            Endpoint::empty_builder(RelayMode::Custom(map))
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
/// the routing table. A bind/connect alone is not enough (macOS defers the route
/// check to send time), so we probe with a single tiny datagram: no route → the
/// send errors immediately. The one stray empty UDP packet to a public DNS server
/// is harmless.
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
    if sock.connect(target).is_err() {
        return true;
    }
    // The actual routing decision happens on send; no route -> immediate error.
    // A one-byte datagram forces the check (a zero-length send is a no-op on some
    // platforms). The stray byte to a public DNS server's QUIC port is harmless.
    sock.send(&[0u8]).is_err()
}
