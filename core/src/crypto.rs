//! End-to-end payload encryption with **HPKE auth mode** (RFC 9180).
//!
//! Encrypts toward the recipient's public key **and** binds the sender's
//! identity (auth mode), so the recipient learns *who* sent the payload —
//! closing the gap of encrypt-only schemes like plain `age`. A relay or mailbox
//! only ever sees ciphertext.
//!
//! Ciphersuite: X25519-HKDF-SHA256 KEM, HKDF-SHA256 KDF, AES-256-GCM AEAD.
//!
//! AES-256-GCM (rather than ChaCha20-Poly1305) so the exact same cipher can be
//! decrypted natively in a browser via WebCrypto for the download-link path,
//! keeping one AEAD across the whole codebase. It is equivalent in strength
//! (256-bit AEAD) and hardware-accelerated (AES-NI) on our targets. The nonce
//! discipline below (fresh key per transfer / per password salt) guarantees
//! (key, nonce) is never reused — the one invariant AES-GCM depends on.

use anyhow::{anyhow, Context, Result};
use hpke::{
    aead::AesGcm256, kdf::HkdfSha256, kem::X25519HkdfSha256, Deserializable, Kem as KemTrait,
    OpModeR, OpModeS, Serializable,
};

type KemAlg = X25519HkdfSha256;
type AeadAlg = AesGcm256;
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

