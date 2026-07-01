//! A BLAKE3 content hash — the identity of a chunk's ciphertext.
//!
//! A small newtype over a 32-byte BLAKE3 digest, so the workspace no longer
//! depends on `iroh-blobs` just for its `Hash` type. Serialized as 32 raw bytes
//! (compact in tickets); displayed as lowercase hex (used for relay file names).

use std::fmt;
use std::str::FromStr;

use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};

/// A BLAKE3-256 content hash.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hash([u8; 32]);

impl Hash {
    /// The BLAKE3 hash of `data`.
    pub fn new(data: impl AsRef<[u8]>) -> Self {
        Self(*blake3::hash(data.as_ref()).as_bytes())
    }

    /// The raw 32 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Construct from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&data_encoding::HEXLOWER.encode(&self.0))
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({self})")
    }
}

impl FromStr for Hash {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let bytes = data_encoding::HEXLOWER
            .decode(s.trim().as_bytes())
            .map_err(|e| anyhow!("invalid hash hex: {e}"))?;
        ensure!(bytes.len() == 32, "hash must be 32 bytes");
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Ok(Self(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_and_blake3() {
        let h = Hash::new(b"hello arvolo");
        let s = h.to_string();
        assert_eq!(s.len(), 64);
        assert_eq!(Hash::from_str(&s).unwrap(), h);
        // Matches BLAKE3 of the same input.
        assert_eq!(h.as_bytes(), blake3::hash(b"hello arvolo").as_bytes());
        // Distinct inputs differ.
        assert_ne!(Hash::new(b"a"), Hash::new(b"b"));
    }

    #[test]
    fn rejects_bad_hex() {
        assert!(Hash::from_str("nothex").is_err());
        assert!(Hash::from_str("ab").is_err()); // too short
    }
}
