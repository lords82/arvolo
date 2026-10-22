use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::chunked::CHUNK_SIZE;
use crate::crypto::{
    open, open_chunk, random_chunk_key, random_pw_salt, seal, unwrap_with_password,
    wrap_with_password, Identity, PublicId, Sealed, CHUNK_KEY_LEN,
};
use crate::offline::OfflineTicket;
use crate::upload::{DepositMeta, SealedLayout, UploadError, UploadHooks, UploadSession};

use super::recv::{default_out, safe_download_name};
use super::MAILBOX_KEY_AAD;

/// Result of an offline deposit: the ticket to hand the recipient, plus the
/// sender-only **revoke token** — keep it to later cancel the delivery via
/// [`revoke_offline`]. The relay stores only a hash of it and never learns the
/// token unless a revoke is requested.
pub struct Deposited {
    pub ticket: OfflineTicket,
    pub revoke_token: String,
    /// Seconds the relay actually granted, which may be **less than asked**: it
    /// clamps to its own `ARVOLO_MAX_TTL`. Everything downstream must deadline off
    /// this and not off the request — the inbox offer's own TTL above all, or the
    /// recipient keeps an offer whose blob the relay has already reaped.
    ///
    /// A relay too old to report it answers with the requested TTL, which is what
    /// that relay's behaviour was assumed to be anyway.
    pub ttl_secs: u64,
}

/// Response header on a deposit: the TTL in seconds the relay granted.
pub(crate) const GRANTED_TTL_HEADER: &str = "x-arvolo-ttl";

/// Why the relay wouldn't serve a deposit, in words the holder of the ticket can
/// act on.
///
/// It used to be one line for every status — "expired or already claimed?" — which
/// guessed out loud and then left the reader with no next step. The relay does
/// distinguish the two (`410` for an entry it still has and refuses, `404` once the
/// row is gone), and either way the answer is the same: nothing local will fix it,
/// the file has to be sent again. Say that.
fn fetch_refusal(status: reqwest::StatusCode) -> String {
    match status {
        reqwest::StatusCode::NOT_FOUND => "the relay no longer holds this deposit — it expired, \
             or it was already collected (a sealed deposit is burn-after-read). Nothing on your \
             side can recover it: ask the sender to send it again."
            .to_string(),
        reqwest::StatusCode::GONE => "the relay is holding this deposit for nobody: it has \
             expired or hit its download limit. Ask the sender to send it again."
            .to_string(),
        other => format!("the relay refused to serve this deposit ({other})"),
    }
}

/// Read the granted TTL off a deposit response, falling back to `asked` when the
/// relay doesn't report one (pre-header relay) or reports something unreadable.
pub(crate) fn granted_ttl(resp: &reqwest::Response, asked: u64) -> u64 {
    granted_ttl_from(
        resp.headers()
            .get(GRANTED_TTL_HEADER)
            .and_then(|v| v.to_str().ok()),
        asked,
    )
}

/// The rule [`granted_ttl`] applies, split out from the HTTP plumbing so it can be
/// tested on its own.
fn granted_ttl_from(raw: Option<&str>, asked: u64) -> u64 {
    raw.and_then(|s| s.trim().parse::<u64>().ok())
        // A relay may only ever grant *less*. A larger number is a broken or hostile
        // relay talking the sender into a deadline it hasn't promised to honour, and
        // trusting it would recreate exactly the bug this header exists to close.
        .map(|granted| granted.min(asked))
        .unwrap_or(asked)
}

const REVOKE_TOKEN_HEADER: &str = "x-arvolo-revoke-token";

/// On a relay's 413: the per-blob cap it enforces, in bytes. The same figure its
/// `/v1/features` advertises as `max_blob_bytes` ([`crate::link::relay_max_blob_bytes`]).
const MAX_BLOB_HEADER: &str = "x-arvolo-max-blob-bytes";

