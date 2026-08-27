//! iroh-backed node: bind an endpoint, dial peers by id, exchange a ping.
//!
//! Step 0.2 wires iroh in behind the crate. The blob transfer (`Transport`
//! trait) lands in Step 0.3 on top of `iroh-blobs`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use anyhow::{Context, Result};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey};

/// ALPN for the liveness ping protocol.
pub const PING_ALPN: &[u8] = b"arvolo/ping/0";

const PING: &[u8; 4] = b"ping";
const PONG: &[u8; 4] = b"pong";

/// A bound arvolo node wrapping an iroh [`Endpoint`].
pub struct Node {
    endpoint: Endpoint,
}

impl Node {
    /// Bind a node dialable over the internet, honouring the configured relay and
    /// discovery.
    ///
    /// It used to call `Endpoint::builder()` directly, which is n0's preset: n0
    /// relays *and* the pkarr publisher, whatever the user had configured. Nothing
    /// but the ping test reaches this today, so it leaked nothing in practice — but a
    /// binding function that quietly ignores the settings every other binding
    /// function honours is a trap for whoever wires it into a real path next.
    pub async fn bind() -> Result<Self> {
        let relay = crate::transfer::RelayChoice::from_env();
        let discovery = crate::transfer::DiscoveryChoice::from_env(&relay);
        let endpoint = crate::transfer::bind_endpoint_full(
            relay,
            discovery,
            generate_secret_key(),
            Some(vec![PING_ALPN.to_vec()]),
        )
        .await?;
        Ok(Self { endpoint })
    }

    /// Bind for local/direct use only (relay and discovery both off). Used by tests
    /// and LAN.
    pub async fn bind_local() -> Result<Self> {
        let endpoint = Endpoint::empty_builder(RelayMode::Disabled)
            .secret_key(generate_secret_key())
            .alpns(vec![PING_ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| anyhow::anyhow!("bind endpoint: {e}"))?;
        Ok(Self { endpoint })
    }

    /// This node's stable id (its public key).
    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// The underlying iroh endpoint.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// An [`EndpointAddr`] dialable on the same host: id + bound ports with any
    /// unspecified bind address rewritten to loopback.
    pub fn local_addr(&self) -> EndpointAddr {
        local_addr_of(&self.endpoint)
    }

    /// Accept a single ping connection and reply with a pong.
    pub async fn serve_one_ping(&self) -> Result<EndpointId> {
        let incoming = self.endpoint.accept().await.context("endpoint closed")?;
        let conn = incoming.await.map_err(|e| anyhow::anyhow!("accept: {e}"))?;
        let remote = conn.remote_id();
        let (mut send, mut recv) = conn
            .accept_bi()
            .await
            .map_err(|e| anyhow::anyhow!("accept_bi: {e}"))?;
        let mut buf = [0u8; 4];
        recv.read_exact(&mut buf)
            .await
            .map_err(|e| anyhow::anyhow!("read ping: {e}"))?;
        anyhow::ensure!(&buf == PING, "unexpected payload: {buf:?}");
        send.write_all(PONG)
            .await
            .map_err(|e| anyhow::anyhow!("write pong: {e}"))?;
        send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
        conn.closed().await;
        Ok(remote)
    }

    /// Dial `addr` and exchange a ping/pong. Returns Ok if pong received.
    pub async fn ping(&self, addr: EndpointAddr) -> Result<()> {
        let conn = self
            .endpoint
            .connect(addr, PING_ALPN)
            .await
            .map_err(|e| anyhow::anyhow!("connect: {e}"))?;
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("open_bi: {e}"))?;
        send.write_all(PING)
            .await
            .map_err(|e| anyhow::anyhow!("write ping: {e}"))?;
        send.finish().map_err(|e| anyhow::anyhow!("finish: {e}"))?;
        let mut buf = [0u8; 4];
        recv.read_exact(&mut buf)
            .await
            .map_err(|e| anyhow::anyhow!("read pong: {e}"))?;
        anyhow::ensure!(&buf == PONG, "unexpected reply: {buf:?}");
        conn.close(0u32.into(), b"bye");
        Ok(())
    }

    /// Gracefully close the endpoint.
    pub async fn close(self) {
        self.endpoint.close().await;
    }
}

/// A shareable connection ticket: a serialized [`EndpointAddr`] (base32, no pad).
pub fn encode_ticket(addr: &EndpointAddr) -> Result<String> {
    let bytes = postcard::to_allocvec(addr).context("serialize addr")?;
    Ok(data_encoding::BASE32_NOPAD.encode(&bytes))
}

