//! End-to-end payload encryption with **HPKE auth mode** (RFC 9180).
//!
//! Encrypts toward the recipient's public key **and** binds the sender's
//! identity (auth mode), so the recipient learns *who* sent the payload —
//! closing the gap of encrypt-only schemes like plain `age`. A relay or mailbox
//! only ever sees ciphertext.
//!
//! Ciphersuite: X25519-HKDF-SHA256 KEM, HKDF-SHA256 KDF, ChaCha20-Poly1305 AEAD.

use anyhow::{anyhow, Context, Result};
use hpke::{
    aead::ChaCha20Poly1305, kdf::HkdfSha256, kem::X25519HkdfSha256, Deserializable,
    Kem as KemTrait, OpModeR, OpModeS, Serializable,
};

type KemAlg = X25519HkdfSha256;
type AeadAlg = ChaCha20Poly1305;
type KdfAlg = HkdfSha256;

const INFO: &[u8] = b"arvolo/hpke/v1";

/// A long-term identity keypair (X25519). No PII; the public part is the
/// contact id others encrypt toward.
pub struct Identity {
    sk: <KemAlg as KemTrait>::PrivateKey,
    pk: <KemAlg as KemTrait>::PublicKey,
}

/// A contact's public identity (what you encrypt toward / verify as sender).
#[derive(Clone)]
pub struct PublicId(<KemAlg as KemTrait>::PublicKey);

/// HPKE output: the encapsulated key plus the AEAD ciphertext.
pub struct Sealed {
    pub encapped_key: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

impl Identity {
    /// Generate a fresh random identity.
    pub fn generate() -> Self {
        let (sk, pk) = KemAlg::gen_keypair(&mut rand::rng());
        Self { sk, pk }
    }

    /// This identity's public id.
    pub fn public(&self) -> PublicId {
        PublicId(self.pk.clone())
    }

    /// Serialize the secret key (32 bytes). Store this securely.
    pub fn secret_bytes(&self) -> Vec<u8> {
        self.sk.to_bytes().to_vec()
    }

    /// Restore an identity from its secret-key bytes (public key is derived).
    pub fn from_secret_bytes(bytes: &[u8]) -> Result<Self> {
        let sk = <KemAlg as KemTrait>::PrivateKey::from_bytes(bytes)
            .map_err(|e| anyhow!("invalid secret key: {e}"))?;
        let pk = <KemAlg as KemTrait>::sk_to_pk(&sk);
        Ok(Self { sk, pk })
    }

    /// Write the secret key to `path` (owner-only permissions on unix).
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, self.secret_bytes())
            .with_context(|| format!("write identity to {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
        }
        Ok(())
    }

    /// Load an identity from `path`.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("read identity from {}", path.display()))?;
        Self::from_secret_bytes(&bytes)
    }

    /// Load the identity at `path`, creating and saving a new one if absent.
    pub fn load_or_create(path: &std::path::Path) -> Result<Self> {
        if path.exists() {
            Self::load(path)
        } else {
            let id = Self::generate();
            id.save(path)?;
            Ok(id)
        }
    }
}

impl PublicId {
    /// Serialize the public id (32 bytes).
    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.to_bytes().to_vec()
    }

    /// Parse a public id from its bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(PublicId(
            <KemAlg as KemTrait>::PublicKey::from_bytes(bytes)
                .map_err(|e| anyhow!("invalid public id: {e}"))?,
        ))
    }
}

/// Encrypt `plaintext` toward `recipient`, authenticated as `sender`.
/// `aad` is authenticated-but-not-encrypted associated data (e.g. a file name).
pub fn seal(
    plaintext: &[u8],
    recipient: &PublicId,
    sender: &Identity,
    aad: &[u8],
) -> Result<Sealed> {
    let mode = OpModeS::<KemAlg>::Auth((sender.sk.clone(), sender.pk.clone()));
    let (encapped, ciphertext) = hpke::single_shot_seal::<AeadAlg, KdfAlg, KemAlg, _>(
        &mode,
        &recipient.0,
        INFO,
        plaintext,
        aad,
        &mut rand::rng(),
    )
    .map_err(|e| anyhow!("hpke seal: {e}"))?;
    Ok(Sealed {
        encapped_key: encapped.to_bytes().to_vec(),
        ciphertext,
    })
}

/// Decrypt a [`Sealed`] message addressed to `recipient`, verifying it came from
/// `sender`. Fails if the sender doesn't match (auth mode) or on tampering.
pub fn open(
    sealed: &Sealed,
    recipient: &Identity,
    sender: &PublicId,
    aad: &[u8],
) -> Result<Vec<u8>> {
    let mode = OpModeR::<KemAlg>::Auth(sender.0.clone());
    let encapped = <KemAlg as KemTrait>::EncappedKey::from_bytes(&sealed.encapped_key)
        .map_err(|e| anyhow!("invalid encapped key: {e}"))?;
    hpke::single_shot_open::<AeadAlg, KdfAlg, KemAlg>(
        &mode,
        &recipient.sk,
        &encapped,
        INFO,
        &sealed.ciphertext,
        aad,
    )
    .map_err(|e| anyhow!("hpke open (wrong recipient, sender, or tampered): {e}"))
}