impl std::fmt::Debug for PublicId {
    /// Debug as the human fingerprint — never dump raw key bytes into logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PublicId({})", self.fingerprint())
    }
}

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

    /// Number of words in a fingerprint. Eight words = **64 bits** of the
    /// public-key digest. A shorter fingerprint (the old six words, ~48 bits) is
    /// grindable: an active MITM could brute-force a substitute keypair whose
    /// words match the victim's and defeat the out-of-band check, so we widen it to
    /// match the strength of Signal/WhatsApp safety numbers.
    pub const FINGERPRINT_WORDS: usize = 8;

    /// A short, human-comparable fingerprint of this identity: eight words derived
    /// from `BLAKE3` of the public key (**64 bits**). It is a *display aid* for
    /// out-of-band verification ("read me your eight words") — the full base32 id
    /// remains the authoritative value for matching contacts.
    pub fn fingerprint(&self) -> String {
        // Context is versioned (`v2`): the previous `v1` produced six words. The
        // stored authoritative value is the base32 id, not the words, so widening
        // the display fingerprint breaks no persisted data — only what humans read.
        let mut h = blake3::Hasher::new();
        h.update(b"arvolo/fp/v2");
        h.update(&self.to_bytes());
        let digest = h.finalize();
        let bytes = digest.as_bytes();
        bytes[..Self::FINGERPRINT_WORDS]
            .iter()
            .map(|b| crate::wordlist::WORDS[*b as usize])
            .collect::<Vec<_>>()
            .join("-")
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

/// Encrypt `plaintext` toward `recipient` **without** authenticating a sender
/// (HPKE base mode). Used for anonymous challenges — e.g. the relay sealing a
/// proof-of-possession nonce to an inbox owner it can't (and needn't) identify.
pub fn seal_anon(plaintext: &[u8], recipient: &PublicId, aad: &[u8]) -> Result<Sealed> {
    let mode = OpModeS::<KemAlg>::Base;
    let (encapped, ciphertext) = hpke::single_shot_seal::<AeadAlg, KdfAlg, KemAlg, _>(
        &mode,
        &recipient.0,
        INFO,
        plaintext,
        aad,
        &mut rand::rng(),
    )
    .map_err(|e| anyhow!("hpke seal_anon: {e}"))?;
    Ok(Sealed {
        encapped_key: encapped.to_bytes().to_vec(),
        ciphertext,
    })
}

/// Decrypt a base-mode [`Sealed`] addressed to `recipient` (no sender to verify).
/// Succeeding proves possession of `recipient`'s private key — the basis of the
/// inbox proof-of-possession handshake.
pub fn open_anon(sealed: &Sealed, recipient: &Identity, aad: &[u8]) -> Result<Vec<u8>> {
    let mode = OpModeR::<KemAlg>::Base;
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
    .map_err(|e| anyhow!("hpke open_anon (wrong recipient or tampered): {e}"))
}

// ---- chunk stream encryption ----------------------------------------------
//
// The chunked transfer path (`arvc` tickets) is an ephemeral capability model:
// whoever holds the ticket may receive. We make the relay zero-knowledge by
// encrypting each chunk under a per-transfer random content key that travels
// only inside the ticket (out-of-band). Each chunk is sealed INDEPENDENTLY with
// a nonce derived from its index, so out-of-order multi-source fetch and resume
// keep working — every ciphertext chunk is self-verifying (AEAD tag) and the
// ticket's BLAKE3 hashes address the ciphertext. AES-256-GCM AEAD.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm as ChunkCipher, Key, Nonce};

/// Length of a chunk content key.
pub const CHUNK_KEY_LEN: usize = 32;

/// AES-256-GCM authentication tag length appended to each sealed chunk, so a
/// sealed chunk is `plaintext.len() + CHUNK_TAG_LEN` bytes. Lets a streaming
/// receiver frame the ciphertext without a length prefix.
pub const CHUNK_TAG_LEN: usize = 16;

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

/// Encrypt one chunk. Output is `plaintext.len() + 16` bytes (GCM tag).
pub fn seal_chunk(
    key: &[u8; CHUNK_KEY_LEN],
    index: u32,
    total_chunks: u32,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let cipher = ChunkCipher::new(Key::<ChunkCipher>::from_slice(key));
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
    let cipher = ChunkCipher::new(Key::<ChunkCipher>::from_slice(key));
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

// ---- password-wrapped payload (E2E link password) -------------------------
//
// An optional OUTER layer so that holding the link (ticket) is not enough to
// decrypt: the payload is additionally wrapped under a key derived from a shared
// password via Argon2id. The relay only ever stores the wrapped bytes; the salt
// travels in the ticket (it is not secret) and the password is shared
// out-of-band. Without the password the inner ciphertext cannot be recovered, so
// even the intended recipient cannot decrypt — the relay can never bypass it.

use argon2::{Algorithm, Argon2, Params, Version};

/// Length of the random salt used for password key derivation.
pub const PW_SALT_LEN: usize = 16;

/// AAD tag binding the password-wrap layer to this protocol/version.
const PW_AAD: &[u8] = b"arvolo/pw/v1";

/// Explicit, pinned Argon2id cost parameters for the link-password wrap, rather
/// than relying on the library default (which can shift between crate versions and
/// silently change the derived key). Chosen for a hardened offline-guessing cost:
/// 64 MiB memory, 3 passes, 1 lane — comfortably above OWASP's Argon2id minimum
/// and still fast enough on the desktop/CLI targets that do this once per file.
/// The wrap runs only in Rust (send/recv); the browser link path never derives an
/// Argon2 key, so the memory cost is not a WebCrypto concern.
///
/// NOTE: these parameters are part of the on-the-wire contract — a payload wrapped
/// with one set can only be unwrapped with the same. Changing them invalidates
/// previously wrapped links (an accepted breaking change).
const PW_ARGON2_M_COST: u32 = 64 * 1024; // 64 MiB, in KiB
const PW_ARGON2_T_COST: u32 = 3; // iterations
const PW_ARGON2_P_COST: u32 = 1; // lanes

/// The pinned Argon2id instance used for both wrap and unwrap.
fn pw_argon2() -> Result<Argon2<'static>> {
    let params = Params::new(
        PW_ARGON2_M_COST,
        PW_ARGON2_T_COST,
        PW_ARGON2_P_COST,
        Some(CHUNK_KEY_LEN),
    )
    .map_err(|e| anyhow!("argon2 params: {e}"))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// A fresh random salt for password wrapping.
pub fn random_pw_salt() -> [u8; PW_SALT_LEN] {
    use rand::RngCore;
    let mut s = [0u8; PW_SALT_LEN];
    rand::rng().fill_bytes(&mut s);
    s
}

/// Derive a 32-byte wrap key from `password` and `salt` (Argon2id, pinned cost).
///
/// Enforces the salt-length invariant at runtime (not just a `debug_assert!`): the
/// all-zero nonce used by [`wrap_with_password`] is safe **only** because a fresh,
/// sufficiently long random salt makes the derived key unique per payload. A short
/// or empty salt would break that, so we refuse it here in every build.
fn pw_key(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    if salt.len() < PW_SALT_LEN {
        return Err(anyhow!(
            "password-wrap salt must be >= {PW_SALT_LEN} bytes and unique per payload"
        ));
    }
    let mut key = [0u8; CHUNK_KEY_LEN];
    pw_argon2()?
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("password key derivation: {e}"))?;
    Ok(key)
}

/// Wrap `plaintext` under a key derived from `password` + `salt`. `salt` must be
/// random per payload (see [`random_pw_salt`]) and is stored/sent alongside — it
/// is not secret. A fresh salt yields a unique key, so a fixed nonce is safe.
pub fn wrap_with_password(password: &str, salt: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    // The all-zero nonce below is safe ONLY because the key is unique per payload,
    // which holds iff the salt is random and unique (see `random_pw_salt`). The
    // salt-length invariant is enforced at runtime inside `pw_key`.
    let key = pw_key(password, salt)?;
    let cipher = ChunkCipher::new(Key::<ChunkCipher>::from_slice(&key));
    cipher
        .encrypt(
            Nonce::from_slice(&[0u8; 12]),
            Payload {
                msg: plaintext,
                aad: PW_AAD,
            },
        )
        .map_err(|_| anyhow!("wrap payload with password"))
}

/// Reverse of [`wrap_with_password`]. Fails on the wrong password or tampering.
pub fn unwrap_with_password(password: &str, salt: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let key = pw_key(password, salt)?;
    let cipher = ChunkCipher::new(Key::<ChunkCipher>::from_slice(&key));
    cipher
        .decrypt(
            Nonce::from_slice(&[0u8; 12]),
            Payload {
                msg: ciphertext,
                aad: PW_AAD,
            },
        )
        .map_err(|_| anyhow!("wrong password or tampered payload"))
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

    #[test]
    fn password_wrap_roundtrip() {
        let salt = random_pw_salt();
        let msg = b"inner hpke ciphertext";
        let wrapped = wrap_with_password("correct horse", &salt, msg).unwrap();
        assert_ne!(&wrapped[..], &msg[..]);
        assert_eq!(
            unwrap_with_password("correct horse", &salt, &wrapped).unwrap(),
            msg
        );
    }

    #[test]
    fn password_wrong_fails() {
        let salt = random_pw_salt();
        let wrapped = wrap_with_password("right", &salt, b"secret").unwrap();
        assert!(unwrap_with_password("wrong", &salt, &wrapped).is_err());
    }

    #[test]
    fn password_wrong_salt_fails() {
        let wrapped = wrap_with_password("pw", &random_pw_salt(), b"secret").unwrap();
        assert!(unwrap_with_password("pw", &random_pw_salt(), &wrapped).is_err());
    }

    #[test]
    fn password_tampered_fails() {
        let salt = random_pw_salt();
        let mut wrapped = wrap_with_password("pw", &salt, b"secret").unwrap();
        wrapped[0] ^= 0xff;
        assert!(unwrap_with_password("pw", &salt, &wrapped).is_err());
    }

    #[test]
    fn fingerprint_is_stable_and_distinct() {
        let alice = Identity::generate();
        let bob = Identity::generate();

        let fp = alice.public().fingerprint();
        // Dash-separated words, all from the shared wordlist.
        let words: Vec<&str> = fp.split('-').collect();
        assert_eq!(
            words.len(),
            PublicId::FINGERPRINT_WORDS,
            "fingerprint is {} words: {fp}",
            PublicId::FINGERPRINT_WORDS
        );
        assert!(words.iter().all(|w| crate::wordlist::WORDS.contains(w)));

        // Deterministic: same id -> same fingerprint (via a byte roundtrip too).
        let restored = PublicId::from_bytes(&alice.public().to_bytes()).unwrap();
        assert_eq!(restored.fingerprint(), fp);

        // Different identities almost surely differ.
        assert_ne!(bob.public().fingerprint(), fp);
    }
}