/// The cap a relay named on its refusal, if it named one. An older relay sends
/// nothing, and a relay sending nonsense is read as having said nothing.
pub(crate) fn blob_limit(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(MAX_BLOB_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
}

/// A file the relay will not hold, because it is larger than the relay allows.
///
/// Typed, like [`crate::manager::AcceptRefusal`], because more than one program
/// has to act on it — the daemon answers a window with a code instead of a
/// sentence, the window translates the code and shows the limit in the reader's
/// units — and because it carries the two numbers that make the refusal mean
/// something: how large the file is, and how large the relay lets one be. The
/// bare "413 Payload Too Large" it replaces said neither, and a person reading
/// it could not tell whether a slightly smaller file would do or none would.
///
/// `limit` is `None` when the relay did not say: a relay older than the header
/// and the `max_blob_bytes` feature, which still refuses, just without a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLargeForRelay {
    /// The file, in bytes.
    pub size: u64,
    /// The relay's per-file cap, in bytes, when it named one.
    pub limit: Option<u64>,
}

impl TooLargeForRelay {
    /// The stable word a front-end matches on, in place of reading the sentence.
    /// The same word the engine's waiting reason uses for the same fact.
    pub const CODE: &'static str = "tooLarge";

    /// The refusal inside an error, if that is what it is.
    pub fn of(e: &anyhow::Error) -> Option<Self> {
        e.downcast_ref::<TooLargeForRelay>().copied()
    }
}

impl std::fmt::Display for TooLargeForRelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.limit {
            Some(limit) => write!(
                f,
                "this relay does not accept files this large: its limit is {}, and the \
                 file is {}",
                human_size(limit),
                human_size(self.size)
            ),
            None => write!(
                f,
                "this relay does not accept files this large (the file is {})",
                human_size(self.size)
            ),
        }
    }
}

impl std::error::Error for TooLargeForRelay {}

/// Human-readable byte size, binary units: what a refusal names a limit in.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn random_token() -> String {
    let bytes: [u8; 16] = rand::random();
    data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
}

/// Why an offline mailbox deposit couldn't be placed — lets callers react
/// differently: [`TooLarge`](DepositError::TooLarge) will never fit (deliver live
/// P2P instead), [`Unavailable`](DepositError::Unavailable) is transient (retry
/// later), [`Fatal`](DepositError::Fatal) is a local, unrecoverable error.
#[derive(Debug)]
pub enum DepositError {
    /// The relay refused the file as larger than its per-file cap — or said in
    /// advance that it would, which is the same fact learnt before the upload
    /// rather than after it.
    TooLarge(TooLargeForRelay),
    /// The relay has no room at this moment: it is holding as many bytes as its
    /// operator allows. Distinct from [`Unavailable`](Self::Unavailable) because
    /// nothing is broken and nothing is unreachable — the mailbox is simply full
    /// right now, which on a shared relay means other people are using it. It
    /// clears on its own as deposits expire, so it is worth retrying.
    Busy,
    /// The relay was unreachable or returned a transient error. Human reason.
    Unavailable(String),
    /// A local, unrecoverable error (couldn't read or seal the file).
    Fatal(anyhow::Error),
}

