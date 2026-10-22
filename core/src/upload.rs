//! Putting a deposit on a relay so that a dropped connection costs only the
//! bytes in flight, not the whole file.
//!
//! Two halves, kept apart because each is useful without the other:
//!
//! - [`SealedLayout`] is the ciphertext of a deposit, described rather than
//!   produced: its total length is known up front, and any byte range of it can
//!   be generated on demand by reading and sealing just the chunks that range
//!   touches. The encryption is deterministic given the key — each chunk's
//!   nonce is its index — so the bytes made for a retry are the bytes the relay
//!   already has, and a resumed upload stitches together seamlessly. Both
//!   deposit shapes are layouts: the browser link (a header, then
//!   length-prefixed 1 MiB chunks) and the mailbox blob (bare 16 MiB chunks).
//!
//! - [`upload`] drives a relay's resumable session (`/v1/upload`, see
//!   `docs/PROTOCOL.md`): open it, append in segments from wherever the relay
//!   says it stands, commit. Transient failures are retried here for a short
//!   while; past that the caller gets [`UploadError::Unavailable`] and keeps
//!   the [`UploadSession`] it was told about, to carry on from later — after a
//!   network change, or a restart of the whole program.
//!
//! A relay that predates sessions gets the single-request deposit it always
//! got, streamed from the same layout.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::crypto::{seal_chunk, CHUNK_KEY_LEN};
use crate::flow::{blob_limit, granted_ttl, TooLargeForRelay};
use crate::source::SendSource;

/// Ciphertext bytes per append request.
///
/// The relay keeps every byte that reaches it, so in principle one request
/// could carry the whole file. A reverse proxy that buffers request bodies
/// (nginx does by default) forwards nothing until a body is complete, though,
/// so what a drop really costs is the segment in flight. 32 MiB bounds that to
/// seconds on a decent link while keeping the per-request overhead negligible.
pub const SEGMENT_BYTES: u64 = 32 * 1024 * 1024;

/// How long a request may go without a single byte leaving, or — once its body
/// is sent — without an answer, before it is abandoned as dead. A connection
/// that silently died (a phone changing networks) otherwise hangs until the
/// operating system gives up on the socket, which can take a quarter of an hour.
const STALL: Duration = Duration::from_secs(60);

/// How many failures in a row, with no progress between them, before an upload
/// stops retrying on its own and hands the decision back to its caller — about
/// half a minute of backoff. Past that it is an outage rather than a blip, and
/// the caller's own, slower rhythm is the right one: a held send may meanwhile
/// find the recipient online and go direct.
const MAX_QUIET_RETRIES: u32 = 4;

/// Header names, as the relay spells them.
const UPLOAD_LENGTH_HEADER: &str = "x-arvolo-upload-length";
const UPLOAD_OFFSET_HEADER: &str = "x-arvolo-upload-offset";
const ENCAPPED_KEY_HEADER: &str = "x-arvolo-encapped-key";
const REVOKE_HASH_HEADER: &str = "x-arvolo-revoke-hash";

/// The ciphertext of one deposit, generated on demand. See the module docs.
#[derive(Clone)]
pub struct SealedLayout {
    source: SendSource,
    key: [u8; CHUNK_KEY_LEN],
    chunk_size: u32,
    size: u64,
    total_chunks: u32,
    /// Bytes before the first chunk (the link container's header).
    prefix: Arc<Vec<u8>>,
    /// Whether each chunk is preceded by its ciphertext length as a `u32` LE
    /// (the link container) or not (the mailbox blob).
    len_prefixed: bool,
    /// A local failure — the file could not be read, or changed under us —
    /// reaches the HTTP stack only as a broken body, indistinguishable from a
    /// broken network. It is parked here so [`upload`] can tell the two apart.
    fault: Arc<Mutex<Option<anyhow::Error>>>,
}

/// AES-GCM tag length: each sealed chunk is its plaintext plus this.
const TAG: u64 = 16;

impl SealedLayout {
    /// A mailbox blob: the file as bare sealed chunks of
    /// [`crate::chunked::CHUNK_SIZE`].
    pub fn mailbox(source: SendSource, key: [u8; CHUNK_KEY_LEN], size: u64) -> Self {
        Self::new(
            source,
            key,
            crate::chunked::CHUNK_SIZE,
            size,
            Vec::new(),
            false,
        )
    }

