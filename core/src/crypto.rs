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
use ed25519_dalek::{SigningKey, VerifyingKey};
use hpke::{
    aead::AesGcm256, kdf::HkdfSha256, kem::X25519HkdfSha256, Deserializable, Kem as KemTrait,
    OpModeR, OpModeS, Serializable,
};

type KemAlg = X25519HkdfSha256;
type AeadAlg = AesGcm256;
type KdfAlg = HkdfSha256;

const INFO: &[u8] = b"arvolo/hpke/v1";

/// Domain separators deriving the two halves of an identity from its seed. Two
/// contexts, so the halves are independent: neither can be computed from the other,
/// and no result about one carries over to the other.
const ED_CONTEXT: &str = "arvolo/identity/ed25519/v1";
const X_CONTEXT: &str = "arvolo/identity/x25519/v1";

/// Bytes of one half of a public id.
const HALF: usize = 32;

/// Marks a stored identity file as holding a seed for the two-key identity.
const IDENTITY_MAGIC: &[u8] = b"arvolo-identity-v2\n";

/// A long-term identity: **two** keypairs derived from one stored seed.
///
/// The signing half (Ed25519) names you and proves you own your inbox; the KEM half
/// (X25519) is what messages are encrypted toward. They are derived from the seed
/// through separate KDF contexts and are mathematically unrelated.
///
/// They could have been one key — Ed25519 and X25519 are the same curve in different
/// coordinates, and converting between them is a standard, widely deployed move
/// (libsodium ships the conversion for exactly this). It was not taken. Sharing a
/// key across a signature scheme and a KEM is *believed* safe and has been analysed
/// as such, but "believed safe" is an assumption this product otherwise does not
/// ask anyone to accept; and the birational map drops the sign bit, so `A` and `-A`
/// — two distinct contact ids — would share one encryption key. The price of
/// avoiding both is a contact id of 64 bytes instead of 32. Since the eight-word
/// fingerprint people actually read aloud is unchanged, that price is paid by
/// clipboards and QR codes, not by users.
pub struct Identity {
    /// The stored secret: everything else here derives from it.
    seed: [u8; HALF],
    signing: SigningKey,
    sk: <KemAlg as KemTrait>::PrivateKey,
    pk: <KemAlg as KemTrait>::PublicKey,
}

/// A contact's public identity: the signing half then the KEM half, 64 bytes.
#[derive(Clone)]
pub struct PublicId {
    ed: VerifyingKey,
    x: <KemAlg as KemTrait>::PublicKey,
}

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

impl Drop for Identity {
    /// Zero the seed when the identity leaves memory. The derived keys already do
    /// this for themselves (ed25519-dalek and hpke both zeroize on drop); the raw
    /// seed was the one copy that lingered — and it is the *whole* secret, from
    /// which both halves re-derive.
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.seed.zeroize();
    }
}