// ---- chunk stream encryption ----------------------------------------------
//
// The chunked transfer path (`arvc` tickets) is an ephemeral capability model:
// whoever holds the ticket may receive. We make the relay zero-knowledge by
// encrypting each chunk under a per-transfer random content key that travels
// only inside the ticket (out-of-band). Each chunk is sealed INDEPENDENTLY with
// a nonce derived from its index, so out-of-order multi-source fetch and resume
// keep working — every ciphertext chunk is self-verifying (AEAD tag) and the
// ticket's BLAKE3 hashes address the ciphertext. ChaCha20-Poly1305 AEAD.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305 as ChunkCipher, Key, Nonce};

/// Length of a chunk content key.
pub const CHUNK_KEY_LEN: usize = 32;

/// A fresh random 32-byte content key for one transfer.
pub fn random_chunk_key() -> [u8; CHUNK_KEY_LEN] {
    use rand::RngCore;
    let mut key = [0u8; CHUNK_KEY_LEN];
    rand::rng().fill_bytes(&mut key);
    key
}

/// 12-byte nonce derived from the chunk index (unique per index; the key is
/// fresh-random per transfer, so each (key, nonce) pair is used exactly once).
fn chunk_nonce(index: u32) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..4].copy_from_slice(&index.to_le_bytes());
    n
}

/// AAD binding the chunk's position and the total count, so reordering or
/// truncation is rejected on open.
fn chunk_aad(index: u32, total_chunks: u32) -> [u8; 8] {
    let mut aad = [0u8; 8];
    aad[..4].copy_from_slice(&index.to_le_bytes());
    aad[4..].copy_from_slice(&total_chunks.to_le_bytes());
    aad
}

/// Encrypt one chunk. Output is `plaintext.len() + 16` bytes (Poly1305 tag).
pub fn seal_chunk(
    key: &[u8; CHUNK_KEY_LEN],
    index: u32,
    total_chunks: u32,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let cipher = ChunkCipher::new(Key::from_slice(key));
    let aad = chunk_aad(index, total_chunks);
    cipher
        .encrypt(
            Nonce::from_slice(&chunk_nonce(index)),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("seal chunk {index}"))
}

/// Decrypt one chunk. Fails on wrong key, wrong index, wrong total, or tamper.
pub fn open_chunk(
    key: &[u8; CHUNK_KEY_LEN],
    index: u32,
    total_chunks: u32,
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    let cipher = ChunkCipher::new(Key::from_slice(key));
    let aad = chunk_aad(index, total_chunks);
    cipher
        .decrypt(
            Nonce::from_slice(&chunk_nonce(index)),
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("open chunk {index} (wrong key/index/total or tampered)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip_authenticated() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let msg = b"the eagle lands at midnight";

        let sealed = seal(msg, &bob.public(), &alice, b"file.txt").unwrap();
        let opened = open(&sealed, &bob, &alice.public(), b"file.txt").unwrap();
        assert_eq!(opened, msg);
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let carol = Identity::generate();
        let sealed = seal(b"secret", &bob.public(), &alice, b"").unwrap();
        assert!(open(&sealed, &carol, &alice.public(), b"").is_err());
    }

    #[test]
    fn wrong_sender_fails_auth() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let mallory = Identity::generate();
        let sealed = seal(b"secret", &bob.public(), &alice, b"").unwrap();
        // Bob expects it from Mallory, but Alice sent it -> auth fails.
        assert!(open(&sealed, &bob, &mallory.public(), b"").is_err());
    }

    #[test]
    fn identity_secret_roundtrips() {
        let id = Identity::generate();
        let restored = Identity::from_secret_bytes(&id.secret_bytes()).unwrap();
        assert_eq!(id.public().to_bytes(), restored.public().to_bytes());
    }

    #[test]
    fn tampered_ciphertext_rejected() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let mut sealed = seal(b"secret", &bob.public(), &alice, b"").unwrap();
        sealed.ciphertext[0] ^= 0xff;
        assert!(open(&sealed, &bob, &alice.public(), b"").is_err());
    }

    #[test]
    fn chunk_roundtrip() {
        let key = random_chunk_key();
        let msg = b"a chunk of plaintext data";
        let ct = seal_chunk(&key, 3, 10, msg).unwrap();
        assert_ne!(&ct[..], &msg[..], "ciphertext must differ from plaintext");
        assert_eq!(ct.len(), msg.len() + 16);
        assert_eq!(open_chunk(&key, 3, 10, &ct).unwrap(), msg);
    }

    #[test]
    fn chunk_wrong_key_fails() {
        let ct = seal_chunk(&random_chunk_key(), 0, 1, b"x").unwrap();
        assert!(open_chunk(&random_chunk_key(), 0, 1, &ct).is_err());
    }

    #[test]
    fn chunk_wrong_index_fails() {
        let key = random_chunk_key();
        let ct = seal_chunk(&key, 2, 5, b"payload").unwrap();
        assert!(open_chunk(&key, 3, 5, &ct).is_err());
    }

    #[test]
    fn chunk_wrong_total_fails() {
        let key = random_chunk_key();
        let ct = seal_chunk(&key, 2, 5, b"payload").unwrap();
        assert!(open_chunk(&key, 2, 6, &ct).is_err());
    }

    #[test]
    fn chunk_tampered_fails() {
        let key = random_chunk_key();
        let mut ct = seal_chunk(&key, 0, 1, b"payload").unwrap();
        ct[0] ^= 0xff;
        assert!(open_chunk(&key, 0, 1, &ct).is_err());
    }
}