    /// The pieces from which [`Self::new`] builds a layout, exposed for the link
    /// container, whose header is assembled in `crate::link`.
    pub(crate) fn new(
        source: SendSource,
        key: [u8; CHUNK_KEY_LEN],
        chunk_size: u32,
        size: u64,
        prefix: Vec<u8>,
        len_prefixed: bool,
    ) -> Self {
        let total_chunks = size.div_ceil(chunk_size as u64) as u32;
        Self {
            source,
            key,
            chunk_size,
            size,
            total_chunks,
            prefix: Arc::new(prefix),
            len_prefixed,
            fault: Arc::new(Mutex::new(None)),
        }
    }

    /// Plaintext size.
    pub fn plain_len(&self) -> u64 {
        self.size
    }

    /// Total ciphertext length: what the relay will hold.
    pub fn len(&self) -> u64 {
        let framing = if self.len_prefixed { 4 } else { 0 };
        self.prefix.len() as u64 + self.size + self.total_chunks as u64 * (TAG + framing)
    }

    /// `len() == 0`, for clippy.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How far into the plaintext `cipher_offset` bytes of ciphertext reach —
    /// for a progress bar that counts the file, not its encryption.
    pub fn plain_offset(&self, cipher_offset: u64) -> u64 {
        let len = self.len();
        if len == 0 {
            return 0;
        }
        ((cipher_offset.min(len) as u128 * self.size as u128) / len as u128) as u64
    }

    fn unit_len(&self, i: u32) -> u64 {
        let pt = if i + 1 == self.total_chunks {
            self.size - i as u64 * self.chunk_size as u64
        } else {
            self.chunk_size as u64
        };
        pt + TAG + if self.len_prefixed { 4 } else { 0 }
    }

    /// Read and seal chunk `i`, with its length prefix when the layout has one.
    fn unit(&self, i: u32) -> Result<Vec<u8>> {
        let want = (self.unit_len(i) - TAG - if self.len_prefixed { 4 } else { 0 }) as usize;
        let mut buf = vec![0u8; want];
        let n = self
            .source
            .read_at_full(&mut buf, i as u64 * self.chunk_size as u64)
            .with_context(|| format!("read {}", self.source.label()))?;
        anyhow::ensure!(
            n == want,
            "{} got shorter while it was being sent",
            self.source.label()
        );
        let ct = seal_chunk(&self.key, i, self.total_chunks, &buf)?;
        if !self.len_prefixed {
            return Ok(ct);
        }
        let mut out = Vec::with_capacity(ct.len() + 4);
        out.extend_from_slice(&(ct.len() as u32).to_le_bytes());
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// The ciphertext bytes `[from, to)`, as a body for an HTTP request. Reads
    /// and seals off the async workers, one chunk at a time, pulled by the
    /// socket — so a slow relay slows the reads rather than piling up memory.
    pub fn stream(
        &self,
        from: u64,
        to: u64,
    ) -> impl futures_util::Stream<Item = io::Result<Vec<u8>>> + Send + 'static {
        let this = self.clone();
        let to = to.min(self.len());
        async_stream::stream! {
            let mut pos = from;
            let plen = this.prefix.len() as u64;
            if pos < plen && pos < to {
                let end = to.min(plen);
                yield Ok(this.prefix[pos as usize..end as usize].to_vec());
                pos = end;
            }
            if pos >= to {
                return;
            }
            // Every unit before the last is full-sized, so the unit holding
            // `pos` is a division away.
            let full = this.unit_len(0);
            let mut i = ((pos - plen) / full) as u32;
            let mut skip = (pos - plen) % full;
            while pos < to && i < this.total_chunks {
                let s = this.clone();
                let made = tokio::task::spawn_blocking(move || s.unit(i)).await;
                let unit = match made {
                    Ok(Ok(u)) => u,
                    Ok(Err(e)) => {
                        *this.fault.lock().unwrap() = Some(e);
                        yield Err(io::Error::other("read source file"));
                        return;
                    }
                    Err(e) => {
                        *this.fault.lock().unwrap() = Some(anyhow::Error::from(e));
                        yield Err(io::Error::other("read source file"));
                        return;
                    }
                };
                let start = skip as usize;
                let end = (start as u64 + (to - pos)).min(unit.len() as u64) as usize;
                pos += (end - start) as u64;
                yield Ok(if start == 0 && end == unit.len() {
                    unit
                } else {
                    unit[start..end].to_vec()
                });
                skip = 0;
                i += 1;
            }
        }
    }

