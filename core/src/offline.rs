//! Offline ticket: everything a recipient needs to fetch and decrypt a blob that
//! was deposited on a relay while they were offline.
//!
//! Bundles the relay URL, the claim token, and the *sender's* public id (needed
//! to verify HPKE auth on open). Encoded as `arvm<base32>` so it pastes as a
//! single string. The HPKE encapsulated key travels separately, returned by the
//! relay alongside the ciphertext.

use anyhow::{anyhow, Context, Result};

const PREFIX: &str = "arvm";
const SEP: u8 = b'|';

/// Pointer to an encrypted blob waiting on a relay for an offline recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineTicket {
    /// Base URL of the relay, e.g. `https://relay.example`.
    pub relay: String,
    /// Claim token identifying the blob on the relay.
    pub claim: String,
    /// The sender's public id (HPKE auth), raw bytes.
    pub sender: Vec<u8>,
}

impl OfflineTicket {
    /// Encode to a single pasteable string (`arvm…`).
    pub fn encode(&self) -> String {
        let mut buf = Vec::new();
        buf.extend_from_slice(self.relay.as_bytes());
        buf.push(SEP);
        buf.extend_from_slice(self.claim.as_bytes());
        buf.push(SEP);
        buf.extend_from_slice(&self.sender);
        format!("{PREFIX}{}", data_encoding::BASE32_NOPAD.encode(&buf))
    }

    /// Parse a string produced by [`OfflineTicket::encode`].
    pub fn decode(s: &str) -> Result<Self> {
        let body = s
            .trim()
            .strip_prefix(PREFIX)
            .ok_or_else(|| anyhow!("not an offline ticket (missing {PREFIX} prefix)"))?;
        let bytes = data_encoding::BASE32_NOPAD
            .decode(body.to_uppercase().as_bytes())
            .context("decode offline ticket")?;
        let mut parts = bytes.splitn(3, |b| *b == SEP);
        let relay = parts.next().context("missing relay")?;
        let claim = parts.next().context("missing claim")?;
        let sender = parts.next().context("missing sender")?;
        Ok(OfflineTicket {
            relay: String::from_utf8(relay.to_vec()).context("relay utf8")?,
            claim: String::from_utf8(claim.to_vec()).context("claim utf8")?,
            sender: sender.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_roundtrips() {
        let t = OfflineTicket {
            relay: "https://relay.example:8787".into(),
            claim: "abc123xyz".into(),
            sender: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        let decoded = OfflineTicket::decode(&t.encode()).unwrap();
        assert_eq!(t, decoded);
    }

    #[test]
    fn rejects_foreign_string() {
        assert!(OfflineTicket::decode("blobxxxx").is_err());
    }
}