impl std::fmt::Display for DepositError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DepositError::TooLarge(e) => write!(f, "{e}"),
            DepositError::Busy => write!(
                f,
                "the relay is being used by a lot of people right now — try again \
                 shortly, or use a relay of your own"
            ),
            DepositError::Unavailable(m) => write!(f, "relay unavailable: {m}"),
            DepositError::Fatal(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for DepositError {}

/// Encrypt `path` for `recipient` (authenticated as `me`) and deposit the
/// ciphertext on the relay. When `password` is set, the ciphertext is
/// additionally wrapped under a password-derived key (E2E — the relay can never
/// bypass it), and the recipient must supply the same password to
/// [`fetch_offline`]. Returns the ticket plus a sender-only revoke token.
///
/// One attempt, from the start: the convenience for a caller with nothing to
/// resume. [`deposit_offline_with`] is the same deposit for one that has.
#[allow(clippy::too_many_arguments)]
pub async fn deposit_offline(
    source: &crate::source::SendSource,
    name: &str,
    recipient: &PublicId,
    me: &Identity,
    relay: &str,
    ttl: u64,
    max: u32,
    password: Option<&str>,
) -> std::result::Result<Deposited, DepositError> {
    let seal = MailboxSeal::new(source, recipient, me, password).map_err(DepositError::Fatal)?;
    deposit_offline_with(
        source,
        name,
        &seal,
        me,
        relay,
        ttl,
        max,
        None,
        &UploadHooks::none(),
    )
    .await
}

/// How a mailbox deposit is sealed: the content key, the key as the recipient
/// will receive it (password-wrapped when there is a password, then HPKE-sealed
/// to them), and the sender's revoke secret.
///
/// Fixed before the first byte is uploaded and kept until the last, for the
/// same reason as [`crate::link::LinkSeal`]: the blob is a pure function of the
/// content key and the file, so an interrupted upload resumes into exactly the
/// bytes the relay already has. It also means a resumed deposit never needs the
/// password again — what the password protects is already sealed in here.
///
/// Holds the content key in the clear: whoever writes it down writes it where
/// only the user can read it, as with every record that carries a key.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MailboxSeal {
    pub key: [u8; CHUNK_KEY_LEN],
    /// Password salt; empty when there is no password.
    pub salt: Vec<u8>,
    /// The content key sealed to the recipient — the ticket's `wrapped_key`.
    pub wrapped_key: Vec<u8>,
    /// HPKE encapsulated key, handed to the relay with the blob.
    pub encapped_key: Vec<u8>,
    pub revoke_token: String,
    /// Plaintext size of the file it was made for.
    pub size: u64,
}

impl MailboxSeal {
    /// A fresh seal of `source` for `recipient`: new content key, new revoke
    /// token, the key password-wrapped when `password` is non-empty.
    pub fn new(
        source: &crate::source::SendSource,
        recipient: &PublicId,
        me: &Identity,
        password: Option<&str>,
    ) -> Result<Self> {
        // Only a path can be pre-checked; a handed-off descriptor IS the access.
        if let crate::source::SendSource::Path(p) = source {
            if !p.is_file() {
                return Err(crate::source::NotAFile(p.clone()).into());
            }
        }
        let size = source.len()?;
        // Fresh content key; password-wrap it (small) if requested, then
        // HPKE-seal it to the recipient. The relay blob is then a stream of
        // AES-GCM chunks under the key — so neither side ever holds the whole
        // file in memory.
        let key = random_chunk_key();
        let password = password.filter(|p| !p.is_empty());
        let salt = if password.is_some() {
            random_pw_salt().to_vec()
        } else {
            Vec::new()
        };
        let key_plain = match password {
            None => key.to_vec(),
            Some(pw) => wrap_with_password(pw, &salt, &key).context("wrap key with password")?,
        };
        let sealed =
            seal(&key_plain, recipient, me, MAILBOX_KEY_AAD).context("seal content key")?;
        Ok(Self {
            key,
            salt,
            wrapped_key: sealed.ciphertext,
            encapped_key: sealed.encapped_key,
            revoke_token: random_token(),
            size,
        })
    }
}

