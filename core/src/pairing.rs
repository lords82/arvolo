//! Ephemeral pairing via **SPAKE2** (a PAKE).
//!
//! Both sides derive the same shared key from a short human code (e.g.
//! `7-crater-mango`) by exchanging one message each. A wrong code yields a
//! different key on each side, so it never matches — and the code itself never
//! travels on the wire. Zero registration; ideal for one-shot sends.
//!
//! **Status: not yet wired.** This is a tested building block for the planned
//! ephemeral code/QR pairing mode (roadmap M1.2); no CLI flow calls it today.
//! Kept because it's a correct, self-contained primitive to build that feature
//! on — not dead code to remove.

use anyhow::{anyhow, Result};
use spake2::{Ed25519Group, Identity as PakeId, Password, Spake2};

const PAIR_ID: &[u8] = b"arvolo/pair/v1";

/// One side of an in-progress pairing handshake.
pub struct Pairing {
    inner: Spake2<Ed25519Group>,
}

/// Begin a symmetric pairing from a shared `code`. Returns this side's state and
/// the message to send to the peer.
pub fn start(code: &str) -> (Pairing, Vec<u8>) {
    let (inner, outbound) = Spake2::<Ed25519Group>::start_symmetric(
        &Password::new(code.as_bytes()),
        &PakeId::new(PAIR_ID),
    );
    (Pairing { inner }, outbound)
}

impl Pairing {
    /// Complete the handshake with the peer's message, deriving the shared key.
    /// Equal codes on both sides yield an equal key.
    pub fn finish(self, peer_message: &[u8]) -> Result<Vec<u8>> {
        self.inner
            .finish(peer_message)
            .map_err(|e| anyhow!("pairing failed: {e:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_codes_agree() {
        let (a, msg_a) = start("7-crater-mango");
        let (b, msg_b) = start("7-crater-mango");
        let key_a = a.finish(&msg_b).unwrap();
        let key_b = b.finish(&msg_a).unwrap();
        assert_eq!(key_a, key_b, "same code must derive the same key");
        assert!(!key_a.is_empty());
    }

    #[test]
    fn wrong_code_disagrees() {
        let (a, msg_a) = start("7-crater-mango");
        let (b, msg_b) = start("9-wrong-banana");
        let key_a = a.finish(&msg_b).unwrap();
        let key_b = b.finish(&msg_a).unwrap();
        assert_ne!(key_a, key_b, "different codes must not agree");
    }
}