impl Identity {
    /// Generate a fresh random identity.
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut seed = [0u8; HALF];
        rand::rng().fill_bytes(&mut seed);
        Self::from_seed(seed)
    }

    /// Derive both keypairs from `seed`. Total and deterministic: the seed is the
    /// whole secret, which is what makes the stored file 32 bytes and lets two
    /// devices share an identity by sharing it.
    fn from_seed(seed: [u8; HALF]) -> Self {
        let signing = SigningKey::from_bytes(&blake3::derive_key(ED_CONTEXT, &seed));
        // `PrivateKey::from_bytes` for this KEM stores the scalar and clamps it where
        // the curve requires it, so any 32 bytes are a usable secret.
        let sk =
            <KemAlg as KemTrait>::PrivateKey::from_bytes(&blake3::derive_key(X_CONTEXT, &seed))
                .expect("32 bytes is always a valid X25519 secret");
        let pk = <KemAlg as KemTrait>::sk_to_pk(&sk);
        Self {
            seed,
            signing,
            sk,
            pk,
        }
    }

    /// This identity's public id.
    pub fn public(&self) -> PublicId {
        PublicId {
            ed: self.signing.verifying_key(),
            x: self.pk.clone(),
        }
    }

    /// The signing half, for the inbox proof of possession (see [`blinded`]).
    pub(crate) fn signing(&self) -> &SigningKey {
        &self.signing
    }

    /// Serialize the secret (32 bytes: the seed). Store this securely.
    pub fn secret_bytes(&self) -> Vec<u8> {
        self.seed.to_vec()
    }

    /// Restore an identity from its stored secret (both keypairs are re-derived).
    pub fn from_secret_bytes(bytes: &[u8]) -> Result<Self> {
        let seed: [u8; HALF] = bytes
            .try_into()
            .map_err(|_| anyhow!("invalid identity secret: expected {HALF} bytes"))?;
        Ok(Self::from_seed(seed))
    }

    /// Write the secret to `path` (owner-only permissions on unix).
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut body = IDENTITY_MAGIC.to_vec();
        body.extend_from_slice(&self.seed);
        std::fs::write(path, body)
            .with_context(|| format!("write identity to {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
        }
        Ok(())
    }

    /// Load an identity from `path`.
    ///
    /// The magic prefix exists to make one specific mistake impossible. An identity
    /// file written before the key change is 32 bare bytes — exactly the shape of a
    /// seed — so reading it would succeed and hand back a *different* identity, with
    /// every saved contact silently no longer recognising you and no error anywhere
    /// to explain it. Refusing it by name costs a dozen bytes on disk.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("read identity from {}", path.display()))?;
        let seed = bytes.strip_prefix(IDENTITY_MAGIC).with_context(|| {
            format!(
                "{} is not an arvolo identity of this version. An identity from before \
                 the signing key was added cannot be converted — its id is a different \
                 id. Delete the file to generate a new identity (you will need to \
                 re-exchange contacts).",
                path.display()
            )
        })?;
        Self::from_secret_bytes(seed)
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
    /// Serialize the public id (64 bytes: signing half, then KEM half).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 * HALF);
        out.extend_from_slice(self.ed.as_bytes());
        out.extend_from_slice(&self.x.to_bytes());
        out
    }

    /// The signing half's bytes — what an inbox slot is derived from.
    pub fn ed_bytes(&self) -> [u8; HALF] {
        *self.ed.as_bytes()
    }

    /// The signing half as a verifying key.
    pub(crate) fn verifying(&self) -> &VerifyingKey {
        &self.ed
    }

    /// Parse a public id from its bytes.
    ///
    /// Stricter than it used to be, and deliberately so. Every 32-byte string is a
    /// valid X25519 key, so the old parse could not fail on a wrong id; roughly half
    /// of all strings are not valid Edwards points, so a corrupted or truncated id
    /// now fails here instead of halfway through a transfer.
    ///
    /// The torsion check is load-bearing rather than hygiene. Clamped X25519 scalars
    /// clear the cofactor on every multiplication, which quietly neutralised
    /// small-order components; the per-epoch blinding factor is a full-range scalar
    /// and does no such thing. A key outside the prime-order subgroup has to be
    /// refused at the door, because nothing downstream will do it for us.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 2 * HALF {
            anyhow::bail!(
                "invalid public id: expected {} bytes, got {}",
                2 * HALF,
                bytes.len()
            );
        }
        let ed_bytes: [u8; HALF] = bytes[..HALF].try_into().expect("checked length");
        let ed = VerifyingKey::from_bytes(&ed_bytes)
            .map_err(|e| anyhow!("invalid public id (signing half): {e}"))?;
        if !ed.to_edwards().is_torsion_free() {
            anyhow::bail!("invalid public id: signing half is outside the prime-order subgroup");
        }
        let x = <KemAlg as KemTrait>::PublicKey::from_bytes(&bytes[HALF..])
            .map_err(|e| anyhow!("invalid public id (KEM half): {e}"))?;
        Ok(PublicId { ed, x })
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

/// Per-epoch **blinded** keys: how you prove you own an inbox without telling the
/// relay whose it is.
///
/// The proof used to be a round trip through HPKE: you sent the relay your public
/// key, it sealed a nonce to it, and opening the nonce was the proof. That works,
/// and it hands the relay your long-term identity on every session — from which it
/// can recompute every slot you will ever have, in any epoch. Rotating the slots
/// kept the stable identifier out of the *logs*; it never hid anything from the
/// relay process itself.
///
/// Here the slot **is** a public key: the identity's signing half, blinded for the
/// epoch. The relay decodes the slot, sends a nonce, and checks a signature. No key
/// is transmitted, and the relay cannot link one epoch's slot to another's without
/// already knowing the identity — which is the thing it no longer learns.
///
/// The construction is Tor's v3 onion-key blinding:
///
/// ```text
///   h  = H(A, epoch)          the blinding factor, public
///   A' = h·A                  anyone holding A can compute it → this is the slot
///   a' = h·a                  only the owner, who has a
/// ```
///
/// `a'` is a product reduced mod ℓ: it is **not** a clamped scalar, and must not be.
/// Clamping it would break `[a']G = A'` and every signature would fail to verify —
/// which is why signing goes through `hazmat`, the one API that takes a scalar as
/// given. That is also why the identity's *other* half stays clamped: it does
/// Diffie-Hellman against keys strangers choose, where clearing the cofactor is what
/// keeps small-subgroup attacks out. Two scalars, opposite requirements, and neither
/// is a matter of taste.
pub mod blinded {
    use super::{Identity, PublicId};
    use curve25519_dalek::scalar::Scalar;
    use ed25519_dalek::hazmat::{raw_sign, ExpandedSecretKey};
    use ed25519_dalek::{Sha512, Signature, VerifyingKey};