/// Parse a ticket produced by [`encode_ticket`].
pub fn decode_ticket(ticket: &str) -> Result<EndpointAddr> {
    let bytes = data_encoding::BASE32_NOPAD
        .decode(ticket.trim().to_ascii_uppercase().as_bytes())
        .context("decode ticket")?;
    postcard::from_bytes(&bytes).context("deserialize addr")
}

/// Generate a fresh random node secret key.
pub(crate) fn generate_secret_key() -> SecretKey {
    SecretKey::from_bytes(&random_node_seed())
}

/// A fresh random 32-byte seed for a node secret key. Kept as raw bytes so a
/// send can persist it and later rebind the *same* node id (stable transport
/// identity for the life of one transfer; see `flow::resume_send`).
pub(crate) fn random_node_seed() -> [u8; 32] {
    rand::random()
}

/// Rebuild a node secret key from a persisted [`random_node_seed`].
pub(crate) fn secret_key_from_seed(seed: &[u8; 32]) -> SecretKey {
    SecretKey::from_bytes(seed)
}

/// The address to hand a *remote* peer: everything iroh knows about us minus the
/// loopback candidates.
///
/// `bind_endpoint_with_key` parks the IPv6 socket on `[::1]` when the host has no
/// IPv6 route, so `Endpoint::addr()` reports `[::1]:port` as a direct address and
/// it travels in the ticket. The peer then probes it forever — the probe leaves
/// their machine and arrives at *their own* loopback, where nothing answers. Never
/// useful off-host, so it does not belong in anything we publish. Same-host dialing
/// uses [`local_addr_of`], which keeps loopback on purpose.
pub(crate) fn remote_addr_of(endpoint: &iroh::Endpoint) -> EndpointAddr {
    strip_loopback(endpoint.addr())
}

/// Drop loopback IP candidates, keeping relays and every routable address.
fn strip_loopback(mut addr: EndpointAddr) -> EndpointAddr {
    addr.addrs.retain(|a| match a {
        iroh::TransportAddr::Ip(sock) => !sock.ip().is_loopback(),
        _ => true,
    });
    addr
}

/// How long to wait for a relay to make us reachable from anywhere, before
/// publishing what we have.
///
/// Generous, because coming online through a relay normally takes a second or
/// two and a ticket without a relay address is worth less: it reaches a peer on
/// this network and nobody else.
pub(crate) const RELAY_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The address to put in a ticket, and whether we had to settle for less.
///
/// `iroh::Endpoint::online()` waits for a **home relay** and nothing else — it is
/// `home_relay().initialized().await`, with no timeout. On a network where no
/// relay is reachable (a captive Wi-Fi, a firewall that eats everything but one
/// port) it therefore never returns, and awaiting it bare is what left a send
/// sitting in "preparing" for as long as the user cared to watch: the file was
/// read and encrypted in milliseconds, and the wait that followed was for a
/// relay, forever, with a word on screen that says local work.
///
/// So: wait, but not for ever. What comes back second is `true` when the wait ran
/// out — the endpoint is reachable on this network and nowhere else, which is
/// worth saying out loud and is emphatically not the same as failing. A recipient
/// on the same LAN is reached perfectly well by the direct addresses.
pub(crate) async fn published_addr(
    endpoint: &iroh::Endpoint,
    use_relay: bool,
) -> (EndpointAddr, bool) {
    // No relay configured: there is nothing to become reachable *through*, so the
    // wait would be for something that cannot arrive. Same-host dialing.
    if !use_relay {
        return (local_addr_of(endpoint), false);
    }
    if tokio::time::timeout(RELAY_WAIT, endpoint.online())
        .await
        .is_ok()
    {
        return (remote_addr_of(endpoint), false);
    }
    tracing::warn!(
        "no relay became reachable in {}s — publishing this network's addresses only",
        RELAY_WAIT.as_secs()
    );
    // Whatever iroh does know about us — on a LAN, the interface addresses, which
    // are the whole answer for a peer in the same building — *and* loopback.
    //
    // Loopback is stripped from a ticket that will travel, and rightly: a peer
    // elsewhere would probe its own machine for ever. This ticket is not
    // travelling anywhere and says so. What is left is a peer on this network or
    // one on this very machine — two instances, a daemon and a CLI, a test — and
    // for the second, loopback is the only address that works. It is also the only
    // one that survives a host firewall, which on macOS drops the LAN path for an
    // unsigned binary while leaving loopback alone. Candidates are probed in
    // parallel, so the extra one costs a peer on the LAN nothing.
    let mut addr = remote_addr_of(endpoint);
    for sock in endpoint.bound_sockets() {
        addr = addr.with_ip_addr(loopback_if_unspecified(sock));
    }
    (addr, true)
}

