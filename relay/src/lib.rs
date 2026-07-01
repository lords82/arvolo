//! arvolo relay / mailbox: zero-knowledge store-and-forward.
//!
//! Holds **opaque ciphertext** blobs (the relay never sees plaintext or keys)
//! addressed by a random claim token, each with a TTL after which it is reaped.
//! This is the offline-delivery path: the sender deposits the encrypted blob
//! while the recipient is away; the recipient claims it later; it expires and is
//! deleted on its own.
//!
//! Storage: metadata in **SQLite**, ciphertext as **files on disk** (`blob_dir`).
//! Survives restarts. Milestone 2 scope: a single relay, full-blob deposit, TTL,
//! max-downloads (burn-after-read). Federation, multi-recipient refcount GC, and
//! partial backfill are post-MVP (see docs/ROADMAP-FUTURE.md).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use arvolo_core::backfill::BlobNode;
use arvolo_core::chunked::SeedRequest;
use axum::{
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use rusqlite::{params, Connection};
use serde::Deserialize;

/// Shared HTTP state: the zero-knowledge mailbox plus the blob-store node that
/// backs seed-to-relay backfill.
#[derive(Clone)]
pub struct AppState {
    pub mailbox: Arc<Mailbox>,
    pub blobs: Arc<BlobNode>,
}

/// Maximum blob size accepted by the relay (server policy / abuse guard).
pub const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024 * 1024; // 2 GiB
const ENCAPPED_KEY_HEADER: &str = "x-arvolo-encapped-key";

/// What the sender deposits: an opaque, end-to-end-encrypted blob.
#[derive(Clone)]
pub struct Deposit {
    /// HPKE encapsulated key (opaque to the relay).
    pub encapped_key: Vec<u8>,
    /// HPKE ciphertext (opaque to the relay).
    pub ciphertext: Vec<u8>,
    /// Time-to-live in seconds.
    pub ttl_secs: u64,
    /// How many times it may be fetched before being deleted (>=1).
    pub max_downloads: u32,
}

/// What a recipient gets back on a successful claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claimed {
    pub encapped_key: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// Reasons a claim can fail.
#[derive(Debug, PartialEq, Eq)]
pub enum MailboxError {
    NotFound,
    Expired,
    Exhausted,
    TooLarge,
    Backend(String),
}

impl std::fmt::Display for MailboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MailboxError::NotFound => write!(f, "no such claim"),
            MailboxError::Expired => write!(f, "expired"),
            MailboxError::Exhausted => write!(f, "download limit reached"),
            MailboxError::TooLarge => write!(f, "blob too large"),
            MailboxError::Backend(e) => write!(f, "backend error: {e}"),
        }
    }
}

fn backend<E: std::fmt::Display>(e: E) -> MailboxError {
    MailboxError::Backend(e.to_string())
}

/// Persistent zero-knowledge mailbox: SQLite metadata + ciphertext files.
pub struct Mailbox {
    conn: Mutex<Connection>,
    blob_dir: PathBuf,
}