    /// Domain separator for the blinding factor.
    const BLIND_CONTEXT: &str = "arvolo/inbox/blind/v1";
    /// Domain separator for the per-epoch signing nonce prefix.
    const PREFIX_CONTEXT: &str = "arvolo/inbox/blind-prefix/v1";

    /// Bytes of a blinded public key, i.e. of a slot.
    pub const KEY_LEN: usize = 32;
    /// Bytes of a signature.
    pub const SIG_LEN: usize = 64;

    /// `h` for this identity and epoch. Uniform mod ℓ: 64 hash bytes reduced, rather
    /// than 32 interpreted, so the factor has no bias a shorter draw would leave.
    fn factor(ed_pub: &[u8; KEY_LEN], epoch: u64) -> Scalar {
        let mut h = blake3::Hasher::new_derive_key(BLIND_CONTEXT);
        h.update(ed_pub);
        h.update(&epoch.to_le_bytes());
        let mut wide = [0u8; 64];
        h.finalize_xof().fill(&mut wide);
        Scalar::from_bytes_mod_order_wide(&wide)
    }

    /// The blinded public key of `id` at `epoch` — computable by anyone who knows
    /// the contact id, which is what lets a sender address an inbox it cannot read.
    pub fn public_at(id: &PublicId, epoch: u64) -> [u8; KEY_LEN] {
        let h = factor(&id.ed_bytes(), epoch);
        (id.verifying().to_edwards() * h).compress().to_bytes()
    }

    /// Sign `msg` as the owner of this epoch's slot.
    ///
    /// The verifying key handed to `raw_sign` is the *blinded* one. Passing the
    /// long-term key instead would not merely fail to verify: signing against a key
    /// that does not match the scalar is the documented way to leak the signing key
    /// outright. The pairing is established here, in one place, and the tests check
    /// both halves of it.
    pub fn sign_at(me: &Identity, epoch: u64, msg: &[u8]) -> [u8; SIG_LEN] {
        let public = me.public();
        let h = factor(&public.ed_bytes(), epoch);
        let blinded_point = public.verifying().to_edwards() * h;
        let vk = VerifyingKey::from(blinded_point);

        // A fresh prefix per epoch rather than the identity's own: it is what seeds
        // the signature nonce, and there is no reason to carry one value across keys
        // that are meant to be unlinkable.
        let mut prefix_input = me.signing().to_bytes().to_vec();
        prefix_input.extend_from_slice(&epoch.to_le_bytes());
        let esk = ExpandedSecretKey {
            scalar: h * me.signing().to_scalar(),
            hash_prefix: blake3::derive_key(PREFIX_CONTEXT, &prefix_input),
        };
        raw_sign::<Sha512>(&esk, msg, &vk).to_bytes()
    }

