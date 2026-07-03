//! Browser-download link: encrypt a file into a self-describing container that a
//! plain web page can fetch over HTTP and decrypt **in the browser** with
//! WebCrypto (AES-256-GCM), keeping the relay zero-knowledge.
//!
//! The symmetric key lives only in the URL **fragment** (`…/dl/<claim>#<key>`),
//! which browsers never send to the server, so the relay only ever stores and
//! serves ciphertext — the same end-to-end property as every other arvolo path.
//! The file is encrypted with the exact same chunked AES-256-GCM scheme as the
//! P2P path ([`crate::crypto::seal_chunk`]), so `crypto.subtle` can decrypt it
//! chunk-by-chunk and stream the plaintext straight to disk (no whole-file RAM).
//!
//! Container layout (all integers little-endian), matching the decoder in the
//! relay's `web/dl.js`:
//!
//! ```text
//!   [8]        magic  = b"ARVLNK01"
//!   [4]  u32   chunk_size    (plaintext bytes per data chunk)
//!   [8]  u64   total_size    (plaintext file size; also fixes the chunk count)
//!   [4]  u32   meta_len      (length of the encrypted metadata unit)
//!   [meta_len] meta_ct       AES-256-GCM(key, nonce=idx META_INDEX) of the UTF-8 name
//!   then, for i in 0..ceil(total_size/chunk_size):
//!     [4]  u32 ct_len
//!     [ct_len] chunk_ct      seal_chunk(key, i, total_chunks, plaintext_chunk)
//! ```
//!
//! The metadata unit is sealed as chunk index [`META_INDEX`] (`u32::MAX`), whose
//! nonce can never collide with a real data chunk (a file can't reach `u32::MAX`
//! chunks at [`LINK_CHUNK_SIZE`]), so the per-transfer random key is never reused
//! against the same nonce — the one invariant AES-GCM requires.

use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::crypto::{open_chunk, random_chunk_key, seal_chunk, CHUNK_KEY_LEN};

/// Container magic + version tag. Bump the digits on any format change.
const MAGIC: &[u8; 8] = b"ARVLNK01";

/// Plaintext bytes per chunk (1 MiB): balances WebCrypto per-call overhead
/// against the per-chunk memory a streaming browser download holds at a time.
pub const LINK_CHUNK_SIZE: u32 = 1024 * 1024;

/// Reserved chunk index for the encrypted metadata unit. `u32::MAX` is never a
/// real data-chunk index, so its nonce never collides with one.
const META_INDEX: u32 = u32::MAX;

/// The outcome of depositing a download link.
pub struct LinkOutcome {
    /// The full browser URL to share: `{relay}/dl/{claim}#{key}`.
    pub link: String,
    /// The relay claim (capability id of the deposited blob).
    pub claim: String,
    /// The original file name (also carried, encrypted, inside the blob).
    pub name: String,
    /// Plaintext file size in bytes.
    pub size: u64,
    /// Sender-only secret to later revoke the link via [`crate::flow::revoke_offline`].
    pub revoke_token: String,
}

/// A fresh sender-held secret (16 random bytes, base32) whose BLAKE3 hash the
/// relay stores to authorize a later revoke.
fn random_token() -> String {
    let bytes: [u8; 16] = rand::random();
    data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
}

fn total_chunks_for(size: u64, chunk_size: u32) -> u32 {
    let cs = chunk_size as u64;
    // ceil(size / chunk_size); an empty file has zero data chunks.
    (size.div_ceil(cs)) as u32
}

/// Encrypt `path` into a link container blob. Returns the blob, the per-transfer
/// key (goes in the URL fragment), the file name, and its plaintext size.
pub fn encrypt_link(path: &Path) -> Result<(Vec<u8>, [u8; CHUNK_KEY_LEN], String, u64)> {
    anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
    let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let size = data.len() as u64;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download.bin")
        .to_string();

    let chunk_size = LINK_CHUNK_SIZE;
    let total_chunks = total_chunks_for(size, chunk_size);
    let key = random_chunk_key();

    // Encrypted metadata (the UTF-8 filename): sealed as the reserved META_INDEX
    // chunk so the browser can show the real name without the relay ever seeing it.
    let meta_ct =
        seal_chunk(&key, META_INDEX, total_chunks, name.as_bytes()).context("seal meta")?;

    let mut blob = Vec::with_capacity(data.len() + meta_ct.len() + 24 + total_chunks as usize * 20);
    blob.extend_from_slice(MAGIC);
    blob.extend_from_slice(&chunk_size.to_le_bytes());
    blob.extend_from_slice(&size.to_le_bytes());
    blob.extend_from_slice(&(meta_ct.len() as u32).to_le_bytes());
    blob.extend_from_slice(&meta_ct);
    for (i, chunk) in data.chunks(chunk_size as usize).enumerate() {
        let ct = seal_chunk(&key, i as u32, total_chunks, chunk)
            .with_context(|| format!("seal chunk {i}"))?;
        blob.extend_from_slice(&(ct.len() as u32).to_le_bytes());
        blob.extend_from_slice(&ct);
    }
    Ok((blob, key, name, size))
}

/// Encode a link key for the URL fragment (URL-safe base64, no padding).
pub fn encode_key(key: &[u8; CHUNK_KEY_LEN]) -> String {
    data_encoding::BASE64URL_NOPAD.encode(key)
}

