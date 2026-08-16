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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayChoice {
    /// n0's public relays (shared; suitable for dev/test).
    N0Default,
    /// No relay — direct only (LAN / tests).
    Disabled,
    /// A self-hosted iroh relay URL (production / full sovereignty).
    Custom(String),
}

impl RelayChoice {
    /// Read from `ARVOLO_IROH_RELAY`: `off` for no relay at all (direct/LAN only),
    /// a self-hosted relay URL if set to one, else n0 defaults.
    ///
    /// `off` used to be unreachable from the outside — [`RelayChoice::Disabled`]
    /// existed but only tests could ask for it, so "I don't want my traffic touching
    /// anyone's relay" had no expression short of editing the source.
    pub fn from_env() -> Self {
        Self::parse(&std::env::var("ARVOLO_IROH_RELAY").unwrap_or_default())
    }

    /// The parse behind [`Self::from_env`], separated so it can be tested without
    /// writing to the process environment (which every other test shares).
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if is_off(raw) {
            return RelayChoice::Disabled;
        }
        if raw.is_empty() {
            return RelayChoice::N0Default;
        }
        RelayChoice::Custom(raw.to_string())
    }
}

/// Which iroh **discovery** services to run — separately from the relay choice.
///
/// Discovery is two capabilities that used to be welded together, and to the relay
/// choice as well: n0's preset (`Endpoint::builder()`) installs a `PkarrPublisher`
/// *and* a `DnsDiscovery`, while any other relay choice goes through
/// `empty_builder` and gets neither. So the only way not to publish was to give up
/// resolution and n0's relays too.
///
/// They are worth separating because they cost and buy opposite things:
///
/// - **Publishing** continuously writes a signed record mapping our `EndpointId` —
///   a stable public key — to our current addresses, into a third party's DNS
///   (`iroh.link`), refreshed as we move networks. Arvolo never needs it to *dial*:
///   every connect in this crate takes a full `EndpointAddr` (id + addresses +
///   relay url) out of a ticket or the swarm tracker, never a bare id.
/// - **Resolving** is a fallback that costs us no disclosure: when a ticket's direct
///   addresses have gone stale, iroh can look the id up instead of failing.
///
/// The one thing publishing does buy is [`resume_send`](crate::flow::resume_send):
/// an old ticket reconnects because the rebound node id is *re-resolved* to the new
/// address. With a relay in play the relay itself routes by id and covers that; with
/// [`RelayChoice::Disabled`] it does not, and a re-served ticket cannot be found at a
/// new address. That is the trade [`DiscoveryChoice::ResolveOnly`] asks the user to
/// accept, and why it is not forced on anyone by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryChoice {
    /// n0's DNS/pkarr: publish our own record **and** resolve others'.
    N0,
    /// Resolve through n0's DNS, but never publish a record of our own.
    ResolveOnly,
    /// No discovery at all: dial only what a ticket or the tracker already says.
    Off,
}

impl DiscoveryChoice {
    /// Read from `ARVOLO_IROH_DISCOVERY`: `n0` (publish + resolve), `resolve`
    /// (resolve only, never publish), `off`. Unset keeps the behaviour the relay
    /// choice used to imply on its own, so nothing changes for anyone who has not
    /// asked for it: n0 defaults publish, a custom or disabled relay does not.
    pub fn from_env(relay: &RelayChoice) -> Self {
        Self::parse(
            &std::env::var("ARVOLO_IROH_DISCOVERY").unwrap_or_default(),
            relay,
        )
    }

    /// The parse behind [`Self::from_env`], separated for the same reason as
    /// [`RelayChoice::parse`].
    pub fn parse(raw: &str, relay: &RelayChoice) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "n0" | "on" | "1" | "true" | "yes" | "publish" => DiscoveryChoice::N0,
            "resolve" | "resolve-only" | "resolve_only" | "lookup" => DiscoveryChoice::ResolveOnly,
            s if is_off(s) => DiscoveryChoice::Off,
            // Unset (or unrecognised): whatever this relay choice already did.
            _ => match relay {
                RelayChoice::N0Default => DiscoveryChoice::N0,
                RelayChoice::Custom(_) | RelayChoice::Disabled => DiscoveryChoice::Off,
            },
        }
    }
}

/// The spellings of "no" accepted across the switches here.
fn is_off(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "false" | "no" | "none" | "disabled"
    )
}