/// Deposit `source` sealed as `seal` describes, resuming `session` when there
/// is one. On [`DepositError::Unavailable`] the session last reported through
/// `hooks` is still good: call again with it and the same seal to carry on.
#[allow(clippy::too_many_arguments)]
pub async fn deposit_offline_with(
    source: &crate::source::SendSource,
    name: &str,
    seal: &MailboxSeal,
    me: &Identity,
    relay: &str,
    ttl: u64,
    max: u32,
    session: Option<UploadSession>,
    hooks: &UploadHooks<'_>,
) -> std::result::Result<Deposited, DepositError> {
    use DepositError::Fatal;

    let total_size = seal.size;
    // Ask before uploading: a relay that advertises its cap lets a file it will
    // not hold be refused here, in one small round trip, instead of after every
    // byte of it has been encrypted and sent. A relay that does not advertise
    // one is asked the old way — by uploading — and answers with 413.
    if let Some(limit) = crate::link::relay_max_blob_bytes(relay).await {
        if total_size > limit {
            return Err(DepositError::TooLarge(TooLargeForRelay {
                size: total_size,
                limit: Some(limit),
            }));
        }
    }
    // The file must still be the one the seal was made for: resuming into a
    // file that changed would stitch two files together under one key.
    if source.len().map_err(Fatal)? != total_size {
        return Err(Fatal(anyhow::anyhow!(
            "{} changed size since the send was started",
            source.label()
        )));
    }

    // Sealed chunk by chunk as the upload pulls them — no whole-file copy ever
    // lands in memory or on local disk. The old path spooled the entire
    // ciphertext to a temp file first, which needs as much free space as the
    // file itself on whatever volume holds the system temp dir; a large send on
    // a full disk died there with ENOSPC.
    let layout = SealedLayout::mailbox(source.clone(), seal.key, total_size);
    let meta = DepositMeta {
        encapped_key: seal.encapped_key.clone(),
        // Sender-held revoke secret; the relay stores only its BLAKE3 hash.
        revoke_hash: *blake3::hash(seal.revoke_token.as_bytes()).as_bytes(),
        ttl,
        max,
    };
    let relay = relay.trim_end_matches('/').to_string();
    let done = crate::upload::upload(&relay, &layout, &meta, session, hooks)
        .await
        .map_err(|e| match e {
            UploadError::TooLarge(t) => DepositError::TooLarge(t),
            UploadError::Busy => DepositError::Busy,
            UploadError::Unavailable(m) => DepositError::Unavailable(m),
            UploadError::Fatal(e) => Fatal(e),
            // Only a link deposit can meet this; a sealed one never does.
            UploadError::LinksDisabled => Fatal(anyhow::anyhow!("{e}")),
        })?;

    Ok(Deposited {
        ttl_secs: done.ttl_secs,
        ticket: OfflineTicket {
            relay,
            claim: done.claim,
            sender: me.public().to_bytes(),
            salt: seal.salt.clone(),
            wrapped_key: seal.wrapped_key.clone(),
            total_size,
            // Passed in rather than read off `path`: for a folder or a multi-file
            // send, what is on disk is a temporary tar, and the recipient wants
            // the name the sender meant.
            name: name.to_string(),
        },
        revoke_token: seal.revoke_token.clone(),
    })
}

/// Revoke a previously deposited offline blob, deleting it from the relay so it
/// can no longer be fetched. `revoke_token` is the one returned by
/// [`deposit_offline`]. Idempotent: a claim the relay no longer holds is treated
/// as already gone.
pub async fn revoke_offline(relay: &str, claim: &str, revoke_token: &str) -> Result<()> {
    let url = format!("{}/v1/entry/{}", relay.trim_end_matches('/'), claim);
    let resp = crate::http::client()
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
    let resp = crate::http::client()
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
    use super::recv::collision_free;
    let named = safe_download_name(&t.name);
    // Derived names never overwrite (` (1)`, ` (2)`, …); an explicitly named
    // file is the caller's decision. Same rule as `single_file_out`.
    match (user_out, named) {
        (Some(dir), Some(n)) if dir.is_dir() => collision_free(dir.join(n)),
        (Some(dir), None) if dir.is_dir() => collision_free(dir.join(default_out(&t.claim))),
        // Anything else the caller named is the file they asked for.
        (Some(file), _) => file.to_path_buf(),
        (None, Some(n)) => collision_free(PathBuf::from(n)),
        (None, None) => collision_free(default_out(&t.claim)),
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
    fetch_offline_with_progress(ticket, out, me, password, |_, _| {}).await
}

/// Like [`fetch_offline`], reporting `(bytes_on_disk, total_bytes)` after every
/// decrypted chunk lands. The mailbox fetch used to be silent until the end,
/// which on a big file is indistinguishable from a hang.
pub async fn fetch_offline_with_progress(
    ticket: &str,
    out: Option<PathBuf>,
    me: &Identity,
    password: Option<&str>,
    on_progress: impl Fn(u64, u64),
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

    // `hold=1`: serve the blob but do not spend the download yet — that is the
    // ack below, sent once every byte is on disk. A relay too old to know the
    // query burns on read as it always did, and the ack then finds nothing,
    // which is fine. See `Mailbox::fetch_plan_held` in the relay.
    let relay = t.relay.trim_end_matches('/');
    let url = format!("{relay}/v1/fetch/{}?hold=1", t.claim);
    let resp = crate::http::client()
        .get(&url)
        .send()
        .await
        .context("fetch request")?;
    let resp = match resp.error_for_status_ref() {
        Ok(_) => resp,
        Err(e) => {
            let status = resp.status();
            let why = fetch_refusal(status);
            // Typed when it is the one refusal a person can do something about
            // (ask the sender again), so a window can say it in their language.
            return Err(
                if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
                    anyhow::Error::from(e).context(crate::manager::FailReason::DepositGone(why))
                } else {
                    anyhow::Error::from(e).context(why)
                },
            );
        }
    };

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
        on_progress(
            (idx as u64 * CHUNK_SIZE as u64).saturating_add(plain_len),
            total_size,
        );
    }
    outfile.flush().await.context("flush output")?;
    // Now, and only now, the relay may count this download — and burn a
    // one-shot deposit. Best effort: the file is here whatever the relay says,
    // and an unacked claim expires on its own.
    let ack = format!("{relay}/v1/fetch/{}/ack", t.claim);
    if let Err(e) = crate::http::client().post(&ack).send().await {
        tracing::debug!("mailbox ack not delivered (the deposit stays until its TTL): {e}");
    }
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

