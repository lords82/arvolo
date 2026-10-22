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
//! browser decryptor's `web/decryptor/src/dl.js`:
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
//! The metadata unit is sealed as chunk index `META_INDEX` (`u32::MAX`), whose
//! nonce can never collide with a real data chunk (a file can't reach `u32::MAX`
//! chunks at [`LINK_CHUNK_SIZE`]), so the per-transfer random key is never reused
//! against the same nonce — the one invariant AES-GCM requires.

use anyhow::{anyhow, Context, Result};

use crate::crypto::{open_chunk, random_chunk_key, seal_chunk, CHUNK_KEY_LEN};
use crate::upload::{DepositMeta, SealedLayout, UploadError, UploadHooks, UploadSession};

/// Container magic + version tag. Bump the digits on any format change.
const MAGIC: &[u8; 8] = b"ARVLNK01";

/// Plaintext bytes per chunk (1 MiB): balances WebCrypto per-call overhead
/// against the per-chunk memory a streaming browser download holds at a time.
pub const LINK_CHUNK_SIZE: u32 = 1024 * 1024;

/// Reserved chunk index for the encrypted metadata unit. `u32::MAX` is never a
/// real data-chunk index, so its nonce never collides with one.
const META_INDEX: u32 = u32::MAX;

/// The outcome of depositing a download link.
#[derive(Debug)]
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
    /// Seconds the relay actually granted, which may be less than asked (it clamps
    /// to its own `ARVOLO_MAX_TTL`). The link the sender is about to paste into a
    /// message dies at *this* deadline, so this is the one to quote back to them.
    pub ttl_secs: u64,
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

/// What a link is sealed with: the key that goes in the URL fragment, the
/// sender's revoke secret, and the name and size the container describes.
///
/// Fixed before the first byte is uploaded and kept until the last, because the
/// container is a pure function of it and the file — so an upload interrupted
/// half-way, even across a restart, resumes into the very bytes the relay
/// already holds. A fresh key would make the second half undecryptable with
/// the first half's.
///
/// Holds the link's key in the clear: whoever writes it down writes it where
/// only the user can read it, as with every record that carries a key.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LinkSeal {
    pub key: [u8; CHUNK_KEY_LEN],
    pub revoke_token: String,
    pub name: String,
    pub size: u64,
}

impl LinkSeal {
    /// A fresh seal for `source`: new key, new revoke token.
    pub fn new(source: &crate::source::SendSource) -> Result<Self> {
        if let crate::source::SendSource::Path(p) = source {
            if !p.is_file() {
                return Err(crate::source::NotAFile(p.clone()).into());
            }
        }
        let size = source.len()?;
        // The label is the original path (or filename) even for a handed-off
        // descriptor, so the download page never says "3" for `/dev/fd/3`.
        let name = std::path::Path::new(&source.label())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download.bin")
            .to_string();
        Ok(Self {
            key: random_chunk_key(),
            revoke_token: random_token(),
            name,
            size,
        })
    }
}

/// The link container for `source` under `seal`, described rather than built:
/// see [`SealedLayout`]. Its header — magic, chunk size, file size, the sealed
/// name — is assembled here; the chunks are sealed on demand as they are sent.
pub fn link_layout(source: crate::source::SendSource, seal: &LinkSeal) -> Result<SealedLayout> {
    let chunk_size = LINK_CHUNK_SIZE;
    let total_chunks = total_chunks_for(seal.size, chunk_size);
    // Encrypted metadata (the UTF-8 filename): sealed as the reserved META_INDEX
    // chunk so the browser can show the real name without the relay ever seeing it.
    let meta_ct = seal_chunk(&seal.key, META_INDEX, total_chunks, seal.name.as_bytes())
        .context("seal meta")?;
    let mut prefix = Vec::with_capacity(24 + meta_ct.len());
    prefix.extend_from_slice(MAGIC);
    prefix.extend_from_slice(&chunk_size.to_le_bytes());
    prefix.extend_from_slice(&seal.size.to_le_bytes());
    prefix.extend_from_slice(&(meta_ct.len() as u32).to_le_bytes());
    prefix.extend_from_slice(&meta_ct);
    Ok(SealedLayout::new(
        source, seal.key, chunk_size, seal.size, prefix, true,
    ))
}

