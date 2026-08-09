//! Offline ticket: everything a recipient needs to fetch and decrypt a blob that
//! was deposited on a relay while they were offline.
//!
//! Bundles the relay URL, the claim token, the *sender's* public id (needed to
//! verify HPKE auth on open), and — when the link is password-protected — the
//! (non-secret) key-derivation salt. Encoded as `arvm<base32>` so it pastes as a
//! single string. The HPKE encapsulated key travels separately, returned by the
//! relay alongside the ciphertext.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

const PREFIX: &str = "arvm";

/// Pointer to an encrypted blob waiting on a relay for an offline recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineTicket {
    /// Base URL of the relay, e.g. `https://relay.example`.
    pub relay: String,
    /// Claim token identifying the blob on the relay.
    pub claim: String,
    /// The sender's public id (HPKE auth), raw bytes.
    pub sender: Vec<u8>,
    /// Argon2 salt for the password-wrap layer. Empty ⇒ no link password.
    pub salt: Vec<u8>,
    /// Chunked format: the HPKE-sealed **content key** ciphertext (the relay blob
    /// is then a stream of AES-GCM chunks, not one HPKE blob). Empty ⇒ the legacy
    /// whole-file HPKE format (the blob itself is the sealed file).
    pub wrapped_key: Vec<u8>,
    /// Chunked format: plaintext size of the file, so the receiver can frame the
    /// chunk stream. 0 for the legacy format.
    pub total_size: u64,
    /// The file's name, so the receiver saves it under the name it was sent with.
    ///
    /// Empty for a ticket minted before this field existed — those still decode
    /// (see [`OfflineTicket::decode`]) and still fetch; they just fall back to a
    /// claim-derived name, which is what they always did.
    ///
    /// In the clear inside the ticket, like the chunked ticket's own `name`. The
    /// ticket is the capability to fetch *and* decrypt, so anyone holding it can
    /// read the file anyway — hiding the name from them would protect nothing, and
    /// the relay never sees a ticket.
    pub name: String,
}

/// Postcard wire form — a self-describing, length-prefixed layout so raw byte
/// fields (sender id, salt) can hold any value without a delimiter clash.
#[derive(Serialize, Deserialize)]
struct OfflineWire {
    relay: String,
    claim: String,
    sender: Vec<u8>,
    salt: Vec<u8>,
    wrapped_key: Vec<u8>,
    total_size: u64,
    name: String,
}

/// The shape before `name` was added.
///
/// Postcard is not self-describing and decodes positionally, so a ticket written
/// by an older build simply ends where this struct ends: reading it as
/// [`OfflineWire`] runs off the end and fails. Keeping the old shape lets those
/// tickets go on working — they live for days after being handed out, and an
/// upgrade must not quietly turn one into "not an offline ticket".
#[derive(Deserialize)]
struct OfflineWireV1 {
    relay: String,
    claim: String,
    sender: Vec<u8>,
    salt: Vec<u8>,
    wrapped_key: Vec<u8>,
    total_size: u64,
}

impl OfflineTicket {
    /// True when the link is password-protected (a salt is present).
    pub fn has_password(&self) -> bool {
        !self.salt.is_empty()
    }

    /// Encode to a single pasteable string (`arvm…`).
    pub fn encode(&self) -> String {
        let bytes = postcard::to_allocvec(&OfflineWire {
            relay: self.relay.clone(),
            claim: self.claim.clone(),
            sender: self.sender.clone(),
            salt: self.salt.clone(),
            wrapped_key: self.wrapped_key.clone(),
            total_size: self.total_size,
            name: self.name.clone(),
        })
        .expect("serialize offline ticket");
        format!("{PREFIX}{}", data_encoding::BASE32_NOPAD.encode(&bytes))
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
        // Newest shape first; an older ticket falls short of it and is read with
        // the shape it was written in, with no name to offer.
        if let Ok(w) = postcard::from_bytes::<OfflineWire>(&bytes) {
            return Ok(OfflineTicket {
                relay: w.relay,
                claim: w.claim,
                sender: w.sender,
                salt: w.salt,
                wrapped_key: w.wrapped_key,
                total_size: w.total_size,
                name: w.name,
            });
        }
        let w: OfflineWireV1 =
            postcard::from_bytes(&bytes).context("deserialize offline ticket")?;
        Ok(OfflineTicket {
            relay: w.relay,
            claim: w.claim,
            sender: w.sender,
            salt: w.salt,
            wrapped_key: w.wrapped_key,
            total_size: w.total_size,
            name: String::new(),
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
            salt: Vec::new(),
            wrapped_key: vec![9, 8, 7, 6],
            total_size: 1_234_567,
            name: "report.pdf".into(),
        };
        let decoded = OfflineTicket::decode(&t.encode()).unwrap();
        assert_eq!(t, decoded);
        assert!(!decoded.has_password());
    }

    #[test]
    fn ticket_with_password_roundtrips() {
        // Salt (and the pipe byte, once a delimiter) must survive intact.
        let t = OfflineTicket {
            relay: "https://relay.example".into(),
            claim: "claim".into(),
            sender: vec![0xff, b'|', 0x00, 0x7c],
            salt: vec![1, b'|', 2, 3, 0xff, 0x00, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9],
            wrapped_key: vec![b'|', 0x00, 0xff],
            total_size: 0,
            name: "conti 2026.xlsx".into(),
        };
        let decoded = OfflineTicket::decode(&t.encode()).unwrap();
        assert_eq!(t, decoded);
        assert!(decoded.has_password());
    }

    #[test]
    fn rejects_foreign_string() {
        assert!(OfflineTicket::decode("blobxxxx").is_err());
    }
}
