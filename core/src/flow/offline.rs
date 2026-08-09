use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::chunked::CHUNK_SIZE;
use crate::crypto::{
    open, open_chunk, random_chunk_key, random_pw_salt, seal, seal_chunk, unwrap_with_password,
    wrap_with_password, Identity, PublicId, Sealed, CHUNK_KEY_LEN,
};
use crate::offline::OfflineTicket;

use super::recv::{default_out, safe_download_name};
use super::MAILBOX_KEY_AAD;

/// Result of an offline deposit: the ticket to hand the recipient, plus the
/// sender-only **revoke token** — keep it to later cancel the delivery via
/// [`revoke_offline`]. The relay stores only a hash of it and never learns the
/// token unless a revoke is requested.
pub struct Deposited {
    pub ticket: OfflineTicket,
    pub revoke_token: String,
}

/// HTTP header carrying the base32 revoke-hash at deposit / revoke-token at revoke.
const REVOKE_HASH_HEADER: &str = "x-arvolo-revoke-hash";

const REVOKE_TOKEN_HEADER: &str = "x-arvolo-revoke-token";

fn random_token() -> String {
    let bytes: [u8; 16] = rand::random();
    data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
}

/// Encrypt `path` for `recipient` (authenticated as `me`) and deposit the
/// ciphertext on the relay. When `password` is set, the ciphertext is
/// additionally wrapped under a password-derived key (E2E — the relay can never
/// bypass it), and the recipient must supply the same password to
/// [`fetch_offline`]. Returns the ticket plus a sender-only revoke token.
/// Why an offline mailbox deposit couldn't be placed — lets callers react
/// differently: [`TooLarge`](DepositError::TooLarge) will never fit (deliver live
/// P2P instead), [`Unavailable`](DepositError::Unavailable) is transient (retry
/// later), [`Fatal`](DepositError::Fatal) is a local, unrecoverable error.
#[derive(Debug)]
pub enum DepositError {
    /// The relay refused the file as larger than its per-file cap.
    TooLarge,
    /// The relay was unreachable or returned a transient error. Human reason.
    Unavailable(String),
    /// A local, unrecoverable error (couldn't read or seal the file).
    Fatal(anyhow::Error),
}