    /// The local failure behind a broken body, if that is what broke it.
    fn take_fault(&self) -> Option<anyhow::Error> {
        self.fault.lock().unwrap().take()
    }

    /// The whole ciphertext in memory. For tests and small tools — the point of
    /// the type is that a deposit never needs this.
    pub fn to_vec(&self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.len() as usize);
        out.extend_from_slice(&self.prefix);
        for i in 0..self.total_chunks {
            out.extend_from_slice(&self.unit(i)?);
        }
        Ok(out)
    }
}

/// An upload session the relay has opened for us: enough to carry on with it.
///
/// Small and serialisable on purpose — it is what a caller writes down so that
/// a restart resumes instead of starting over.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UploadSession {
    /// The relay it lives on, as passed to [`upload`].
    pub relay: String,
    /// The session id: a capability, like a claim.
    pub id: String,
    /// The ciphertext length it was opened for. A layout of another length is
    /// another file, and does not resume this session.
    pub length: u64,
}

/// What the relay is told about the deposit, beyond its bytes — the same
/// fields a single-request deposit carries.
pub struct DepositMeta {
    /// HPKE encapsulated key for the recipient; empty for a link.
    pub encapped_key: Vec<u8>,
    /// BLAKE3 of the sender's revoke token.
    pub revoke_hash: [u8; 32],
    pub ttl: u64,
    pub max: u32,
}

/// A finished deposit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub claim: String,
    /// The TTL the relay granted, which may be less than asked.
    pub ttl_secs: u64,
}

/// Why an upload stopped.
#[derive(Debug)]
pub enum UploadError {
    /// The relay will not hold a file this large.
    TooLarge(TooLargeForRelay),
    /// The relay is full right now; worth trying again later.
    Busy,
    /// The relay has public links turned off (a link deposit only).
    LinksDisabled,
    /// The network, or the relay, is not answering. The session — if one was
    /// opened — is still good: try again with it.
    Unavailable(String),
    /// Something local and final: the file cannot be read, or the relay said
    /// something that retrying will not change.
    Fatal(anyhow::Error),
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadError::TooLarge(e) => write!(f, "{e}"),
            UploadError::Busy => write!(
                f,
                "the relay is being used by a lot of people right now — try again \
                 shortly, or use a relay of your own"
            ),
            UploadError::LinksDisabled => write!(
                f,
                "this relay has public download links disabled by its administrator — \
                 send it sealed to a recipient with --to, or use a different relay"
            ),
            UploadError::Unavailable(m) => write!(f, "relay unavailable: {m}"),
            UploadError::Fatal(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for UploadError {}

/// Callbacks an [`upload`] reports through. Both run on the uploading task and
/// must be quick.
pub struct UploadHooks<'a> {
    /// The session changed: opened (`Some`), or finished or abandoned (`None`).
    /// Write it down here to resume across a restart.
    pub session: &'a (dyn Fn(Option<&UploadSession>) + Send + Sync),
    /// Ciphertext bytes the relay is known to hold.
    pub progress: &'a (dyn Fn(u64) + Send + Sync),
}

impl UploadHooks<'_> {
    /// Hooks that do nothing.
    pub fn none() -> UploadHooks<'static> {
        UploadHooks {
            session: &|_| {},
            progress: &|_| {},
        }
    }
}

/// Whether `relay` offers resumable uploads (`"upload":true` in its features).
pub async fn relay_supports_upload(relay: &str) -> bool {
    crate::link::features_doc(relay)
        .await
        .map(|body| {
            body.split_whitespace()
                .collect::<String>()
                .contains("\"upload\":true")
        })
        .unwrap_or(false)
}

/// The client every upload request goes through — see
/// [`crate::http::upload_client`].
fn client() -> reqwest::Client {
    static SHARED: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    SHARED.get_or_init(crate::http::upload_client).clone()
}