    /// Check a signature against a slot. `slot_key` is the blinded public key the
    /// slot encodes; nothing here needs to know which identity it belongs to.
    pub fn verify(slot_key: &[u8], msg: &[u8], sig: &[u8]) -> bool {
        let Ok(key_bytes) = <[u8; KEY_LEN]>::try_from(slot_key) else {
            return false;
        };
        let Ok(sig_bytes) = <[u8; SIG_LEN]>::try_from(sig) else {
            return false;
        };
        let Ok(vk) = VerifyingKey::from_bytes(&key_bytes) else {
            return false;
        };
        // `verify_strict`: rejects small-order keys and non-canonical encodings, so a
        // slot nobody could own cannot be authenticated for.
        vk.verify_strict(msg, &Signature::from_bytes(&sig_bytes))
            .is_ok()
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
        &recipient.x,
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
    let mode = OpModeR::<KemAlg>::Auth(sender.x.clone());
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
/// (HPKE base mode). The outer layer of a sealed-sender offer: the relay sees an
/// opaque blob and cannot tell who deposited it.
pub fn seal_anon(plaintext: &[u8], recipient: &PublicId, aad: &[u8]) -> Result<Sealed> {
    let mode = OpModeS::<KemAlg>::Base;
    let (encapped, ciphertext) = hpke::single_shot_seal::<AeadAlg, KdfAlg, KemAlg, _>(
        &mode,
        &recipient.x,
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
///
/// Now used only for the sealed-sender envelope around an offer. The inbox
/// proof-of-possession used to run through here too — the relay sealed a nonce to
/// the reader's key — and that is precisely what carried the reader's identity to
/// the relay; it is a signature against the slot's own key now (see [`blinded`]).
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

    // ---- the two-key identity ---------------------------------------------

    /// The seed is the whole secret, and it determines both halves. This is what
    /// lets two devices share an identity by sharing 32 bytes.
    #[test]
    fn the_seed_determines_the_whole_identity() {
        let a = Identity::generate();
        let b = Identity::from_secret_bytes(&a.secret_bytes()).unwrap();
        assert_eq!(
            a.secret_bytes().len(),
            32,
            "the stored secret stays 32 bytes"
        );
        assert_eq!(a.public().to_bytes(), b.public().to_bytes());
        // And the KEM half really is usable by the re-derived copy, not just equal
        // in bytes: seal to one, open with the other.
        let sealed = seal(b"hello", &a.public(), &b, b"").unwrap();
        assert_eq!(open(&sealed, &b, &a.public(), b"").unwrap(), b"hello");
    }

    /// A public id is the two halves, in order, and nothing else.
    #[test]
    fn a_public_id_is_two_halves() {
        let id = Identity::generate();
        let bytes = id.public().to_bytes();
        assert_eq!(bytes.len(), 64);
        assert_eq!(&bytes[..32], &id.public().ed_bytes());
        let back = PublicId::from_bytes(&bytes).unwrap();
        assert_eq!(back.to_bytes(), bytes);
    }

    /// The property the whole two-key choice was made for: the KEM half is **not**
    /// the signing half in disguise. If these were ever equal, the identity would be
    /// back to one key doing two jobs — which is exactly the assumption this design
    /// declines to make — and the failure would be invisible from the outside.
    #[test]
    fn the_kem_half_is_not_derived_from_the_signing_half() {
        for _ in 0..8 {
            let id = Identity::generate();
            let pk = id.public();
            let converted = pk.verifying().to_montgomery().to_bytes();
            assert_ne!(
                converted,
                pk.to_bytes()[32..],
                "the KEM half must not be the birational image of the signing half"
            );
        }
    }

    /// Ids are now parsed, not merely accepted. Every 32-byte string was a valid
    /// X25519 key, so the old id could not be wrong; half of all strings are not
    /// Edwards points, so a mangled id fails here instead of much later.
    #[test]
    fn a_malformed_public_id_is_refused() {
        let good = Identity::generate().public().to_bytes();
        assert!(PublicId::from_bytes(&good[..63]).is_err(), "short");
        assert!(
            PublicId::from_bytes(&[good.clone(), vec![0u8]].concat()).is_err(),
            "long"
        );
        // Corrupt the signing half until we hit a non-point (most bytes are not).
        let mut bad = good.clone();
        let broken = (1u8..=255).any(|b| {
            bad[31] = b;
            PublicId::from_bytes(&bad).is_err()
        });
        assert!(
            broken,
            "no corruption of the signing half was ever rejected"
        );
    }

    /// A key outside the prime-order subgroup is refused at the door. It has to be:
    /// the per-epoch blinding factor is a full-range scalar and does not clear the
    /// cofactor the way a clamped X25519 scalar silently did.
    #[test]
    fn a_small_order_signing_half_is_refused() {
        // y = 0 decompresses to a point of order 4 — the standard small-order
        // example, and the exact shape the old X25519 id could not even express.
        let mut bytes = vec![0u8; 64];
        bytes[32..].copy_from_slice(&[7u8; 32]); // any KEM half; it is never reached
        let err = PublicId::from_bytes(&bytes).expect_err("must not accept torsion");
        let msg = err.to_string();
        assert!(
            msg.contains("prime-order") || msg.contains("signing half"),
            "rejected, but for an unclear reason: {msg}"
        );
    }

    // ---- per-epoch blinding -----------------------------------------------

    /// The property the whole construction rests on: what the *sender* computes from
    /// a contact id and what the *owner* signs with are the same key. If these ever
    /// drifted apart, every inbox would simply stop authenticating.
    #[test]
    fn the_owner_can_sign_for_the_slot_a_sender_computes() {
        let me = Identity::generate();
        let slot = blinded::public_at(&me.public(), 42);
        let sig = blinded::sign_at(&me, 42, b"challenge");
        assert!(blinded::verify(&slot, b"challenge", &sig));
    }

    /// Blinding is per epoch, and each epoch's key stands alone: a signature for one
    /// must not authenticate another. Without this the rotation would be decoration.
    #[test]
    fn each_epoch_is_a_separate_key() {
        let me = Identity::generate();
        let (a, b) = (
            blinded::public_at(&me.public(), 1),
            blinded::public_at(&me.public(), 2),
        );
        assert_ne!(a, b, "epochs must not share a slot");

        let sig1 = blinded::sign_at(&me, 1, b"m");
        assert!(blinded::verify(&a, b"m", &sig1));
        assert!(
            !blinded::verify(&b, b"m", &sig1),
            "a signature must not carry across epochs"
        );
    }

    /// Two identities never collide, and the slot is not the identity: a relay
    /// holding the slot has no way back to the contact id.
    #[test]
    fn a_slot_is_neither_shared_nor_the_identity_itself() {
        let (a, b) = (Identity::generate(), Identity::generate());
        assert_ne!(
            blinded::public_at(&a.public(), 7),
            blinded::public_at(&b.public(), 7)
        );
        assert_ne!(
            blinded::public_at(&a.public(), 7).as_slice(),
            &a.public().ed_bytes()[..],
            "the blinded key must not be the long-term key"
        );
    }

    /// Somebody else's signature does not open your inbox — the whole point of the
    /// proof. Checked with a real second identity rather than a mangled signature,
    /// because that is the attacker who actually exists.
    #[test]
    fn another_identity_cannot_sign_for_your_slot() {
        let (me, other) = (Identity::generate(), Identity::generate());
        let slot = blinded::public_at(&me.public(), 3);
        assert!(!blinded::verify(
            &slot,
            b"m",
            &blinded::sign_at(&other, 3, b"m")
        ));
    }

    /// The message is bound: a signature over one challenge cannot be replayed for
    /// another. (The relay issues a fresh nonce each session; this is what makes
    /// that worth doing.)
    #[test]
    fn a_signature_does_not_transfer_to_another_challenge() {
        let me = Identity::generate();
        let slot = blinded::public_at(&me.public(), 9);
        let sig = blinded::sign_at(&me, 9, b"nonce-one");
        assert!(!blinded::verify(&slot, b"nonce-two", &sig));
    }

    /// Verification refuses malformed input rather than panicking on it: the slot
    /// and the signature both arrive from the network.
    #[test]
    fn verification_refuses_junk_instead_of_panicking() {
        let me = Identity::generate();
        let slot = blinded::public_at(&me.public(), 1);
        let sig = blinded::sign_at(&me, 1, b"m");
        assert!(!blinded::verify(&slot[..31], b"m", &sig), "short slot");
        assert!(!blinded::verify(&slot, b"m", &sig[..63]), "short signature");
        assert!(!blinded::verify(&[0u8; 32], b"m", &sig), "small-order slot");
        let mut bent = sig;
        bent[0] ^= 0xff;
        assert!(!blinded::verify(&slot, b"m", &bent), "tampered signature");
    }

    /// Signing is deterministic for a given (identity, epoch, message). Not a
    /// requirement of the scheme, but it pins that the nonce prefix is derived and
    /// not drawn at random — a random one would still verify, and would quietly make
    /// every signature a fresh piece of unlinkable-looking noise nobody audited.
    #[test]
    fn signing_is_deterministic() {
        let me = Identity::generate();
        assert_eq!(
            blinded::sign_at(&me, 5, b"m"),
            blinded::sign_at(&me, 5, b"m")
        );
    }

    /// An identity file from before the signing half existed must be refused by
    /// name. It is 32 bare bytes — the exact shape of a seed — so accepting it would
    /// hand back a *different* identity with no error anywhere to explain why every
    /// saved contact stopped recognising you.
    #[test]
    fn an_identity_file_from_the_old_format_is_refused_not_reinterpreted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.key");

        std::fs::write(&path, [9u8; 32]).unwrap();
        // Not `expect_err`: `Identity` has no `Debug` on purpose — it holds a secret
        // — and a test is not a reason to give a key a way to print itself.
        let err = match Identity::load(&path) {
            Ok(_) => panic!("a bare 32-byte file must not load as an identity"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("different id"),
            "the error must say what happened: {err}"
        );

        // And the current format round-trips through the file.
        let id = Identity::generate();
        id.save(&path).unwrap();
        let back = Identity::load(&path).unwrap();
        assert_eq!(id.public().to_bytes(), back.public().to_bytes());
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