impl std::fmt::Display for DepositError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DepositError::TooLarge => write!(f, "the relay refused the file as too large"),
            DepositError::Unavailable(m) => write!(f, "relay unavailable: {m}"),
            DepositError::Fatal(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for DepositError {}

#[allow(clippy::too_many_arguments)]
pub async fn deposit_offline(
    path: &Path,
    name: &str,
    recipient: &PublicId,
    me: &Identity,
    relay: &str,
    ttl: u64,
    max: u32,
    password: Option<&str>,
) -> std::result::Result<Deposited, DepositError> {
    use std::sync::{Arc, Mutex};

    use tokio::io::AsyncReadExt;
    use DepositError::Fatal;
    let io = |e: std::io::Error| Fatal(anyhow::Error::from(e));

    if !path.is_file() {
        return Err(Fatal(anyhow::anyhow!("{} is not a file", path.display())));
    }
    let total_size = tokio::fs::metadata(path).await.map_err(io)?.len();
    let total_chunks = if total_size == 0 {
        0
    } else {
        total_size.div_ceil(CHUNK_SIZE as u64) as u32
    };

    // Fresh content key; password-wrap it (small) if requested, then HPKE-seal it
    // to the recipient. The relay blob is then a stream of AES-GCM chunks under the
    // key — so neither side ever holds the whole file in memory.
    let key = random_chunk_key();
    let salt = if password.map(|p| !p.is_empty()).unwrap_or(false) {
        random_pw_salt().to_vec()
    } else {
        Vec::new()
    };
    let key_plain = if salt.is_empty() {
        key.to_vec()
    } else {
        wrap_with_password(password.unwrap(), &salt, &key)
            .context("wrap key with password")
            .map_err(Fatal)?
    };
    let sealed = seal(&key_plain, recipient, me, MAILBOX_KEY_AAD)
        .context("seal content key")
        .map_err(Fatal)?;

    // Seal each 16 MiB chunk as the upload pulls it, and hand the relay a lazy
    // stream — no whole-file copy ever lands on local disk. The old path spooled the
    // entire ciphertext to a temp file first, which needs as much free space as the
    // file itself on whatever volume holds the system temp dir; a large send on a
    // full disk died there with ENOSPC. Reading is driven by the socket, so a slow
    // relay just pauses the reads (backpressure) rather than racing ahead of it. The
    // relay enforces its own per-blob cap while it reads, so no Content-Length is
    // needed for an over-limit blob to be refused.
    let revoke_token = random_token();
    // Sender-held revoke secret; the relay stores only its BLAKE3 hash.
    let revoke_hash = blake3::hash(revoke_token.as_bytes());
    let relay = relay.trim_end_matches('/').to_string();
    let url = format!("{relay}/v1/deposit?ttl={ttl}&max={max}");

    // A read or seal failure is local and fatal — the relay never saw it, retrying
    // won't help — but it reaches the caller only as a stream error, which reqwest
    // wraps as an ordinary send failure indistinguishable from the relay being down.
    // Stash it out of band: after the request fails, a stashed error means Fatal, its
    // absence means the network. Without this, a mid-file read error would demote to
    // Unavailable and invite a pointless retry.
    let src = path.to_path_buf();
    let fatal: Arc<Mutex<Option<anyhow::Error>>> = Arc::new(Mutex::new(None));
    let fatal_w = fatal.clone();
    let body = reqwest::Body::wrap_stream(async_stream::stream! {
        let mut infile = match tokio::fs::File::open(&src).await {
            Ok(f) => f,
            Err(e) => {
                *fatal_w.lock().unwrap() = Some(anyhow::Error::from(e));
                yield Err(std::io::Error::other("open source file"));
                return;
            }
        };
        let mut buf = vec![0u8; CHUNK_SIZE as usize];
        for idx in 0..total_chunks {
            let want = if idx == total_chunks - 1 {
                (total_size - idx as u64 * CHUNK_SIZE as u64) as usize
            } else {
                CHUNK_SIZE as usize
            };
            if let Err(e) = infile.read_exact(&mut buf[..want]).await {
                *fatal_w.lock().unwrap() = Some(anyhow::Error::from(e));
                yield Err(std::io::Error::other("read source file"));
                return;
            }
            match seal_chunk(&key, idx, total_chunks, &buf[..want]) {
                Ok(ct) => yield Ok::<Vec<u8>, std::io::Error>(ct),
                Err(e) => {
                    *fatal_w.lock().unwrap() = Some(e);
                    yield Err(std::io::Error::other("seal chunk"));
                    return;
                }
            }
        }
    });

    let result = reqwest::Client::new()
        .post(&url)
        .header(
            "x-arvolo-encapped-key",
            data_encoding::BASE32_NOPAD.encode(&sealed.encapped_key),
        )
        .header(
            REVOKE_HASH_HEADER,
            data_encoding::BASE32_NOPAD.encode(revoke_hash.as_bytes()),
        )
        .body(body)
        .send()
        .await;

    let resp = match result {
        Ok(r) => r,
        Err(e) => {
            // A stashed local error is the true cause; the network wrapper reqwest
            // put on the truncated body is a symptom.
            if let Some(f) = fatal.lock().unwrap().take() {
                return Err(Fatal(f));
            }
            return Err(DepositError::Unavailable(e.to_string()));
        }
    };
    let status = resp.status();
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        return Err(DepositError::TooLarge);
    }
    if !status.is_success() {
        return Err(DepositError::Unavailable(format!(
            "relay returned {status}"
        )));
    }
    let claim = resp
        .text()
        .await
        .map_err(|e| DepositError::Unavailable(e.to_string()))?;

    Ok(Deposited {
        ticket: OfflineTicket {
            relay,
            claim: claim.trim().to_string(),
            sender: me.public().to_bytes(),
            salt,
            wrapped_key: sealed.ciphertext,
            total_size,
            // Passed in rather than read off `path`: for a folder or a multi-file
            // send, what is on disk is a temporary tar, and the recipient wants
            // the name the sender meant.
            name: name.to_string(),
        },
        revoke_token,
    })
}

/// Revoke a previously deposited offline blob, deleting it from the relay so it
/// can no longer be fetched. `revoke_token` is the one returned by
/// [`deposit_offline`]. Idempotent: a claim the relay no longer holds is treated
/// as already gone.
pub async fn revoke_offline(relay: &str, claim: &str, revoke_token: &str) -> Result<()> {
    let url = format!("{}/v1/entry/{}", relay.trim_end_matches('/'), claim);
    let resp = reqwest::Client::new()
        .delete(&url)
        .header(REVOKE_TOKEN_HEADER, revoke_token)
        .send()
        .await
        .context("revoke request")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(()); // already gone / expired
    }
    resp.error_for_status()
        .context("relay rejected revoke (wrong token?)")?;
    Ok(())
}

/// Whether a deposited offline blob is still on the relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimStatus {
    /// Still on the relay, not yet fetched.
    Pending,
    /// No longer on the relay — fetched (burn-after-read) or expired. Within a
    /// short poll window (far below the blob TTL) this means it was fetched.
    Gone,
}