impl Mailbox {
    /// Open (creating if needed) a mailbox with its SQLite db and blob directory.
    pub fn open(
        db_path: impl AsRef<Path>,
        blob_dir: impl AsRef<Path>,
    ) -> Result<Self, MailboxError> {
        let conn = Connection::open(db_path).map_err(backend)?;
        let blob_dir = blob_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&blob_dir).map_err(backend)?;
        Self::init(conn, blob_dir)
    }

    /// An ephemeral mailbox (in-memory SQLite + a temp blob dir) for tests/dev.
    pub fn in_memory() -> Result<Self, MailboxError> {
        let conn = Connection::open_in_memory().map_err(backend)?;
        let mut dir = std::env::temp_dir();
        let suffix: [u8; 8] = rand::random();
        dir.push(format!(
            "arvolo-relay-{}",
            data_encoding::HEXLOWER.encode(&suffix)
        ));
        std::fs::create_dir_all(&dir).map_err(backend)?;
        Self::init(conn, dir)
    }

    fn init(conn: Connection, blob_dir: PathBuf) -> Result<Self, MailboxError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                claim         TEXT PRIMARY KEY,
                encapped_key  BLOB NOT NULL,
                expires_at    INTEGER NOT NULL,
                max_downloads INTEGER NOT NULL,
                downloads     INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS seeded (
                token       TEXT NOT NULL,
                hash        TEXT NOT NULL,
                expires_at  INTEGER NOT NULL,
                PRIMARY KEY (token, hash)
            );
            CREATE TABLE IF NOT EXISTS rendezvous (
                slot        TEXT NOT NULL,
                key         TEXT NOT NULL,
                value       BLOB NOT NULL,
                expires_at  INTEGER NOT NULL,
                PRIMARY KEY (slot, key)
            );",
        )
        .map_err(backend)?;
        Ok(Self {
            conn: Mutex::new(conn),
            blob_dir,
        })
    }

    fn blob_path(&self, claim: &str) -> PathBuf {
        self.blob_dir.join(format!("{claim}.bin"))
    }

    /// Store `deposit`, returning a random claim token. `now` is unix seconds.
    pub fn deposit(&self, deposit: Deposit, now: u64) -> Result<String, MailboxError> {
        if deposit.ciphertext.len() > MAX_BLOB_BYTES {
            return Err(MailboxError::TooLarge);
        }
        let claim = random_claim();
        std::fs::write(self.blob_path(&claim), &deposit.ciphertext).map_err(backend)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO entries (claim, encapped_key, expires_at, max_downloads, downloads)
             VALUES (?1, ?2, ?3, ?4, 0)",
            params![
                claim,
                deposit.encapped_key,
                now.saturating_add(deposit.ttl_secs) as i64,
                deposit.max_downloads.max(1) as i64,
            ],
        )
        .map_err(backend)?;
        Ok(claim)
    }

    /// Claim a blob. Increments the download count; deletes the entry (and its
    /// file) once the download budget is spent (burn-after-read for `max == 1`).
    pub fn fetch(&self, claim: &str, now: u64) -> Result<Claimed, MailboxError> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT encapped_key, expires_at, max_downloads, downloads
                 FROM entries WHERE claim = ?1",
                params![claim],
                |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => MailboxError::NotFound,
                other => backend(other),
            })?;
        let (encapped_key, expires_at, max_downloads, downloads) = row;

        if now >= expires_at as u64 {
            self.delete(&conn, claim)?;
            return Err(MailboxError::Expired);
        }
        if downloads >= max_downloads {
            self.delete(&conn, claim)?;
            return Err(MailboxError::Exhausted);
        }

        let ciphertext = std::fs::read(self.blob_path(claim)).map_err(backend)?;
        let new_downloads = downloads + 1;
        if new_downloads >= max_downloads {
            self.delete(&conn, claim)?;
        } else {
            conn.execute(
                "UPDATE entries SET downloads = ?2 WHERE claim = ?1",
                params![claim, new_downloads],
            )
            .map_err(backend)?;
        }
        Ok(Claimed {
            encapped_key,
            ciphertext,
        })
    }

    /// Delete all expired entries (and their files); returns how many.
    pub fn reap(&self, now: u64) -> Result<usize, MailboxError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT claim FROM entries WHERE expires_at <= ?1")
            .map_err(backend)?;
        let claims: Vec<String> = stmt
            .query_map(params![now as i64], |r| r.get::<_, String>(0))
            .map_err(backend)?
            .collect::<Result<_, _>>()
            .map_err(backend)?;
        for claim in &claims {
            self.delete(&conn, claim)?;
        }
        Ok(claims.len())
    }

    fn delete(&self, conn: &Connection, claim: &str) -> Result<(), MailboxError> {
        let _ = std::fs::remove_file(self.blob_path(claim));
        conn.execute("DELETE FROM entries WHERE claim = ?1", params![claim])
            .map_err(backend)?;
        Ok(())
    }

    /// Number of stored entries.
    pub fn len(&self) -> usize {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get::<_, i64>(0))
            .map(|n| n as usize)
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // ---- seeded-blob lifecycle (backfill) ---------------------------------

    /// Record a seeded blob with a one-time release token and expiry.
    pub fn record_seed(
        &self,
        token: &str,
        hash_hex: &str,
        expires_at: u64,
    ) -> Result<(), MailboxError> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO seeded (token, hash, expires_at) VALUES (?1, ?2, ?3)",
                params![token, hash_hex, expires_at as i64],
            )
            .map_err(backend)?;
        Ok(())
    }

    /// Does this (token, hash) pair authorize releasing the chunk?
    pub fn seed_exists(&self, token: &str, hash: &str) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT 1 FROM seeded WHERE token = ?1 AND hash = ?2",
                params![token, hash],
                |_| Ok(()),
            )
            .is_ok()
    }

    /// Forget a single seeded-chunk record (after release).
    pub fn delete_seed_one(&self, token: &str, hash: &str) -> Result<(), MailboxError> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM seeded WHERE token = ?1 AND hash = ?2",
                params![token, hash],
            )
            .map_err(backend)?;
        Ok(())
    }

    /// (token, hash) pairs of seeded chunks whose TTL has passed.
    pub fn expired_seeds(&self, now: u64) -> Vec<(String, String)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare("SELECT token, hash FROM seeded WHERE expires_at <= ?1") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map(params![now as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        });
        match rows {
            Ok(it) => it.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ---- rendezvous (short-code pairing) ----------------------------------

    /// Claim a rendezvous slot by writing its first value. Returns `false` if the
    /// slot key already exists (someone else claimed it) — the sender then retries
    /// with a fresh nameplate.
    pub fn rz_claim(
        &self,
        slot: &str,
        key: &str,
        value: &[u8],
        expires_at: u64,
    ) -> Result<bool, MailboxError> {
        let n = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT OR IGNORE INTO rendezvous (slot, key, value, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![slot, key, value, expires_at as i64],
            )
            .map_err(backend)?;
        Ok(n == 1)
    }

    /// Write (or overwrite) a rendezvous value.
    pub fn rz_put(
        &self,
        slot: &str,
        key: &str,
        value: &[u8],
        expires_at: u64,
    ) -> Result<(), MailboxError> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO rendezvous (slot, key, value, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![slot, key, value, expires_at as i64],
            )
            .map_err(backend)?;
        Ok(())
    }

    /// Read a rendezvous value (if present and unexpired).
    pub fn rz_get(&self, slot: &str, key: &str, now: u64) -> Option<Vec<u8>> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT value FROM rendezvous WHERE slot = ?1 AND key = ?2 AND expires_at > ?3",
                params![slot, key, now as i64],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .ok()
    }

    /// Delete a whole rendezvous slot (all its keys) — called after the ticket is
    /// fetched (burn) so nothing lingers.
    pub fn rz_delete_slot(&self, slot: &str) {
        let _ = self
            .conn
            .lock()
            .unwrap()
            .execute("DELETE FROM rendezvous WHERE slot = ?1", params![slot]);
    }

    /// Delete all expired rendezvous rows; returns how many.
    pub fn rz_reap(&self, now: u64) -> usize {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM rendezvous WHERE expires_at <= ?1",
                params![now as i64],
            )
            .unwrap_or(0)
    }
}