/// Put `layout` on `relay` and return its claim.
///
/// `session` is the one to resume, when there is one — a session for another
/// relay or another length is ignored, and a session the relay no longer knows
/// is replaced by a new one. Every change is reported through `hooks.session`.
pub async fn upload(
    relay: &str,
    layout: &SealedLayout,
    meta: &DepositMeta,
    session: Option<UploadSession>,
    hooks: &UploadHooks<'_>,
) -> Result<Committed, UploadError> {
    let relay = relay.trim_end_matches('/').to_string();
    if !relay_supports_upload(&relay).await {
        return single_request(&relay, layout, meta, hooks).await;
    }
    let len = layout.len();
    let mut session = session.filter(|s| s.relay == relay && s.length == len);
    let mut quiet_failures = 0u32;
    let mut restarts = 0u32;
    let backoff = |n: u32| Duration::from_secs((1u64 << n.min(5)).min(30));

    loop {
        // Open, or find out where the one we have stands.
        let s = match session.clone() {
            Some(s) => s,
            None => match open(&relay, len, layout.plain_len(), meta).await {
                Ok(id) => {
                    let s = UploadSession {
                        relay: relay.clone(),
                        id,
                        length: len,
                    };
                    (hooks.session)(Some(&s));
                    session = Some(s.clone());
                    s
                }
                Err(Step::Retry(why)) => {
                    quiet_failures += 1;
                    if quiet_failures > MAX_QUIET_RETRIES {
                        return Err(UploadError::Unavailable(why));
                    }
                    tokio::time::sleep(backoff(quiet_failures)).await;
                    continue;
                }
                Err(Step::Stop(e)) => return Err(e),
                Err(Step::Restart) => unreachable!("open never restarts"),
            },
        };

        let outcome = drive(&s, layout, meta.ttl, hooks).await;
        match outcome {
            Ok(done) => {
                (hooks.session)(None);
                return Ok(done);
            }
            Err((Step::Retry(why), progressed)) => {
                if progressed {
                    quiet_failures = 0;
                }
                quiet_failures += 1;
                if quiet_failures > MAX_QUIET_RETRIES {
                    return Err(UploadError::Unavailable(why));
                }
                tracing::debug!(%why, attempt = quiet_failures, "upload interrupted; resuming");
                tokio::time::sleep(backoff(quiet_failures)).await;
            }
            Err((Step::Restart, _)) => {
                // The relay forgot the session (idle too long, or it was
                // restarted with its state wiped). Start over — but not in a
                // loop: a relay that forgets every session is broken.
                restarts += 1;
                (hooks.session)(None);
                session = None;
                if restarts > 2 {
                    return Err(UploadError::Unavailable(
                        "the relay keeps losing the upload".into(),
                    ));
                }
            }
            Err((Step::Stop(e), _)) => return Err(e),
        }
    }
}

/// Abandon a session on the relay, best effort: its partial file and the space
/// it reserved are released now rather than at the relay's idle deadline.
pub async fn abandon(session: &UploadSession) {
    let url = format!("{}/v1/upload/{}", session.relay, session.id);
    let _ = crate::http::client_with_timeout(Duration::from_secs(10))
        .delete(&url)
        .send()
        .await;
}

/// What one step of the protocol says to do next.
enum Step {
    /// Try again after a pause; the session is still good.
    Retry(String),
    /// The session is gone: open a new one.
    Restart,
    /// Give up.
    Stop(UploadError),
}

/// Map a refusal status to what it means for the upload. `None` for the
/// statuses each caller handles itself.
fn refusal(resp: &reqwest::Response, size: u64) -> Option<Step> {
    let status = resp.status();
    Some(match status {
        reqwest::StatusCode::PAYLOAD_TOO_LARGE => {
            Step::Stop(UploadError::TooLarge(TooLargeForRelay {
                size,
                limit: blob_limit(resp),
            }))
        }
        reqwest::StatusCode::INSUFFICIENT_STORAGE => Step::Stop(UploadError::Busy),
        reqwest::StatusCode::FORBIDDEN => Step::Stop(UploadError::LinksDisabled),
        reqwest::StatusCode::NOT_FOUND => Step::Restart,
        // Rate-limited, or a proxy/relay hiccup: transient.
        s if s == reqwest::StatusCode::TOO_MANY_REQUESTS || s.is_server_error() => {
            Step::Retry(format!("relay returned {s}"))
        }
        _ => return None,
    })
}