/// Build an [`EndpointAddr`] dialable on the same host (loopback + bound ports).
pub(crate) fn local_addr_of(endpoint: &iroh::Endpoint) -> EndpointAddr {
    let mut addr = EndpointAddr::new(endpoint.id());
    for sock in endpoint.bound_sockets() {
        addr = addr.with_ip_addr(loopback_if_unspecified(sock));
    }
    addr
}

fn loopback_if_unspecified(sock: SocketAddr) -> SocketAddr {
    match sock.ip() {
        IpAddr::V4(ip) if ip == Ipv4Addr::UNSPECIFIED => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), sock.port())
        }
        IpAddr::V6(ip) if ip == Ipv6Addr::UNSPECIFIED => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), sock.port())
        }
        _ => sock,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_roundtrips() {
        let addr = EndpointAddr::new(generate_secret_key().public())
            .with_ip_addr("127.0.0.1:4242".parse().unwrap());
        let t = encode_ticket(&addr).unwrap();
        let back = decode_ticket(&t).unwrap();
        assert_eq!(addr, back);
    }

    /// A loopback candidate in a published ticket makes the peer probe its *own*
    /// loopback forever. `ARVOLO_IPV4_ONLY` parks the v6 socket there, so this is
    /// reachable in practice, not hypothetical.
    #[test]
    fn a_published_address_carries_no_loopback() {
        let addr = EndpointAddr::new(generate_secret_key().public())
            .with_ip_addr("127.0.0.1:4242".parse().unwrap())
            .with_ip_addr("[::1]:4243".parse().unwrap())
            .with_ip_addr("192.168.1.9:4244".parse().unwrap())
            .with_ip_addr("95.75.91.227:4245".parse().unwrap());

        let published = strip_loopback(addr);

        let mut kept: Vec<_> = published
            .addrs
            .iter()
            .filter_map(|a| match a {
                iroh::TransportAddr::Ip(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        kept.sort();
        // The LAN address stays: it is how two peers on one network find each other.
        assert_eq!(kept, vec!["192.168.1.9:4244", "95.75.91.227:4245"]);
    }

    /// The wait that had no end.
    ///
    /// A relay nobody answers on is what a closed network looks like from inside:
    /// captive Wi-Fi, a firewall that passes one port, a hotel. `online()` waits
    /// for a home relay and nothing else, so awaiting it bare never came back —
    /// and a send sat in "preparing" with its file encrypted and ready, for as
    /// long as anyone cared to watch. What this pins is that the wait ends, and
    /// that it ends with something dialable: on a LAN those addresses are the
    /// whole answer.
    ///
    /// Costs the real wait, once. Shortening it for the test would leave the
    /// number that matters — the one shipped to people — untested.
    #[tokio::test]
    async fn a_relay_that_never_answers_does_not_hold_the_address_for_ever() {
        // An endpoint with no relay at all, asked for the address of one that
        // wants a relay — which is what "a relay that never answers" reduces to
        // here, and reaches the branch without a single packet leaving the
        // machine. (Pointing it at a dead URL instead exercises iroh's retry
        // loop for a minute and leaves its tasks behind.)
        let endpoint =
            match crate::transfer::bind_endpoint(crate::transfer::RelayChoice::Disabled).await {
                Ok(e) => e,
                // P2P off in this environment: there is no endpoint to test with, and
                // that refusal has its own test.
                Err(_) => return,
            };

        let started = std::time::Instant::now();
        let (addr, local_only) = published_addr(&endpoint, true).await;
        let waited = started.elapsed();

        assert!(
            local_only,
            "no relay answered — the caller has to be told, or it cannot say why"
        );
        assert!(
            waited < RELAY_WAIT * 2,
            "the wait must be bounded; it took {waited:?}"
        );
        assert!(
            !addr.addrs.is_empty(),
            "something dialable has to come back, or the send cannot start at all"
        );
        endpoint.close().await;
    }
}