/// Current unix time in seconds.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_claim() -> String {
    let bytes: [u8; 16] = rand::random();
    data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
}

// ---- HTTP layer -----------------------------------------------------------

#[derive(Deserialize)]
struct DepositQuery {
    #[serde(default = "default_ttl")]
    ttl: u64,
    #[serde(default = "default_max")]
    max: u32,
}

fn default_ttl() -> u64 {
    7 * 24 * 3600
}
fn default_max() -> u32 {
    1
}

/// Build the relay HTTP router over the shared [`AppState`].
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/deposit", post(deposit_handler))
        .route("/v1/fetch/{claim}", get(fetch_handler))
        .route("/v1/addr", get(addr_handler))
        .route("/v1/seed", post(seed_handler))
        .route("/v1/release/{token}/{hash}", post(release_handler))
        .route("/v1/rz/{slot}/{key}", post(rz_post_handler).get(rz_get_handler))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
}

/// TTL (seconds) for a rendezvous slot: long enough for a human to type the code,
/// short enough that abandoned slots vanish quickly.
const RZ_TTL: u64 = 600;
/// Key under which the sender claims a slot (its SPAKE2 message).
const RZ_CLAIM_KEY: &str = "ms";
/// Key holding the encrypted ticket; fetching it burns the whole slot.
const RZ_TICKET_KEY: &str = "tkt";

/// Store a rendezvous value. The claim key (`ms`) fails with 409 if the slot is
/// already taken, so the sender can pick a fresh nameplate.
async fn rz_post_handler(
    State(state): State<AppState>,
    AxumPath((slot, key)): AxumPath<(String, String)>,
    body: Bytes,
) -> Result<String, (StatusCode, String)> {
    let exp = now_unix().saturating_add(RZ_TTL);
    if key == RZ_CLAIM_KEY {
        let claimed = state
            .mailbox
            .rz_claim(&slot, &key, &body, exp)
            .map_err(|e| (status_for(&e), e.to_string()))?;
        if !claimed {
            return Err((StatusCode::CONFLICT, "slot already taken".into()));
        }
    } else {
        state
            .mailbox
            .rz_put(&slot, &key, &body, exp)
            .map_err(|e| (status_for(&e), e.to_string()))?;
    }
    Ok("ok".into())
}