fn offset_of(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(UPLOAD_OFFSET_HEADER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

async fn open(relay: &str, len: u64, plain: u64, meta: &DepositMeta) -> Result<String, Step> {
    let url = format!("{relay}/v1/upload?ttl={}&max={}", meta.ttl, meta.max);
    let resp = client()
        .post(&url)
        .header(
            ENCAPPED_KEY_HEADER,
            data_encoding::BASE32_NOPAD.encode(&meta.encapped_key),
        )
        .header(
            REVOKE_HASH_HEADER,
            data_encoding::BASE32_NOPAD.encode(&meta.revoke_hash),
        )
        .header(UPLOAD_LENGTH_HEADER, len.to_string())
        .send()
        .await
        .map_err(|e| Step::Retry(e.to_string()))?;
    if resp.status().is_success() {
        let id = resp.text().await.map_err(|e| Step::Retry(e.to_string()))?;
        return Ok(id.trim().to_string());
    }
    // Plaintext size for the refusal message: that is the number the person
    // knows their file by.
    Err(match refusal(&resp, plain) {
        Some(Step::Restart) | None => Step::Stop(UploadError::Fatal(anyhow::anyhow!(
            "relay refused the upload ({})",
            resp.status()
        ))),
        Some(step) => step,
    })
}

async fn status(s: &UploadSession, plain: u64) -> Result<u64, Step> {
    let url = format!("{}/v1/upload/{}", s.relay, s.id);
    let resp = client()
        .get(&url)
        .send()
        .await
        .map_err(|e| Step::Retry(e.to_string()))?;
    if resp.status().is_success() {
        return offset_of(&resp).ok_or_else(|| Step::Retry("relay sent no upload offset".into()));
    }
    Err(refusal(&resp, plain)
        .unwrap_or_else(|| Step::Retry(format!("relay returned {}", resp.status()))))
}

/// Send what is missing of `s`, then commit it. On failure, also says whether
/// any bytes landed — progress resets the retry budget.
async fn drive(
    s: &UploadSession,
    layout: &SealedLayout,
    ttl: u64,
    hooks: &UploadHooks<'_>,
) -> Result<Committed, (Step, bool)> {
    let plain = layout.plain_len();
    let mut progressed = false;
    let mut at = status(s, plain).await.map_err(|e| (e, false))?;
    (hooks.progress)(at);
    loop {
        while at < s.length {
            let to = (at + SEGMENT_BYTES).min(s.length);
            let url = format!("{}/v1/upload/{}", s.relay, s.id);
            let request = client()
                .patch(&url)
                .header(UPLOAD_OFFSET_HEADER, at.to_string());
            let sent = send_watched(request, layout.stream(at, to), at, hooks).await;
            let resp = match sent {
                Ok(r) => r,
                Err(e) => {
                    if let Some(f) = layout.take_fault() {
                        return Err((Step::Stop(UploadError::Fatal(f)), progressed));
                    }
                    return Err((Step::Retry(e), progressed));
                }
            };
            let status = resp.status();
            if status.is_success() || status == reqwest::StatusCode::CONFLICT {
                // Either way the relay says where it now stands. A 409 with no
                // offset is "busy with another request" — most likely our own
                // previous attempt still draining: ask again shortly.
                match offset_of(&resp) {
                    // An answer that leaves the upload where it was would send
                    // the same segment again, and again: count it as a failed
                    // attempt so the retry budget ends it.
                    Some(new_at) if new_at == at => {
                        return Err((
                            Step::Retry("the relay did not take the bytes".into()),
                            progressed,
                        ));
                    }
                    Some(new_at) => {
                        if new_at > at {
                            progressed = true;
                        }
                        at = new_at;
                        (hooks.progress)(at);
                    }
                    None => {
                        return Err((Step::Retry("upload busy".into()), progressed));
                    }
                }
                continue;
            }
            // A failed append still reports how far it got.
            if let Some(new_at) = offset_of(&resp) {
                if new_at > at {
                    progressed = true;
                    at = new_at;
                    (hooks.progress)(at);
                }
            }
            return Err((
                refusal(&resp, plain)
                    .unwrap_or_else(|| Step::Retry(format!("relay returned {status}"))),
                progressed,
            ));
        }

        let url = format!("{}/v1/upload/{}/commit", s.relay, s.id);
        let resp = client()
            .post(&url)
            .send()
            .await
            .map_err(|e| (Step::Retry(e.to_string()), progressed))?;
        let status = resp.status();
        if status.is_success() {
            let ttl_secs = granted_ttl(&resp, ttl);
            let claim = resp
                .text()
                .await
                .map_err(|e| (Step::Retry(e.to_string()), progressed))?;
            return Ok(Committed {
                claim: claim.trim().to_string(),
                ttl_secs,
            });
        }
        if status == reqwest::StatusCode::CONFLICT {
            // Not complete after all (or busy): go on from where it stands.
            match offset_of(&resp) {
                Some(new_at) if new_at < s.length => {
                    at = new_at;
                    continue;
                }
                // Complete by its own count yet refused: asking again at once
                // would only get the same answer.
                Some(_) => {
                    return Err((
                        Step::Retry("the relay would not commit the upload".into()),
                        progressed,
                    ))
                }
                None => return Err((Step::Retry("upload busy".into()), progressed)),
            }
        }
        return Err((
            refusal(&resp, plain)
                .unwrap_or_else(|| Step::Retry(format!("relay returned {status}"))),
            progressed,
        ));
    }
}

/// Send `request` with `body` and wait for the answer, abandoning it when
/// nothing moves for [`STALL`]. Reports progress as `base` plus the body bytes
/// handed to the connection — finer than "one segment done", which on a slow
/// link is minutes between ticks.
async fn send_watched(
    request: reqwest::RequestBuilder,
    body: impl futures_util::Stream<Item = io::Result<Vec<u8>>> + Send + 'static,
    base: u64,
    hooks: &UploadHooks<'_>,
) -> Result<reqwest::Response, String> {
    use futures_util::StreamExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    let sent = Arc::new(AtomicU64::new(0));
    let counter = sent.clone();
    let body = body.map(move |chunk| {
        if let Ok(c) = &chunk {
            counter.fetch_add(c.len() as u64, Ordering::Relaxed);
        }
        chunk
    });
    let pending = request.body(reqwest::Body::wrap_stream(body)).send();
    tokio::pin!(pending);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut last = 0u64;
    let mut quiet_since = tokio::time::Instant::now();
    loop {
        tokio::select! {
            r = &mut pending => return r.map_err(|e| e.to_string()),
            _ = tick.tick() => {
                let now = sent.load(Ordering::Relaxed);
                if now != last {
                    last = now;
                    quiet_since = tokio::time::Instant::now();
                    (hooks.progress)(base + now);
                } else if quiet_since.elapsed() >= STALL {
                    return Err(format!(
                        "no progress for {}s — the connection looks dead",
                        STALL.as_secs()
                    ));
                }
            }
        }
    }
}

/// The deposit as one request, for a relay without sessions. Nothing survives
/// a drop here — that is what the relay offers — but it streams from the same
/// layout, so it never holds the file in memory either.
async fn single_request(
    relay: &str,
    layout: &SealedLayout,
    meta: &DepositMeta,
    hooks: &UploadHooks<'_>,
) -> Result<Committed, UploadError> {
    let url = format!("{relay}/v1/deposit?ttl={}&max={}", meta.ttl, meta.max);
    let request = crate::http::client()
        .post(&url)
        .header(
            ENCAPPED_KEY_HEADER,
            data_encoding::BASE32_NOPAD.encode(&meta.encapped_key),
        )
        .header(
            REVOKE_HASH_HEADER,
            data_encoding::BASE32_NOPAD.encode(&meta.revoke_hash),
        );
    let result = send_watched(request, layout.stream(0, layout.len()), 0, hooks).await;
    let resp = match result {
        Ok(r) => r,
        Err(e) => {
            if let Some(f) = layout.take_fault() {
                return Err(UploadError::Fatal(f));
            }
            return Err(UploadError::Unavailable(e));
        }
    };
    let status = resp.status();
    if status.is_success() {
        let ttl_secs = granted_ttl(&resp, meta.ttl);
        let claim = resp
            .text()
            .await
            .map_err(|e| UploadError::Unavailable(e.to_string()))?;
        (hooks.progress)(layout.len());
        return Ok(Committed {
            claim: claim.trim().to_string(),
            ttl_secs,
        });
    }
    Err(match refusal(&resp, layout.plain_len()) {
        Some(Step::Stop(e)) => e,
        Some(Step::Retry(why)) => UploadError::Unavailable(why),
        Some(Step::Restart) | None => UploadError::Unavailable(format!("relay returned {status}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn file(len: usize) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.bin");
        let data: Vec<u8> = (0..len).map(|i| (i * 31 % 257) as u8).collect();
        std::fs::write(&p, data).unwrap();
        (dir, p)
    }

    async fn range(l: &SealedLayout, from: u64, to: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut s = Box::pin(l.stream(from, to));
        while let Some(chunk) = s.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        out
    }

    /// Any range of the ciphertext, generated on its own, is exactly that range
    /// of the whole — which is what makes a resumed upload stitch together. The
    /// cuts land on every kind of boundary: inside the header, on a chunk edge,
    /// inside a length prefix, inside a chunk, at the very end.
    async fn every_range_is_a_slice_of_the_whole(l: SealedLayout) {
        let whole = l.to_vec().unwrap();
        assert_eq!(whole.len() as u64, l.len(), "len() is what is produced");
        let n = whole.len() as u64;
        let mut cuts = vec![0, 1, 3, 23, 24, 25, 4096, n / 3, n / 2, n - 17, n - 1, n];
        let unit = l.unit_len(0);
        let base = l.prefix.len() as u64;
        for k in 1..4u64 {
            for d in [0u64, 1, 2, 3, 4, 5] {
                cuts.push((base + k * unit + d).min(n));
                cuts.push((base + k * unit).saturating_sub(d).min(n));
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        // Each cut to the next one, and each cut to the end: every boundary is
        // both a start and an end somewhere, which is what a resume needs.
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            assert_eq!(
                range(&l, a, b).await,
                &whole[a as usize..b as usize],
                "range {a}..{b}"
            );
        }
        for &a in &cuts {
            assert_eq!(
                range(&l, a, n).await,
                &whole[a as usize..],
                "range {a}..end"
            );
        }
    }

    #[tokio::test]
    async fn a_link_container_is_the_same_bytes_whichever_way_it_is_cut() {
        let (_d, p) = file(3 * crate::link::LINK_CHUNK_SIZE as usize + 777);
        let source = SendSource::from(p.as_path());
        let seal = crate::link::LinkSeal::new(&source).unwrap();
        let l = crate::link::link_layout(source, &seal).unwrap();
        // And it is a link container a reader can open.
        let (name, plain) = crate::link::decrypt_link(&l.to_vec().unwrap(), &seal.key).unwrap();
        assert_eq!(name, "f.bin");
        assert_eq!(plain, std::fs::read(&p).unwrap());
        every_range_is_a_slice_of_the_whole(l).await;
    }

    #[tokio::test]
    async fn a_mailbox_blob_is_the_same_bytes_whichever_way_it_is_cut() {
        // Small chunks, so the boundaries are many and the test is quick; the
        // arithmetic is the same at 16 MiB.
        let (_d, p) = file(5 * 1000 + 3);
        let key = crate::crypto::random_chunk_key();
        let l = SealedLayout::new(
            SendSource::from(p.as_path()),
            key,
            1000,
            5003,
            Vec::new(),
            false,
        );
        every_range_is_a_slice_of_the_whole(l).await;
    }

    #[tokio::test]
    async fn an_empty_file_is_its_header_and_nothing_else() {
        let (_d, p) = file(0);
        let source = SendSource::from(p.as_path());
        let seal = crate::link::LinkSeal::new(&source).unwrap();
        let l = crate::link::link_layout(source, &seal).unwrap();
        assert_eq!(range(&l, 0, l.len()).await, l.to_vec().unwrap());
        let (_, plain) = crate::link::decrypt_link(&l.to_vec().unwrap(), &seal.key).unwrap();
        assert!(plain.is_empty());
    }

    /// A file that shrinks under an upload is a local, final failure — parked
    /// where the uploader can find it, not reported as a network error to retry.
    #[tokio::test]
    async fn a_file_that_shrinks_mid_upload_is_reported_as_itself() {
        let (_d, p) = file(4000);
        let key = crate::crypto::random_chunk_key();
        let l = SealedLayout::new(
            SendSource::from(p.as_path()),
            key,
            1000,
            4000,
            Vec::new(),
            false,
        );
        std::fs::write(&p, vec![0u8; 1500]).unwrap();
        let mut s = Box::pin(l.stream(0, l.len()));
        let mut broke = false;
        while let Some(chunk) = s.next().await {
            if chunk.is_err() {
                broke = true;
                break;
            }
        }
        assert!(broke);
        assert!(l.take_fault().is_some());
    }

    #[test]
    fn plain_progress_follows_the_cipher_offset() {
        let (_d, p) = file(10_000);
        let key = crate::crypto::random_chunk_key();
        let l = SealedLayout::new(
            SendSource::from(p.as_path()),
            key,
            1000,
            10_000,
            Vec::new(),
            false,
        );
        assert_eq!(l.plain_offset(0), 0);
        assert_eq!(l.plain_offset(l.len()), 10_000);
        assert!(l.plain_offset(l.len() / 2).abs_diff(5000) < 20);
    }
}