/// Whether direct peer-to-peer transport may be used at all.
///
/// `ARVOLO_P2P=off` turns it off, and then every transfer has to go through the
/// relay's mailbox instead. The point is not efficiency, it is that the relay path
/// is **HTTP**: it is the only path [`crate::http`]'s proxy can carry, so it is the
/// only configuration in which neither the relay nor the peer learns your address.
/// A direct QUIC transfer always shows it to the peer, and SOCKS cannot tunnel QUIC
/// — so "hide my address" and "transfer directly" are genuinely exclusive, and this
/// is which of the two you get.
///
/// Enforced at [`bind_endpoint_with_key`], the one place a P2P endpoint comes from,
/// rather than only at the commands that would use one: a path nobody remembered to
/// gate then fails loudly instead of quietly binding a socket the user asked not to
/// have. It is a **client** switch — a relay needs its own endpoint for backfill,
/// and refuses to start if this is set in its environment.
pub fn p2p_enabled() -> bool {
    !is_off(&std::env::var("ARVOLO_P2P").unwrap_or_default())
}

/// Bind an endpoint that speaks our chunk + control ALPNs, with the given relay
/// and a fresh random node id.
pub async fn bind_endpoint(relay: RelayChoice) -> Result<Endpoint> {
    bind_endpoint_with_key(relay, generate_secret_key()).await
}

/// Like [`bind_endpoint`] but with a caller-supplied transport `secret` — so a
/// resumed send can rebind the exact same node id its ticket was issued under.
pub async fn bind_endpoint_with_key(relay: RelayChoice, secret: SecretKey) -> Result<Endpoint> {
    let discovery = DiscoveryChoice::from_env(&relay);
    bind_endpoint_full(relay, discovery, secret, None).await
}

/// Bind an endpoint with everything named explicitly, including its `alpns`
/// (`None` = the chunk + control pair every transfer speaks).
///
/// The single place a P2P socket is created, which is why the `ARVOLO_P2P` refusal
/// lives here — see [`p2p_enabled`].
pub async fn bind_endpoint_full(
    relay: RelayChoice,
    discovery: DiscoveryChoice,
    secret: SecretKey,
    alpns: Option<Vec<Vec<u8>>>,
) -> Result<Endpoint> {
    anyhow::ensure!(
        p2p_enabled(),
        "direct peer-to-peer transport is disabled (ARVOLO_P2P=off / `p2p = false`), \
         so this needs the relay's mailbox instead: send with `--deposit`, and receive \
         the `arvm…` ticket it prints. (A relay must not set ARVOLO_P2P — it needs an \
         endpoint of its own for backfill.)"
    );
    // n0's `Endpoint::builder()` is `empty_builder` plus the N0 preset, i.e. the
    // pkarr publisher, the DNS resolver and n0's relays in one bundle. Build the
    // relay half here and add the discovery half below, so the two are independent.
    let mut builder = match &relay {
        RelayChoice::N0Default => Endpoint::empty_builder(iroh::endpoint::default_relay_mode()),
        RelayChoice::Disabled => Endpoint::empty_builder(RelayMode::Disabled),
        RelayChoice::Custom(url) => {
            let parsed: iroh::RelayUrl = url
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid ARVOLO_IROH_RELAY url {url:?}: {e}"))?;
            Endpoint::empty_builder(RelayMode::Custom(iroh::RelayMap::from(parsed)))
        }
    };
    // Publishing is what a stable public key tied to a moving address costs; add it
    // only when asked. Resolving discloses nothing, so it comes with either of the
    // two "on" settings.
    match discovery {
        DiscoveryChoice::N0 => {
            builder = builder
                .discovery(iroh::discovery::pkarr::PkarrPublisher::n0_dns())
                .discovery(iroh::discovery::dns::DnsDiscovery::n0_dns());
        }
        DiscoveryChoice::ResolveOnly => {
            builder = builder.discovery(iroh::discovery::dns::DnsDiscovery::n0_dns());
        }
        DiscoveryChoice::Off => {}
    }
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
        .transport_config(transport_config())
        .secret_key(secret)
        .alpns(alpns.unwrap_or_else(|| vec![CHUNK_ALPN.to_vec(), CTRL_ALPN.to_vec()]))
        .bind()
        .await
        .map_err(|e| anyhow::anyhow!("bind endpoint: {e}"))
}