/// Read a rendezvous value (404 until posted). Reading the ticket burns the slot.
async fn rz_get_handler(
    State(state): State<AppState>,
    AxumPath((slot, key)): AxumPath<(String, String)>,
) -> Result<Bytes, (StatusCode, String)> {
    match state.mailbox.rz_get(&slot, &key, now_unix()) {
        Some(v) => {
            if key == RZ_TICKET_KEY {
                state.mailbox.rz_delete_slot(&slot);
            }
            Ok(Bytes::from(v))
        }
        None => Err((StatusCode::NOT_FOUND, "not yet".into())),
    }
}

fn status_for(e: &MailboxError) -> StatusCode {
    match e {
        MailboxError::NotFound => StatusCode::NOT_FOUND,
        MailboxError::Expired | MailboxError::Exhausted => StatusCode::GONE,
        MailboxError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        MailboxError::Backend(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn deposit_handler(
    State(state): State<AppState>,
    Query(q): Query<DepositQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<String, (StatusCode, String)> {
    let mb = &state.mailbox;
    let encapped_key = headers
        .get(ENCAPPED_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            data_encoding::BASE32_NOPAD
                .decode(s.to_uppercase().as_bytes())
                .ok()
        })
        .ok_or((
            StatusCode::BAD_REQUEST,
            format!("missing/invalid {ENCAPPED_KEY_HEADER} header (base32)"),
        ))?;

    let deposit = Deposit {
        encapped_key,
        ciphertext: body.to_vec(),
        ttl_secs: q.ttl,
        max_downloads: q.max,
    };
    mb.deposit(deposit, now_unix())
        .map_err(|e| (status_for(&e), e.to_string()))
}

async fn fetch_handler(
    State(state): State<AppState>,
    AxumPath(claim): AxumPath<String>,
) -> Result<Response, (StatusCode, String)> {
    match state.mailbox.fetch(&claim, now_unix()) {
        Ok(c) => {
            let mut resp = c.ciphertext.into_response();
            let encoded = data_encoding::BASE32_NOPAD.encode(&c.encapped_key);
            if let Ok(val) = encoded.parse() {
                resp.headers_mut().insert(ENCAPPED_KEY_HEADER, val);
            }
            Ok(resp)
        }
        Err(e) => Err((status_for(&e), e.to_string())),
    }
}

/// Seed (backfill) a P2P blob into the relay's store. Body = the sender's blob
/// ticket; returns the relay's provider address (base32) so the sender can
/// advertise the relay as a fallback provider.
async fn seed_handler(
    State(state): State<AppState>,
    body: String,
) -> Result<String, (StatusCode, String)> {
    let req = SeedRequest::decode(body.trim())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad seed request: {e}")))?;
    state
        .blobs
        .seed_chunks(req.sender, &req.chunks)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("seed failed: {e}")))?;
    let exp = now_unix().saturating_add(seed_ttl());
    for hash in &req.chunks {
        state
            .mailbox
            .record_seed(&req.token, &hash.to_string(), exp)
            .map_err(|e| (status_for(&e), e.to_string()))?;
    }
    Ok("ok".into())
}

/// The relay's iroh blob-node address plus a fresh transfer token, so the sender
/// can advertise the relay as a provider and use the token to seed/release.
async fn addr_handler(State(state): State<AppState>) -> Result<String, (StatusCode, String)> {
    let addr = state
        .blobs
        .addr_encoded()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("addr: {e}")))?;
    Ok(format!("{addr}\n{}", random_claim()))
}

/// TTL (seconds) for seeded chunks not yet released. Default 24h.
fn seed_ttl() -> u64 {
    std::env::var("ARVOLO_SEED_TTL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24 * 3600)
}

/// Incremental cleanup: the receiver calls this for each chunk as it gets it, so
/// the relay frees that chunk during the download (TTL is only a backstop).
async fn release_handler(
    State(state): State<AppState>,
    AxumPath((token, hash)): AxumPath<(String, String)>,
) -> Result<String, (StatusCode, String)> {
    if state.mailbox.seed_exists(&token, &hash) {
        state.blobs.release_hex(&hash).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("release failed: {e}"),
            )
        })?;
        let _ = state.mailbox.delete_seed_one(&token, &hash);
    }
    Ok("ok".into())
}