/// Encrypt `path` into a link container blob, in memory. Returns the blob, the
/// per-transfer key (goes in the URL fragment), the file name, and its plaintext
/// size. The deposit path never does this — it streams the same bytes from
/// [`link_layout`] — so it is only for tests and small tools.
pub fn encrypt_link(
    source: impl Into<crate::source::SendSource>,
) -> Result<(Vec<u8>, [u8; CHUNK_KEY_LEN], String, u64)> {
    let source = source.into();
    let seal = LinkSeal::new(&source)?;
    let blob = link_layout(source, &seal)?.to_vec()?;
    Ok((blob, seal.key, seal.name, seal.size))
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

    // `size` is an unauthenticated header field. It IS bound into the meta chunk's
    // AAD (via `total_chunks`), so a tampered size fails the meta decrypt above —
    // but never pre-allocate a full attacker-declared `size` up front regardless:
    // cap the reservation and let the vec grow as real, tag-verified bytes arrive.
    const MAX_PREALLOC: usize = 64 * 1024 * 1024; // 64 MiB
    let mut out = Vec::with_capacity((size as usize).min(MAX_PREALLOC));
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

/// Whether the relay offers public download links. Queries `GET /v1/features`.
/// An older relay without the endpoint (404 / unreachable / unparsable) is
/// treated as **allowing** links, so we never block on a relay that predates the
/// feature flag; a relay that explicitly reports `"links":false` disables them.
pub async fn relay_allows_links(relay: &str) -> Result<bool> {
    let Some(body) = features_doc(relay).await else {
        return Ok(true);
    };
    // Minimal parse (no JSON dep): only an explicit false disables.
    Ok(!body
        .split_whitespace()
        .collect::<String>()
        .contains("\"links\":false"))
}

/// The largest single file this relay will hold, in bytes — what its
/// `/v1/features` advertises as `max_blob_bytes`. `None` for no limit, and for a
/// relay that does not say: older, unreachable, or answering nonsense. Either
/// way the deposit goes ahead, and a relay that does have a cap it kept quiet
/// about still refuses with a 413, which carries the same figure in a header.
///
/// Asked before a deposit so a file the relay will not take is refused here, in
/// one small round trip, instead of after all of it has been encrypted and
/// uploaded — which is how a person first learnt of the cap, and learnt only
/// "413", not how large is too large.
pub async fn relay_max_blob_bytes(relay: &str) -> Option<u64> {
    parse_max_blob_bytes(&features_doc(relay).await?)
}

/// The rule [`relay_max_blob_bytes`] applies to the document, split out from the
/// HTTP so it can be tested against a literal. Read by hand like the other
/// probes (see `features_handler` in the relay): the field is a bare integer, so
/// a flattened substring match and a parse is the whole of it.
fn parse_max_blob_bytes(body: &str) -> Option<u64> {
    let flat: String = body.split_whitespace().collect();
    let digits = flat.split("\"max_blob_bytes\":").nth(1)?;
    // Digits only, from the first character: a sign, a quote or a word in that
    // position is a relay answering nonsense, read as having said nothing.
    let n: u64 = digits
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    // Zero is the relay's word for "no limit", not a limit of nothing.
    (n > 0).then_some(n)
}

/// `GET /v1/features`, as text, when the relay answered it. Shared by the probes
/// above so a deposit that asks two questions makes one round trip per question
/// and no more — each probe reads what it needs by hand.
pub(crate) async fn features_doc(relay: &str) -> Option<String> {
    let url = format!("{}/v1/features", relay.trim_end_matches('/'));
    let resp = crate::http::client().get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.text().await.ok()
}

/// Encrypt `path` into a link container and deposit it on the relay, returning
/// the shareable browser URL (`{relay}/dl/{claim}#{key}`) and a sender-only
/// revoke token. Unlike [`crate::flow::deposit_offline`], the payload is NOT
/// sealed to a specific recipient: whoever holds the link (its fragment key) can
/// download it — the link *is* the capability. The relay still only sees
/// ciphertext.
///
/// One attempt, from the start: the convenience for a caller with nothing to
/// resume. [`deposit_link_with`] is the same deposit for one that has.
pub async fn deposit_link(
    source: impl Into<crate::source::SendSource>,
    relay: &str,
    ttl: u64,
    max: u32,
) -> Result<LinkOutcome> {
    let source = source.into();
    let seal = LinkSeal::new(&source)?;
    deposit_link_with(source, &seal, relay, ttl, max, None, &UploadHooks::none())
        .await
        .map_err(|e| match e {
            // Typed, so a caller can still read the cap off it.
            UploadError::TooLarge(t) => anyhow::Error::from(t),
            other => anyhow!("{other}"),
        })
}

/// The refusals a relay gives a link before any of it is sent: links turned
/// off, or a file over its per-file cap. Two small requests; a relay that does
/// not answer them is not refused here — the upload will find out.
///
/// For a caller that wants to say no straight away rather than start a
/// transfer only to fail it a moment later.
pub async fn preflight(relay: &str, size: u64) -> std::result::Result<(), UploadError> {
    if !relay_allows_links(relay).await.unwrap_or(true) {
        return Err(UploadError::LinksDisabled);
    }
    if let Some(limit) = relay_max_blob_bytes(relay).await {
        if size > limit {
            return Err(UploadError::TooLarge(crate::flow::TooLargeForRelay {
                size,
                limit: Some(limit),
            }));
        }
    }
    Ok(())
}

/// Deposit the link `seal` describes, resuming `session` when there is one.
///
/// Checks what can be checked before sending anything — links turned off, a
/// file over the relay's cap — and then hands the container to
/// [`crate::upload::upload`]. On [`UploadError::Unavailable`] the session last
/// reported through `hooks` is still good: call again with it and the same
/// seal to carry on.
pub async fn deposit_link_with(
    source: crate::source::SendSource,
    seal: &LinkSeal,
    relay: &str,
    ttl: u64,
    max: u32,
    session: Option<UploadSession>,
    hooks: &UploadHooks<'_>,
) -> std::result::Result<LinkOutcome, UploadError> {
    // Fail fast (before encrypting/uploading) if the relay administrator has
    // turned links off — with a message that says exactly that.
    if !relay_allows_links(relay)
        .await
        .map_err(|e| UploadError::Unavailable(format!("{e:#}")))?
    {
        return Err(UploadError::LinksDisabled);
    }
    // And if the file is larger than the relay lets one be — the same way, and
    // for the same reason: the refusal is certain, so it should cost one small
    // request and say what the limit is, not a full upload and a status code.
    if let Some(limit) = relay_max_blob_bytes(relay).await {
        if seal.size > limit {
            return Err(UploadError::TooLarge(crate::flow::TooLargeForRelay {
                size: seal.size,
                limit: Some(limit),
            }));
        }
    }
    // The file must still be the one the seal was made for: resuming into a
    // file that changed would stitch two files together under one key.
    let now = source.len().map_err(UploadError::Fatal)?;
    if now != seal.size {
        return Err(UploadError::Fatal(anyhow!(
            "{} changed size since the link was started",
            source.label()
        )));
    }
    let layout = link_layout(source, seal).map_err(UploadError::Fatal)?;
    let meta = DepositMeta {
        // No HPKE on the link path → empty encapped key (decodes to no bytes).
        encapped_key: Vec::new(),
        // Sender-held revoke secret; the relay stores only its BLAKE3 hash.
        revoke_hash: *blake3::hash(seal.revoke_token.as_bytes()).as_bytes(),
        ttl,
        max,
    };
    let relay = relay.trim_end_matches('/').to_string();
    let done = crate::upload::upload(&relay, &layout, &meta, session, hooks).await?;
    Ok(LinkOutcome {
        link: format!("{relay}/dl/{}#{}", done.claim, encode_key(&seal.key)),
        claim: done.claim,
        name: seal.name.clone(),
        size: seal.size,
        revoke_token: seal.revoke_token.clone(),
        ttl_secs: done.ttl_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cap is read off the document the relay actually writes — and off the
    /// documents older relays wrote, which have no such field and must read as
    /// "no limit known", not as a parse failure that blocks the deposit.
    #[test]
    fn the_blob_cap_is_read_by_hand_from_the_features_document() {
        let current = r#"{"links":true,"rz2":true,"have":true,"hold":true,"version":"0.14.2","cors":true,"max_blob_bytes":5368709120}"#;
        assert_eq!(parse_max_blob_bytes(current), Some(5_368_709_120));
        // Whitespace is the client's to ignore, as for every other field.
        let spaced = r#"{ "links": true, "max_blob_bytes": 64 }"#;
        assert_eq!(parse_max_blob_bytes(spaced), Some(64));
        // Zero is "no limit", not "nothing fits".
        assert_eq!(parse_max_blob_bytes(r#"{"max_blob_bytes":0}"#), None);
        // Older relays: no field at all.
        let older = r#"{"links":true,"rz2":true,"version":"0.14.1","cors":true}"#;
        assert_eq!(parse_max_blob_bytes(older), None);
        // A relay answering nonsense is read as having said nothing.
        assert_eq!(parse_max_blob_bytes(r#"{"max_blob_bytes":"lots"}"#), None);
        assert_eq!(parse_max_blob_bytes(r#"{"max_blob_bytes":-5}"#), None);
        assert_eq!(parse_max_blob_bytes("not json"), None);
    }

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
    fn huge_declared_size_fails_gracefully_no_oom() {
        // The `size` header (bytes 12..20, little-endian u64) is unauthenticated.
        // Overwriting it with a gigantic value must NOT trigger a multi-exabyte
        // pre-allocation — it must fail cleanly (the size is bound into the meta
        // chunk AAD, and the pre-alloc is capped regardless). See F8.
        let (_d, p) = write_tmp(b"payload");
        let (mut blob, key, _n, _s) = encrypt_link(&p).unwrap();
        blob[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
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