/// QUIC transport tuning.
///
/// quinn defaults to Cubic, which recovers by halving and then climbing back over
/// many round trips. That is the wrong shape for us: iroh resets the congestion
/// controller on every path change it observes (see `rtt_actor`), and on a mobile
/// uplink that happens every 40-60 s even when the peer address never changes.
/// Cubic restarts from slow start each time and, at ~100 ms RTT, never reaches the
/// link rate before the next reset — measured at a fifth of what TCP gets on the
/// same path. BBR keeps a bandwidth estimate and re-probes in a couple of round
/// trips instead, so a reset costs far less.
///
/// Measured on one mobile uplink, 60 MB: Cubic 225 s with 8 dropped connections
/// (a second run never finished inside 400 s), BBR 34-53 s with none. On a fat
/// server link, 100 MB: 2.9 MB/s to 3.85 MB/s. Both directions now finish at the
/// link's TCP rate.
///
/// The caveat, stated plainly: quinn labels its BBR "Experimental! Use at your own
/// risk", and BBR is known to take more than its share when sharing a bottleneck
/// with Cubic flows. `ARVOLO_CC=cubic` restores the default — for comparing the two
/// on one path, and as the way back if BBR misbehaves somewhere we have not measured.
fn transport_config() -> iroh::endpoint::TransportConfig {
    let mut cfg = iroh::endpoint::TransportConfig::default();
    // iroh's own default, which supplying a config would otherwise drop.
    cfg.keep_alive_interval(Some(std::time::Duration::from_secs(1)));
    if !matches!(
        std::env::var("ARVOLO_CC").ok().as_deref(),
        Some("cubic") | Some("Cubic")
    ) {
        cfg.congestion_controller_factory(std::sync::Arc::new(
            iroh_quinn_proto::congestion::BbrConfig::default(),
        ));
    }
    cfg
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_choice_reads_off_a_url_and_nothing() {
        assert_eq!(RelayChoice::parse(""), RelayChoice::N0Default);
        assert_eq!(RelayChoice::parse("   "), RelayChoice::N0Default);
        // The case this adds: "no relay at all" was previously unaskable.
        for off in [
            "off", "OFF", "0", "false", "no", "none", "disabled", " off ",
        ] {
            assert_eq!(RelayChoice::parse(off), RelayChoice::Disabled, "{off:?}");
        }
        assert_eq!(
            RelayChoice::parse(" https://relay.example.com "),
            RelayChoice::Custom("https://relay.example.com".to_string()),
            "a url is trimmed, not otherwise touched"
        );
    }

    #[test]
    fn discovery_unset_keeps_exactly_what_the_relay_choice_used_to_imply() {
        // The compatibility promise: nobody's behaviour changes until they ask. n0
        // relays came bundled with the publisher; every other choice came with
        // nothing, because it went through `empty_builder`.
        assert_eq!(
            DiscoveryChoice::parse("", &RelayChoice::N0Default),
            DiscoveryChoice::N0
        );
        assert_eq!(
            DiscoveryChoice::parse("", &RelayChoice::Disabled),
            DiscoveryChoice::Off
        );
        assert_eq!(
            DiscoveryChoice::parse("", &RelayChoice::Custom("https://r.example".into())),
            DiscoveryChoice::Off
        );
        // An unrecognised value is not a silent "off": it falls back to the same
        // default, so a typo cannot quietly cost someone their resume path.
        assert_eq!(
            DiscoveryChoice::parse("resolvee", &RelayChoice::N0Default),
            DiscoveryChoice::N0
        );
    }

    #[test]
    fn discovery_can_be_asked_for_resolution_without_publication() {
        for raw in [
            "resolve",
            "resolve-only",
            "resolve_only",
            "lookup",
            "LOOKUP",
        ] {
            assert_eq!(
                DiscoveryChoice::parse(raw, &RelayChoice::N0Default),
                DiscoveryChoice::ResolveOnly,
                "{raw:?}"
            );
        }
        for raw in ["off", "0", "none"] {
            assert_eq!(
                DiscoveryChoice::parse(raw, &RelayChoice::N0Default),
                DiscoveryChoice::Off,
                "{raw:?}"
            );
        }
        // And publication can be asked for even with a self-hosted relay, which the
        // old welding made impossible.
        assert_eq!(
            DiscoveryChoice::parse("n0", &RelayChoice::Custom("https://r.example".into())),
            DiscoveryChoice::N0
        );
    }

    #[test]
    fn off_is_spelled_the_same_way_everywhere() {
        // One vocabulary across ARVOLO_IROH_RELAY, ARVOLO_IROH_DISCOVERY and
        // ARVOLO_P2P, so "off" never means "on" in one of the three.
        for off in ["off", "0", "false", "no", "none", "disabled", "OFF", " no "] {
            assert!(is_off(off), "{off:?}");
        }
        for on in ["", "1", "true", "yes", "n0", "https://r.example"] {
            assert!(!is_off(on), "{on:?}");
        }
    }
}