/// Decode a link key from a URL fragment. Rejects anything that isn't exactly a
/// 32-byte key.
pub fn decode_key(fragment: &str) -> Result<[u8; CHUNK_KEY_LEN]> {
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(fragment.trim().as_bytes())
        .context("invalid link key (base64url)")?;
    <[u8; CHUNK_KEY_LEN]>::try_from(bytes.as_slice())
        .map_err(|_| anyhow!("link key must be {CHUNK_KEY_LEN} bytes"))
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .ok_or_else(|| anyhow!("truncated link container"))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// Decode + decrypt a link container — the Rust mirror of the browser's decoder.
/// Verifies every chunk's AEAD tag (wrong key, tamper, reorder, or truncation
/// all fail). Returns the original file name and the recovered plaintext.
pub fn decrypt_link(blob: &[u8], key: &[u8; CHUNK_KEY_LEN]) -> Result<(String, Vec<u8>)> {
    let mut c = Cursor { buf: blob, pos: 0 };
    anyhow::ensure!(c.take(8)? == MAGIC, "not an arvolo link container");
    let chunk_size = c.u32()?;
    anyhow::ensure!(chunk_size > 0, "invalid chunk size");
    let size = c.u64()?;
    let total_chunks = total_chunks_for(size, chunk_size);

    let meta_len = c.u32()? as usize;
    let meta_ct = c.take(meta_len)?;
    let meta_pt = open_chunk(key, META_INDEX, total_chunks, meta_ct).context("decrypt metadata")?;
    let name = String::from_utf8(meta_pt).context("file name is not valid UTF-8")?;

    let mut out = Vec::with_capacity(size as usize);
    for i in 0..total_chunks {
        let ct_len = c.u32()? as usize;
        let ct = c.take(ct_len)?;
        let pt =
            open_chunk(key, i, total_chunks, ct).with_context(|| format!("decrypt chunk {i}"))?;
        out.extend_from_slice(&pt);
    }
    anyhow::ensure!(out.len() as u64 == size, "recovered size mismatch");
    Ok((name, out))
}

/// Encrypt `path` into a link container and deposit it on the relay, returning
/// the shareable browser URL (`{relay}/dl/{claim}#{key}`) and a sender-only
/// revoke token. Unlike [`crate::flow::deposit_offline`], the payload is NOT
/// sealed to a specific recipient: whoever holds the link (its fragment key) can
/// download it — the link *is* the capability. The relay still only sees
/// ciphertext.
pub async fn deposit_link(path: &Path, relay: &str, ttl: u64, max: u32) -> Result<LinkOutcome> {
    let (blob, key, name, size) = encrypt_link(path)?;

    // Sender-held revoke secret; the relay stores only its BLAKE3 hash.
    let revoke_token = random_token();
    let revoke_hash = blake3::hash(revoke_token.as_bytes());

    let relay = relay.trim_end_matches('/').to_string();
    let url = format!("{relay}/v1/deposit?ttl={ttl}&max={max}");
    let claim = reqwest::Client::new()
        .post(&url)
        // No HPKE on the link path → empty encapped key (decodes to no bytes).
        .header("x-arvolo-encapped-key", "")
        .header(
            "x-arvolo-revoke-hash",
            data_encoding::BASE32_NOPAD.encode(revoke_hash.as_bytes()),
        )
        .body(blob)
        .send()
        .await
        .context("deposit request")?
        .error_for_status()
        .context("relay rejected deposit")?
        .text()
        .await
        .context("read claim")?;
    let claim = claim.trim().to_string();

    let link = format!("{relay}/dl/{claim}#{}", encode_key(&key));
    Ok(LinkOutcome {
        link,
        claim,
        name,
        size,
        revoke_token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("photo.jpg");
        std::fs::write(&p, bytes).unwrap();
        (dir, p)
    }

    #[test]
    fn link_roundtrip_multichunk() {
        // Larger than one chunk so the multi-chunk framing is exercised.
        let bytes: Vec<u8> = (0..(LINK_CHUNK_SIZE as usize * 2 + 12345))
            .map(|i| (i % 251) as u8)
            .collect();
        let (_d, p) = write_tmp(&bytes);
        let (blob, key, name, size) = encrypt_link(&p).unwrap();
        assert_eq!(name, "photo.jpg");
        assert_eq!(size, bytes.len() as u64);

        let (dec_name, dec) = decrypt_link(&blob, &key).unwrap();
        assert_eq!(dec_name, "photo.jpg");
        assert_eq!(dec, bytes);
    }

    #[test]
    fn link_roundtrip_empty_and_small() {
        for bytes in [vec![], vec![7u8], b"hello world".to_vec()] {
            let (_d, p) = write_tmp(&bytes);
            let (blob, key, _n, _s) = encrypt_link(&p).unwrap();
            let (_dn, dec) = decrypt_link(&blob, &key).unwrap();
            assert_eq!(dec, bytes);
        }
    }

    #[test]
    fn wrong_key_fails() {
        let (_d, p) = write_tmp(b"secret payload");
        let (blob, _key, _n, _s) = encrypt_link(&p).unwrap();
        let other = random_chunk_key();
        assert!(decrypt_link(&blob, &other).is_err());
    }

    #[test]
    fn tampered_chunk_fails() {
        let (_d, p) = write_tmp(b"secret payload that is tamper-evident");
        let (mut blob, key, _n, _s) = encrypt_link(&p).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        assert!(decrypt_link(&blob, &key).is_err());
    }

    #[test]
    fn key_fragment_roundtrips() {
        let key = random_chunk_key();
        let frag = encode_key(&key);
        assert!(!frag.contains('='), "fragment is unpadded");
        assert_eq!(decode_key(&frag).unwrap(), key);
    }
}