/// How much of the requested TTL the sender may believe it got.
///
/// Every case here has to end in a number no larger than what was asked, because
/// that number becomes the inbox offer's own lifetime: whatever the sender
/// over-estimates, the recipient pays for as an arrival that 404s.
#[cfg(test)]
mod granted_ttl_tests {
    use super::{granted_ttl_from, human_size, TooLargeForRelay};

    /// The sentence names both numbers when it has them, in units a person reads,
    /// and does not invent a limit when the relay gave none.
    #[test]
    fn a_refusal_says_how_large_is_too_large() {
        let with = TooLargeForRelay {
            size: 6 * 1024 * 1024 * 1024,
            limit: Some(5 * 1024 * 1024 * 1024),
        }
        .to_string();
        assert!(with.contains("5.0 GiB"), "{with}");
        assert!(with.contains("6.0 GiB"), "{with}");

        let without = TooLargeForRelay {
            size: 150 * 1024 * 1024,
            limit: None,
        }
        .to_string();
        assert!(without.contains("150.0 MiB"), "{without}");
        assert!(!without.contains("limit is"), "{without}");

        // Carried inside anyhow, it is still itself on the way out.
        let e = anyhow::Error::new(TooLargeForRelay {
            size: 1,
            limit: Some(2),
        });
        assert_eq!(TooLargeForRelay::of(&e).and_then(|r| r.limit), Some(2));
        assert_eq!(TooLargeForRelay::of(&anyhow::anyhow!("other")), None);
    }

    #[test]
    fn sizes_read_in_binary_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    const WEEK: u64 = 7 * 24 * 3600;

    #[test]
    fn a_reported_shorter_ttl_is_what_we_got() {
        assert_eq!(granted_ttl_from(Some("86400"), WEEK), 86_400);
    }

    #[test]
    fn a_relay_that_reports_nothing_is_taken_at_its_word() {
        // A relay older than the header. Assuming the request was honoured is what
        // every client did before it existed, so nothing regresses for those relays.
        assert_eq!(granted_ttl_from(None, WEEK), WEEK);
    }

    #[test]
    fn a_ttl_longer_than_asked_is_not_believed() {
        // A relay cannot extend a deadline on the sender's behalf: acting on the
        // larger number is precisely how an offer outlives its blob.
        assert_eq!(granted_ttl_from(Some("999999999"), 3600), 3600);
    }

    #[test]
    fn an_unreadable_ttl_falls_back_instead_of_failing() {
        // A deposit is already placed by the time this is read; a junk header is no
        // reason to fail the send.
        assert_eq!(granted_ttl_from(Some("soon"), 3600), 3600);
        assert_eq!(granted_ttl_from(Some(""), 3600), 3600);
        assert_eq!(granted_ttl_from(Some("-1"), 3600), 3600);
    }

    #[test]
    fn whitespace_around_the_number_is_tolerated() {
        assert_eq!(granted_ttl_from(Some("  600 "), 3600), 600);
    }
}
