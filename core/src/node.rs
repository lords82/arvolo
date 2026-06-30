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
    /// Bind with n0 defaults (relay + discovery) so the node is dialable by id
    /// over the internet.
    pub async fn bind() -> Result<Self> {
        let endpoint = Endpoint::builder()
            .secret_key(generate_secret_key())
            .alpns(vec![PING_ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| anyhow::anyhow!("bind endpoint: {e}"))?;
        Ok(Self { endpoint })
    }

    /// Bind for local/direct use only (relay disabled). Used by tests and LAN.
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
    // Random 32 bytes -> ed25519 secret. `from_bytes` accepts any 32 bytes.
    let bytes: [u8; 32] = rand::random();
    SecretKey::from_bytes(&bytes)
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
}