/// Live status of a deposited blob on the relay, with download accounting when
/// the relay reports it. `downloads`/`max_downloads` are `None` against an older
/// relay that only signals presence.
#[derive(Debug, Clone, Copy)]
pub struct ClaimInfo {
    pub present: bool,
    pub downloads: Option<u32>,
    pub max_downloads: Option<u32>,
}

#[derive(serde::Deserialize)]
struct ClaimStatusBody {
    downloads: Option<u32>,
    max_downloads: Option<u32>,
}

/// Query a deposited blob's status **and** how many times it's been fetched.
/// Newer relays return the counts as JSON; against an older relay the counts are
/// `None` but presence still resolves.
pub async fn claim_info(relay: &str, claim: &str) -> Result<ClaimInfo> {
    let url = format!("{}/v1/entry/{}/status", relay.trim_end_matches('/'), claim);
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .context("claim status request")?;
    if resp.status().is_success() {
        let (downloads, max_downloads) = match resp.json::<ClaimStatusBody>().await {
            Ok(b) => (b.downloads, b.max_downloads),
            Err(_) => (None, None), // older relay: plain-text body, presence only
        };
        Ok(ClaimInfo {
            present: true,
            downloads,
            max_downloads,
        })
    } else if matches!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE
    ) {
        Ok(ClaimInfo {
            present: false,
            downloads: None,
            max_downloads: None,
        })
    } else {
        anyhow::bail!("relay rejected claim status: {}", resp.status())
    }
}

/// Query whether a deposited blob (`claim`) is still on the relay. Lets a sender
/// confirm an offline delivery (poll until [`ClaimStatus::Gone`]).
pub async fn claim_status(relay: &str, claim: &str) -> Result<ClaimStatus> {
    Ok(if claim_info(relay, claim).await?.present {
        ClaimStatus::Pending
    } else {
        ClaimStatus::Gone
    })
}

/// Where a mailbox fetch lands, from the caller's destination and the ticket.
///
/// `user_out` is a *destination*, not necessarily a filename — the same three
/// cases the chunked receiver already distinguishes, and for the same reason:
/// handed a directory (`--out ~/Downloads`, a GUI folder picker) this used to try
/// to open the folder as a file and fail with "Is a directory", and handed nothing
/// it wrote `received-<claim>.bin` even though the ticket knew the real name.
///
/// The name comes from the sender, so it is reduced to a single path component
/// before it reaches the filesystem: `../../.ssh/authorized_keys` saves as
/// `authorized_keys`, inside the chosen directory and nowhere else. A ticket
/// minted before names were carried has none, and falls back to the claim-derived
/// name it always had.
fn mailbox_out(user_out: Option<&Path>, t: &OfflineTicket) -> PathBuf {
    let named = safe_download_name(&t.name);
    match (user_out, named) {
        (Some(dir), Some(n)) if dir.is_dir() => dir.join(n),
        (Some(dir), None) if dir.is_dir() => dir.join(default_out(&t.claim)),
        // Anything else the caller named is the file they asked for.
        (Some(file), _) => file.to_path_buf(),
        (None, Some(n)) => PathBuf::from(n),
        (None, None) => default_out(&t.claim),
    }
}

/// Fetch and decrypt an offline ticket into `out` — a directory to save inside, a
/// filename to use, or nothing at all, in which case the ticket's own name is
/// used. Returns the output path and the number of plaintext bytes written.
pub async fn fetch_offline(
    ticket: &str,
    out: Option<PathBuf>,
    me: &Identity,
    password: Option<&str>,
) -> Result<(PathBuf, usize)> {
    use tokio::io::AsyncWriteExt;
    let t = OfflineTicket::decode(ticket)?;
    anyhow::ensure!(
        !t.wrapped_key.is_empty(),
        "unsupported offline ticket (older whole-file format is no longer accepted)"
    );
    let sender = PublicId::from_bytes(&t.sender).context("invalid sender in ticket")?;
    if t.has_password() && password.map(|p| p.is_empty()).unwrap_or(true) {
        anyhow::bail!("this link is password-protected — supply the password");
    }

    let url = format!("{}/v1/fetch/{}", t.relay.trim_end_matches('/'), t.claim);
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .context("fetch request")?
        .error_for_status()
        .context("relay rejected fetch (expired or already claimed?)")?;

    let encapped = resp
        .headers()
        .get("x-arvolo-encapped-key")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            data_encoding::BASE32_NOPAD
                .decode(s.to_uppercase().as_bytes())
                .ok()
        })
        .context("missing encapped key from relay")?;

    // Recover the content key: HPKE-open (verifies the sender), then peel the
    // optional password layer. Small — the file itself never passes through here.
    let key_plain = open(
        &Sealed {
            encapped_key: encapped,
            ciphertext: t.wrapped_key.clone(),
        },
        me,
        &sender,
        MAILBOX_KEY_AAD,
    )
    .context("decrypt content key (wrong identity, sender, or tampered)")?;
    let key_bytes = if t.has_password() {
        let pw = password.expect("password presence checked above");
        unwrap_with_password(pw, &t.salt, &key_plain).context("unwrap key with password")?
    } else {
        key_plain
    };
    let key: [u8; CHUNK_KEY_LEN] = key_bytes
        .as_slice()
        .try_into()
        .context("invalid content key length")?;

    let total_size = t.total_size;
    let total_chunks = if total_size == 0 {
        0
    } else {
        total_size.div_ceil(CHUNK_SIZE as u64) as u32
    };

    // Stream the ciphertext chunk stream straight to disk, decrypting a 16 MiB
    // chunk at a time — peak memory is ~one chunk, never the whole file. `carry`
    // reassembles exactly one sealed chunk from arbitrary HTTP frame boundaries.
    let out = mailbox_out(out.as_deref(), &t);
    let mut outfile = tokio::fs::File::create(&out)
        .await
        .with_context(|| format!("create {}", out.display()))?;
    let mut resp = resp;
    let mut carry: Vec<u8> = Vec::new();
    let mut eof = false;
    for idx in 0..total_chunks {
        let plain_len = if idx == total_chunks - 1 {
            total_size - idx as u64 * CHUNK_SIZE as u64
        } else {
            CHUNK_SIZE as u64
        };
        let ct_len = plain_len as usize + crate::crypto::CHUNK_TAG_LEN;
        while carry.len() < ct_len && !eof {
            match resp.chunk().await.context("read ciphertext")? {
                Some(b) => carry.extend_from_slice(&b),
                None => eof = true,
            }
        }
        anyhow::ensure!(
            carry.len() >= ct_len,
            "truncated mailbox blob at chunk {idx}"
        );
        let ct: Vec<u8> = carry.drain(..ct_len).collect();
        let plain = open_chunk(&key, idx, total_chunks, &ct).context("decrypt chunk")?;
        outfile.write_all(&plain).await.context("write chunk")?;
    }
    outfile.flush().await.context("flush output")?;
    Ok((out, total_size as usize))
}

#[cfg(test)]
mod mailbox_out_tests {
    use super::*;

    fn ticket(name: &str) -> OfflineTicket {
        OfflineTicket {
            relay: "https://relay.example".into(),
            claim: "claim1234567890abcdef".into(),
            sender: vec![1, 2, 3],
            salt: Vec::new(),
            wrapped_key: vec![9],
            total_size: 10,
            name: name.into(),
        }
    }

    /// The bug this resolves: a mailbox fetch used to write `received-<claim>.bin`
    /// even though the sender had a name for it.
    #[test]
    fn a_named_ticket_is_saved_under_its_name() {
        assert_eq!(
            mailbox_out(None, &ticket("report.pdf")),
            PathBuf::from("report.pdf")
        );
    }

    /// And the other half: `--out <dir>` used to be opened as a file and fail with
    /// EISDIR, which is how a folder picker or a shell tab-complete ends up here.
    #[test]
    fn a_directory_is_saved_into_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            mailbox_out(Some(dir.path()), &ticket("report.pdf")),
            dir.path().join("report.pdf")
        );
    }

    #[test]
    fn a_named_destination_is_taken_literally() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("elsewhere.bin");
        assert_eq!(mailbox_out(Some(&file), &ticket("report.pdf")), file);
    }

    /// The name is the sender's to choose, so it never reaches the filesystem as
    /// more than one component.
    #[test]
    fn a_traversal_name_cannot_escape_the_chosen_directory() {
        let dir = tempfile::tempdir().unwrap();
        let out = mailbox_out(Some(dir.path()), &ticket("../../.ssh/authorized_keys"));
        assert_eq!(out, dir.path().join("authorized_keys"));
        assert!(out.starts_with(dir.path()));
    }

    /// A ticket minted before names were carried keeps exactly the behaviour it
    /// had — including inside a directory, which is the case that used to fail.
    #[test]
    fn a_nameless_ticket_falls_back_to_the_claim() {
        let t = ticket("");
        assert_eq!(mailbox_out(None, &t), default_out(&t.claim));
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            mailbox_out(Some(dir.path()), &t),
            dir.path().join(default_out(&t.claim))
        );
    }
}
