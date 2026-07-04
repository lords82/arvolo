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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use arvolo_core::backfill::BlobNode;
use arvolo_core::chunked::SeedRequest;
use arvolo_core::swarm::{AnnounceReq, AnnounceResp, PeerInfo};
use axum::{
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, Connection};
use serde::Deserialize;

/// Shared HTTP state: the zero-knowledge mailbox plus the blob-store node that
/// backs seed-to-relay backfill.
#[derive(Clone)]
pub struct AppState {
    pub mailbox: Arc<Mailbox>,
    pub blobs: Arc<BlobNode>,
    /// Per-process secret keying the inbox session-token MAC. Random at startup:
    /// a restart just forces clients to re-run the (cheap) auth handshake.
    pub auth_secret: Arc<[u8; 32]>,
    /// Whether this relay offers public **browser download links** (`--link`).
    /// When an administrator disables it, the `/dl` page is not served and
    /// link deposits (HPKE-less blobs) are refused. Defaults to enabled.
    pub links_enabled: bool,
    /// Swarm tracker: `swarm_id → (peer node_addr → entry)`, in-memory with a TTL.
    /// Purely a rendezvous for peers of one shared `arvc…` ticket to find each
    /// other; the relay learns node addresses + bitfields, never the key/plaintext.
    pub swarm: Arc<Mutex<HashMap<String, HashMap<String, SwarmPeer>>>>,
}

/// One tracked peer of a swarm (in-memory tracker row).
#[derive(Clone)]
pub struct SwarmPeer {
    pub node_addr: String,
    pub bitfield: Vec<u8>,
    pub expires_at: u64,
}

/// How long a swarm peer stays listed without re-announcing.
pub const SWARM_PEER_TTL_SECS: u64 = 60;
/// Max peers tracked (and returned) per swarm — bounds memory and response size.
pub const MAX_SWARM_PEERS: usize = 100;
/// Max distinct swarms tracked at once (disk/memory guard; new swarms rejected
/// beyond this until entries expire).
pub const MAX_SWARMS: usize = 10_000;
/// Sanity cap on a swarm's piece count (bounds an announced bitfield).
pub const MAX_SWARM_CHUNKS: u32 = 1_000_000;

/// Whether the relay administrator has disabled public download links, via
/// `ARVOLO_DISABLE_LINKS` (set to `1`/`true`/`yes`/`on`).
pub fn links_disabled_from_env() -> bool {
    matches!(
        std::env::var("ARVOLO_DISABLE_LINKS")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

impl AppState {
    /// Build state with a fresh random inbox-auth secret.
    pub fn new(mailbox: Arc<Mailbox>, blobs: Arc<BlobNode>) -> Self {
        Self {
            links_enabled: true,
            mailbox,
            blobs,
            auth_secret: Arc::new(rand::random()),
            swarm: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Default cap on a single deposited blob. The relay buffers the whole body in
/// memory, so this is deliberately memory-safe rather than the aspirational
/// large-file size — big transfers use the streaming P2P chunk path instead.
/// Override with `ARVOLO_MAX_BLOB_BYTES`.
pub const DEFAULT_MAX_BLOB_BYTES: usize = 256 * 1024 * 1024; // 256 MiB
/// Default cap on the number of stored mailbox entries (disk-fill guard).
/// Override with `ARVOLO_MAX_ENTRIES`.
pub const DEFAULT_MAX_ENTRIES: i64 = 100_000;
/// Cap on chunks a single seed request may ask the relay to fetch+store.
pub const MAX_SEED_CHUNKS_PER_REQ: usize = 4096;
/// Default cap on the total number of pending seeded chunk rows (each backs one
/// ≤16 MiB chunk file on disk), bounding the unauthenticated seed path's disk
/// footprint. Override with `ARVOLO_MAX_SEEDED_ROWS`.
pub const DEFAULT_MAX_SEEDED_ROWS: i64 = 50_000;
/// Cap on a rendezvous value (SPAKE2 message / encrypted ticket are tiny).
pub const MAX_RZ_VALUE_BYTES: usize = 64 * 1024; // 64 KiB
/// Default cap on the number of live rendezvous rows (disk-fill guard).
pub const DEFAULT_MAX_RZ_ROWS: i64 = 100_000;
/// Cap on a single sealed inbox offer (metadata + an `arvc` ticket whose size
/// grows with chunk count — a big file's ticket is a few hundred KiB).
pub const MAX_INBOX_VALUE_BYTES: usize = 512 * 1024; // 512 KiB
/// Default cap on the total number of live inbox rows (disk-fill guard).
/// Override with `ARVOLO_MAX_INBOX_ROWS`.
pub const DEFAULT_MAX_INBOX_ROWS: i64 = 100_000;
/// Cap on pending offers a single inbox slot may hold, so one victim's inbox
/// can't be flooded (and so a slow reader can't accumulate unbounded rows).
pub const MAX_INBOX_PER_SLOT: i64 = 64;
/// Default TTL (seconds) for a deposited offer when the client doesn't request
/// one: long enough for a client that is briefly away to come back, short enough
/// that an unaccepted offer vanishes.
pub const INBOX_TTL_SECS: u64 = 600;
/// Upper bound on a caller-requested offer TTL (`?ttl=`). An offline offer must
/// survive as long as the mailbox blob it points at (up to the blob TTL cap), so
/// this mirrors the deposit TTL cap rather than the short live-offer default.
pub const INBOX_MAX_TTL_SECS: u64 = 30 * 24 * 3600; // 30 days
/// TTL (seconds) of a presence beacon: a listening client refreshes well within
/// this, so a stale beacon means the client went away. Kept short so a departed
/// client stops showing "online" quickly (the send-side watchdog covers the brief
/// window where a just-quit client still looks online).
pub const PRESENCE_TTL_SECS: u64 = 30;
/// Default cap on the number of live presence beacon rows (disk-fill guard).
/// Override with `ARVOLO_MAX_PRESENCE_ROWS`.
pub const DEFAULT_MAX_PRESENCE_ROWS: i64 = 100_000;
/// Default cap on a blob's TTL (seconds). Prevents effectively-immortal entries
/// permanently occupying storage — and the i64 overflow a near-`u64::MAX` TTL
/// would cause (`now + ttl` wraps negative → the entry never expires). Override
/// with `ARVOLO_MAX_TTL`.
pub const DEFAULT_MAX_TTL_SECS: u64 = 30 * 24 * 3600; // 30 days
/// Upper bound on the max-downloads a single deposit may request.
pub const MAX_DOWNLOADS_CAP: u32 = 10_000;

fn env_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Max deposited-blob size (env `ARVOLO_MAX_BLOB_BYTES`, default 256 MiB).
pub fn max_blob_bytes() -> usize {
    env_usize("ARVOLO_MAX_BLOB_BYTES", DEFAULT_MAX_BLOB_BYTES)
}

fn max_entries() -> i64 {
    env_usize("ARVOLO_MAX_ENTRIES", DEFAULT_MAX_ENTRIES as usize) as i64
}

fn max_rz_rows() -> i64 {
    env_usize("ARVOLO_MAX_RZ_ROWS", DEFAULT_MAX_RZ_ROWS as usize) as i64
}

fn max_inbox_rows() -> i64 {
    env_usize("ARVOLO_MAX_INBOX_ROWS", DEFAULT_MAX_INBOX_ROWS as usize) as i64
}

fn max_presence_rows() -> i64 {
    env_usize(
        "ARVOLO_MAX_PRESENCE_ROWS",
        DEFAULT_MAX_PRESENCE_ROWS as usize,
    ) as i64
}

fn max_ttl_secs() -> u64 {
    env_usize("ARVOLO_MAX_TTL", DEFAULT_MAX_TTL_SECS as usize) as u64
}

fn max_seeded_rows() -> i64 {
    env_usize("ARVOLO_MAX_SEEDED_ROWS", DEFAULT_MAX_SEEDED_ROWS as usize) as i64
}

const ENCAPPED_KEY_HEADER: &str = "x-arvolo-encapped-key";
/// Base32 BLAKE3 hash of the revoke token, sent at deposit (optional).
const REVOKE_HASH_HEADER: &str = "x-arvolo-revoke-hash";
/// The revoke token itself, sent on a DELETE to authorize revocation.
const REVOKE_TOKEN_HEADER: &str = "x-arvolo-revoke-token";
/// Base32 BLAKE3 hash of an inbox offer's retract token, sent at inbox POST.
const INBOX_POSTER_HASH_HEADER: &str = "x-arvolo-poster-hash";
/// The inbox offer's retract token, sent on a DELETE to retract one's own offer.
const INBOX_POSTER_TOKEN_HEADER: &str = "x-arvolo-poster-token";

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
    /// BLAKE3 hash of the sender's revoke token (empty ⇒ not revocable). The
    /// relay only ever holds the hash, never the token itself.
    pub revoke_hash: Vec<u8>,
}

/// What a recipient gets back on a successful claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claimed {
    pub encapped_key: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// Poster-facing status of an inbox offer (see [`Mailbox::inbox_status_by_poster`]).
#[derive(Debug, PartialEq, Eq)]
pub enum InboxStatus {
    /// The offer no longer exists (recipient acked/accepted it, or it expired).
    Gone,
    /// The presented retract token does not match this offer.
    BadToken,
    /// Still queued, not yet seen by a live recipient poll.
    Pending,
    /// Delivered to a live, authenticated recipient poll.
    Fetched,
}

/// The metadata result of a claim: enough to read the blob file off-lock.
/// Returned by [`Mailbox::fetch_plan`].
pub struct FetchPlan {
    pub encapped_key: Vec<u8>,
    /// Filesystem path of the blob to read.
    pub blob_path: PathBuf,
    /// This fetch spent the last download → remove the file after reading it.
    pub burn: bool,
}

/// Reasons a claim can fail.
#[derive(Debug, PartialEq, Eq)]
pub enum MailboxError {
    NotFound,
    Expired,
    Exhausted,
    TooLarge,
    /// The relay is at capacity (too many entries / rows) — abuse/disk guard.
    Capacity,
    /// Revoke attempted with a missing/wrong token, or on a non-revocable entry.
    Forbidden,
    Backend(String),
}

impl std::fmt::Display for MailboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MailboxError::NotFound => write!(f, "no such claim"),
            MailboxError::Expired => write!(f, "expired"),
            MailboxError::Exhausted => write!(f, "download limit reached"),
            MailboxError::TooLarge => write!(f, "blob too large"),
            MailboxError::Capacity => write!(f, "relay at capacity"),
            MailboxError::Forbidden => write!(f, "not allowed (wrong or missing revoke token)"),
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
                downloads     INTEGER NOT NULL,
                revoke_hash   BLOB
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
            );
            CREATE TABLE IF NOT EXISTS inbox (
                slot        TEXT NOT NULL,
                id          TEXT NOT NULL,
                value       BLOB NOT NULL,
                expires_at  INTEGER NOT NULL,
                poster_hash BLOB,
                fetched_at  INTEGER,
                PRIMARY KEY (slot, id)
            );
            CREATE INDEX IF NOT EXISTS inbox_by_slot ON inbox (slot, expires_at);
            CREATE TABLE IF NOT EXISTS beacon (
                slot        TEXT PRIMARY KEY,
                expires_at  INTEGER NOT NULL
            );",
        )
        .map_err(backend)?;
        // Migrate pre-0.2 databases that predate the revoke_hash column. Adding a
        // column that already exists errors; that case means we're up to date.
        let _ = conn.execute("ALTER TABLE entries ADD COLUMN revoke_hash BLOB", []);
        // Migrate databases whose inbox predates the poster_hash column (lets an
        // offer's poster retract it — e.g. a live offer superseded by a fallback).
        let _ = conn.execute("ALTER TABLE inbox ADD COLUMN poster_hash BLOB", []);
        // Migrate databases whose inbox predates fetched_at (stamped when a live
        // recipient polls the offer, so the sender knows it was seen).
        let _ = conn.execute("ALTER TABLE inbox ADD COLUMN fetched_at INTEGER", []);
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
        if deposit.ciphertext.len() > max_blob_bytes() {
            return Err(MailboxError::TooLarge);
        }
        // Disk-fill guard: refuse new blobs once the store is at capacity.
        if self.len() as i64 >= max_entries() {
            return Err(MailboxError::Capacity);
        }
        // Clamp caller-supplied policy: bound the TTL (no immortal entries / no
        // i64 overflow) and the download budget.
        let ttl = deposit.ttl_secs.min(max_ttl_secs());
        let max_downloads = deposit.max_downloads.clamp(1, MAX_DOWNLOADS_CAP);
        let claim = random_claim();
        std::fs::write(self.blob_path(&claim), &deposit.ciphertext).map_err(backend)?;
        let revoke_hash: Option<Vec<u8>> = if deposit.revoke_hash.is_empty() {
            None
        } else {
            Some(deposit.revoke_hash)
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO entries
                (claim, encapped_key, expires_at, max_downloads, downloads, revoke_hash)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            params![
                claim,
                deposit.encapped_key,
                now.saturating_add(ttl) as i64,
                max_downloads as i64,
                revoke_hash,
            ],
        )
        .map_err(backend)?;
        Ok(claim)
    }

    /// Revoke an entry by claim, authorized by the sender's revoke token (whose
    /// BLAKE3 hash was recorded at deposit). Deletes the blob so it can no longer
    /// be fetched. Fails `Forbidden` if the entry isn't revocable or the token
    /// doesn't match; `NotFound` if there's no such claim.
    pub fn revoke(&self, claim: &str, token: &str) -> Result<(), MailboxError> {
        let conn = self.conn.lock().unwrap();
        let stored: Option<Vec<u8>> = conn
            .query_row(
                "SELECT revoke_hash FROM entries WHERE claim = ?1",
                params![claim],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => MailboxError::NotFound,
                other => backend(other),
            })?;
        let stored = stored.ok_or(MailboxError::Forbidden)?;
        let presented = blake3::hash(token.as_bytes());
        // Constant-time comparison (both are 32-byte BLAKE3 digests).
        if stored.len() != presented.as_bytes().len()
            || !constant_time_eq(&stored, presented.as_bytes())
        {
            return Err(MailboxError::Forbidden);
        }
        self.delete(&conn, claim)
    }

    /// The metadata half of a claim: validates and updates the download
    /// accounting **under the DB lock**, WITHOUT touching the (large) blob file.
    /// Returns what the caller needs to then read the file *off-lock* — so the
    /// SQLite mutex is never held across blocking file IO (which would stall
    /// every other relay request behind one slow/large download).
    ///
    /// When this fetch spends the last download it deletes the row here (claiming
    /// it, so a concurrent fetch sees `NotFound` — exactly-once burn-after-read)
    /// and sets [`FetchPlan::burn`] so the caller removes the file after reading.
    pub fn fetch_plan(&self, claim: &str, now: u64) -> Result<FetchPlan, MailboxError> {
        let conn = self.conn.lock().unwrap();
        let (encapped_key, expires_at, max_downloads, downloads) = conn
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

        if now >= expires_at as u64 {
            self.delete(&conn, claim)?;
            return Err(MailboxError::Expired);
        }
        if downloads >= max_downloads {
            self.delete(&conn, claim)?;
            return Err(MailboxError::Exhausted);
        }

        let burn = downloads + 1 >= max_downloads;
        if burn {
            // Claim the last download by removing the row now (the file is removed
            // by the caller after it reads it). This serializes exactly one winner.
            conn.execute("DELETE FROM entries WHERE claim = ?1", params![claim])
                .map_err(backend)?;
        } else {
            conn.execute(
                "UPDATE entries SET downloads = ?2 WHERE claim = ?1",
                params![claim, downloads + 1],
            )
            .map_err(backend)?;
        }
        Ok(FetchPlan {
            encapped_key,
            blob_path: self.blob_path(claim),
            burn,
        })
    }

    /// Claim a blob (synchronous convenience: [`fetch_plan`] + blocking file read).
    /// Async callers should use [`fetch_plan`] and read the file off-lock instead.
    pub fn fetch(&self, claim: &str, now: u64) -> Result<Claimed, MailboxError> {
        let plan = self.fetch_plan(claim, now)?;
        let ciphertext = std::fs::read(&plan.blob_path).map_err(backend)?;
        if plan.burn {
            let _ = std::fs::remove_file(&plan.blob_path);
        }
        Ok(Claimed {
            encapped_key: plan.encapped_key,
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

    /// Does a deposited entry still exist (and is unexpired)? Used by the sender
    /// to confirm delivery: within a short poll window an entry that is gone was
    /// almost certainly fetched (burn-after-read), not expired (TTL is days).
    pub fn entry_exists(&self, claim: &str, now: u64) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT 1 FROM entries WHERE claim = ?1 AND expires_at > ?2",
                params![claim, now as i64],
                |_| Ok(()),
            )
            .is_ok()
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

    /// Number of pending seeded chunk rows (disk-footprint guard).
    pub fn seeded_count(&self) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM seeded", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
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

    /// Number of rendezvous rows currently stored (capacity guard).
    pub fn rz_count(&self) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM rendezvous", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap_or(0)
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

    // ---- inbox (presence: offers to online peers) -------------------------

    /// Queue a sealed offer `value` in `slot` under `id`, expiring at `expires_at`.
    /// `poster_hash` (BLAKE3 of the poster's retract token, empty ⇒ not retractable)
    /// lets the original poster later delete this offer.
    pub fn inbox_put(
        &self,
        slot: &str,
        id: &str,
        value: &[u8],
        expires_at: u64,
        poster_hash: &[u8],
    ) -> Result<(), MailboxError> {
        let poster_hash: Option<&[u8]> = if poster_hash.is_empty() {
            None
        } else {
            Some(poster_hash)
        };
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO inbox (slot, id, value, expires_at, poster_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![slot, id, value, expires_at as i64, poster_hash],
            )
            .map_err(backend)?;
        Ok(())
    }

    /// Delete an offer if `token`'s BLAKE3 hash matches the stored poster hash.
    /// Returns whether a row was deleted. Lets a sender retract its own offer
    /// (e.g. a live offer replaced by an offline fallback) without the recipient's
    /// session — while a stranger, lacking the token, cannot.
    pub fn inbox_delete_by_poster(&self, slot: &str, id: &str, token: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        let stored: Option<Vec<u8>> = conn
            .query_row(
                "SELECT poster_hash FROM inbox WHERE slot = ?1 AND id = ?2",
                params![slot, id],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            )
            .ok()
            .flatten();
        let Some(stored) = stored else {
            return false;
        };
        let presented = blake3::hash(token.as_bytes());
        if stored.len() != presented.as_bytes().len()
            || !constant_time_eq(&stored, presented.as_bytes())
        {
            return false;
        }
        conn.execute(
            "DELETE FROM inbox WHERE slot = ?1 AND id = ?2",
            params![slot, id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    /// Mark the given offer `ids` in `slot` as fetched (delivered to a live,
    /// authenticated recipient poll) — first time only. Lets the poster learn
    /// its offer was actually seen by an online client.
    pub fn inbox_mark_fetched(&self, slot: &str, ids: &[String], now: u64) {
        let conn = self.conn.lock().unwrap();
        for id in ids {
            let _ = conn.execute(
                "UPDATE inbox SET fetched_at = ?3
                 WHERE slot = ?1 AND id = ?2 AND fetched_at IS NULL",
                params![slot, id, now as i64],
            );
        }
    }

    /// Status of an offer for its poster (authenticated by the retract token):
    /// `Gone` if it no longer exists (e.g. the recipient acked/accepted it),
    /// `BadToken` if the token doesn't match, else `Pending`/`Fetched`.
    pub fn inbox_status_by_poster(&self, slot: &str, id: &str, token: &str) -> InboxStatus {
        let conn = self.conn.lock().unwrap();
        let row: Option<(Option<Vec<u8>>, Option<i64>)> = conn
            .query_row(
                "SELECT poster_hash, fetched_at FROM inbox WHERE slot = ?1 AND id = ?2",
                params![slot, id],
                |r| Ok((r.get::<_, Option<Vec<u8>>>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .ok();
        let Some((stored, fetched)) = row else {
            return InboxStatus::Gone;
        };
        let Some(stored) = stored else {
            return InboxStatus::BadToken;
        };
        let presented = blake3::hash(token.as_bytes());
        if stored.len() != presented.as_bytes().len()
            || !constant_time_eq(&stored, presented.as_bytes())
        {
            return InboxStatus::BadToken;
        }
        if fetched.is_some() {
            InboxStatus::Fetched
        } else {
            InboxStatus::Pending
        }
    }

    /// All unexpired offers queued in `slot`, as `(id, value)` pairs.
    pub fn inbox_list(&self, slot: &str, now: u64) -> Vec<(String, Vec<u8>)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, value FROM inbox WHERE slot = ?1 AND expires_at > ?2 ORDER BY expires_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map(params![slot, now as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        });
        match rows {
            Ok(it) => it.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Delete a single offer from a slot (the recipient's ack after handling).
    pub fn inbox_delete(&self, slot: &str, id: &str) {
        let _ = self.conn.lock().unwrap().execute(
            "DELETE FROM inbox WHERE slot = ?1 AND id = ?2",
            params![slot, id],
        );
    }

    /// Total number of inbox rows (global disk-fill guard).
    pub fn inbox_count(&self) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM inbox", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
    }

    /// Number of unexpired offers already queued in one slot (per-slot flood guard).
    pub fn inbox_count_slot(&self, slot: &str, now: u64) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM inbox WHERE slot = ?1 AND expires_at > ?2",
                params![slot, now as i64],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
    }

    /// Delete all expired inbox rows; returns how many.
    pub fn inbox_reap(&self, now: u64) -> usize {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM inbox WHERE expires_at <= ?1",
                params![now as i64],
            )
            .unwrap_or(0)
    }

    // ---- presence beacons -------------------------------------------------

    /// Refresh (or create) a presence beacon for `slot`, expiring at `expires_at`.
    pub fn beacon_put(&self, slot: &str, expires_at: u64) -> Result<(), MailboxError> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO beacon (slot, expires_at) VALUES (?1, ?2)",
                params![slot, expires_at as i64],
            )
            .map_err(backend)?;
        Ok(())
    }

    /// Is there a live (unexpired) beacon for `slot`?
    pub fn beacon_alive(&self, slot: &str, now: u64) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT 1 FROM beacon WHERE slot = ?1 AND expires_at > ?2",
                params![slot, now as i64],
                |_| Ok(()),
            )
            .is_ok()
    }

    /// Number of beacon rows (disk-fill guard).
    pub fn beacon_count(&self) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM beacon", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
    }

    /// Delete all expired beacon rows; returns how many.
    pub fn beacon_reap(&self, now: u64) -> usize {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM beacon WHERE expires_at <= ?1",
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

/// Length-independent constant-time byte comparison (equal lengths only).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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

/// The browser secure-download page, its script, and the streaming service
/// worker. Decryption happens entirely client-side; the relay only serves
/// ciphertext.
const DL_HTML: &str = include_str!("web/dl.html");
const DL_JS: &str = include_str!("web/dl.js");
const DL_SW: &str = include_str!("web/arvolo-sw.js");

/// A strict Content-Security-Policy for the download page: no external
/// resources at all, only same-origin script/worker/frame and same-origin fetch
/// (to `/v1/fetch/{claim}`). `worker-src`/`frame-src` are for the streaming
/// service worker and the hidden download iframe it drives. No inline script.
const DL_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; \
     connect-src 'self'; img-src 'self' data:; worker-src 'self'; frame-src 'self'; \
     frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// Shown (with 403) when the administrator has disabled download links.
const DL_DISABLED_HTML: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>arvolo</title></head>\
<body style=\"font-family:system-ui,sans-serif;background:#0b0d10;color:#e7ebf0;display:grid;\
place-items:center;min-height:100vh;margin:0;padding:24px\"><main style=\"max-width:420px;text-align:center\">\
<h1 style=\"font-size:17px;margin:0 0 8px\">Download links are turned off</h1>\
<p style=\"color:#8b95a3;font-size:13px;line-height:1.5\">The administrator of this relay has disabled \
public browser download links. Ask the sender to share the file another way.</p></main></body></html>";

const LINKS_DISABLED_MSG: &str = "public download links are disabled by this relay's administrator";

fn links_disabled_page() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        DL_DISABLED_HTML,
    )
        .into_response()
}

async fn dl_page_handler(State(state): State<AppState>) -> Response {
    if !state.links_enabled {
        return links_disabled_page();
    }
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (axum::http::header::CONTENT_SECURITY_POLICY, DL_CSP),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        DL_HTML,
    )
        .into_response()
}

async fn dl_js_handler(State(state): State<AppState>) -> Response {
    if !state.links_enabled {
        return (StatusCode::FORBIDDEN, LINKS_DISABLED_MSG).into_response();
    }
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (axum::http::header::CONTENT_SECURITY_POLICY, DL_CSP),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        DL_JS,
    )
        .into_response()
}

/// The streaming download service worker. Served from the root path so its
/// default scope (`/`) covers the `/dl/stream/{id}` requests it must intercept.
async fn dl_sw_handler(State(state): State<AppState>) -> Response {
    if !state.links_enabled {
        return (StatusCode::FORBIDDEN, LINKS_DISABLED_MSG).into_response();
    }
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (
                axum::http::HeaderName::from_static("service-worker-allowed"),
                "/",
            ),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        DL_SW,
    )
        .into_response()
}

/// Advertise this relay's optional features so a client can fail fast. Currently
/// just `links` (public browser download links).
async fn features_handler(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        format!("{{\"links\":{}}}", state.links_enabled),
    )
}

/// Swarm tracker: register/refresh this peer for `swarm_id` and return a sample of
/// the others. `POST /v1/swarm/{swarm_id}/announce`. Zero-knowledge: the relay
/// stores node addresses + bitfields with a TTL, never the key or plaintext.
async fn swarm_announce_handler(
    State(state): State<AppState>,
    AxumPath(swarm_id): AxumPath<String>,
    Json(req): Json<AnnounceReq>,
) -> Response {
    if req.n_chunks > MAX_SWARM_CHUNKS
        || req.node_addr.is_empty()
        || req.node_addr.len() > 4096
        || req.bitfield.len() > (req.n_chunks as usize).div_ceil(8).max(1)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let now = now_unix();
    let want = (req.want as usize).min(MAX_SWARM_PEERS);
    let mut tracker = state.swarm.lock().unwrap();

    // Reject a brand-new swarm once we're tracking too many (memory guard).
    if !tracker.contains_key(&swarm_id) && tracker.len() >= MAX_SWARMS && req.event != "stopped" {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    let peers_map = tracker.entry(swarm_id.clone()).or_default();
    peers_map.retain(|_, e| e.expires_at > now);

    if req.event == "stopped" {
        peers_map.remove(&req.node_addr);
    } else if peers_map.len() < MAX_SWARM_PEERS || peers_map.contains_key(&req.node_addr) {
        peers_map.insert(
            req.node_addr.clone(),
            SwarmPeer {
                node_addr: req.node_addr.clone(),
                bitfield: req.bitfield.clone(),
                expires_at: now + SWARM_PEER_TTL_SECS,
            },
        );
    }

    let peers: Vec<PeerInfo> = peers_map
        .values()
        .filter(|e| e.node_addr != req.node_addr)
        .take(want)
        .map(|e| PeerInfo {
            node_addr: e.node_addr.clone(),
            bitfield: e.bitfield.clone(),
        })
        .collect();

    if peers_map.is_empty() {
        tracker.remove(&swarm_id);
    }
    Json(AnnounceResp { peers }).into_response()
}

/// Swarm tracker: list current peers for `swarm_id` without announcing.
/// `GET /v1/swarm/{swarm_id}/peers`.
async fn swarm_peers_handler(
    State(state): State<AppState>,
    AxumPath(swarm_id): AxumPath<String>,
) -> Response {
    let now = now_unix();
    let mut tracker = state.swarm.lock().unwrap();
    let peers = tracker
        .get_mut(&swarm_id)
        .map(|m| {
            m.retain(|_, e| e.expires_at > now);
            m.values()
                .take(MAX_SWARM_PEERS)
                .map(|e| PeerInfo {
                    node_addr: e.node_addr.clone(),
                    bitfield: e.bitfield.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Json(AnnounceResp { peers }).into_response()
}

/// Build the relay HTTP router over the shared [`AppState`].
///
/// A global request-body limit (`max_blob_bytes()`) is applied so the relay
/// never buffers an unbounded body into memory — the deposit path materializes
/// the whole body, so this is the primary OOM guard.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/deposit", post(deposit_handler))
        .route("/v1/features", get(features_handler))
        .route("/v1/fetch/{claim}", get(fetch_handler))
        .route("/v1/entry/{claim}", axum::routing::delete(revoke_handler))
        .route("/v1/entry/{claim}/status", get(entry_status_handler))
        .route("/v1/addr", get(addr_handler))
        .route("/v1/seed", post(seed_handler))
        .route("/v1/release/{token}/{hash}", post(release_handler))
        .route(
            "/v1/rz/{slot}/{key}",
            post(rz_post_handler).get(rz_get_handler),
        )
        .route(
            "/v1/inbox/{slot}",
            post(inbox_post_handler).get(inbox_get_handler),
        )
        .route("/v1/inbox/{slot}/session", post(inbox_session_handler))
        .route(
            "/v1/inbox/{slot}/{id}",
            axum::routing::delete(inbox_delete_handler),
        )
        .route("/v1/inbox/{slot}/{id}/status", get(inbox_status_handler))
        .route(
            "/v1/presence/{slot}",
            post(presence_post_handler).get(presence_get_handler),
        )
        // Swarm tracker (peer rendezvous for a shared arvc… ticket).
        .route("/v1/swarm/{swarm_id}/announce", post(swarm_announce_handler))
        .route("/v1/swarm/{swarm_id}/peers", get(swarm_peers_handler))
        // Browser secure-download page (E2E: decrypts client-side).
        .route("/dl/{claim}", get(dl_page_handler))
        .route("/dl.js", get(dl_js_handler))
        .route("/arvolo-sw.js", get(dl_sw_handler))
        .route("/healthz", get(|| async { "ok" }))
        // Applied after the routes so it wraps them all: bounds every request
        // body, so no handler can buffer an unbounded body into memory.
        .layer(axum::extract::DefaultBodyLimit::max(max_blob_bytes()))
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
    if body.len() > MAX_RZ_VALUE_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "rendezvous value too large".into(),
        ));
    }
    // Disk-fill guard on the (unauthenticated) rendezvous table.
    if state.mailbox.rz_count() >= max_rz_rows() {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "relay at capacity".into()));
    }
    let exp = now_unix().saturating_add(RZ_TTL);
    if key == RZ_CLAIM_KEY {
        let claimed = state
            .mailbox
            .rz_claim(&slot, &key, &body, exp)
            .map_err(err_response)?;
        if !claimed {
            return Err((StatusCode::CONFLICT, "slot already taken".into()));
        }
    } else {
        state
            .mailbox
            .rz_put(&slot, &key, &body, exp)
            .map_err(err_response)?;
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

/// How long a client may ask the relay to hold a GET open (long-poll), and the
/// poll granularity while it waits.
const INBOX_MAX_WAIT_SECS: u64 = 30;
const INBOX_POLL_MS: u64 = 500;
/// Lifetime of an inbox read/delete session token.
const INBOX_SESSION_TTL: u64 = 3600;
/// Length of the proof-of-possession nonce.
const INBOX_NONCE_LEN: usize = 16;

#[derive(Deserialize)]
struct InboxWait {
    #[serde(default)]
    wait: u64,
}

/// MAC binding a session nonce to its slot and expiry, keyed by the relay's
/// per-process secret. BLAKE3 keyed hash → 32 bytes.
fn inbox_mac(secret: &[u8; 32], slot: &str, nonce: &[u8], exp: u64) -> [u8; 32] {
    let mut input = Vec::with_capacity(slot.len() + nonce.len() + 8);
    input.extend_from_slice(slot.as_bytes());
    input.extend_from_slice(nonce);
    input.extend_from_slice(&exp.to_le_bytes());
    *blake3::keyed_hash(secret, &input).as_bytes()
}

/// Verify the `Authorization: Bearer <token>` proves ownership of `slot`:
/// the token's MAC must match one we issued for this slot, and be unexpired.
fn inbox_authorized(headers: &HeaderMap, slot: &str, secret: &[u8; 32]) -> bool {
    let Some(auth) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(token) = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
    else {
        return false;
    };
    let Some((nonce, exp, mac)) = arvolo_core::presence::decode_session_token(token.trim()) else {
        return false;
    };
    if now_unix() >= exp {
        return false;
    }
    let expected = inbox_mac(secret, slot, &nonce, exp);
    mac.len() == expected.len() && constant_time_eq(&mac, &expected)
}

/// Issue a proof-of-possession session: seal a random nonce to the presented
/// public key (only its owner can open it) and return the sealed nonce plus a
/// MAC binding it to this slot. The client echoes the opened nonce back as a
/// bearer token on subsequent read/delete requests.
async fn inbox_session_handler(
    State(state): State<AppState>,
    AxumPath(slot): AxumPath<String>,
    body: Bytes,
) -> Result<Bytes, (StatusCode, String)> {
    let pubid = arvolo_core::crypto::PublicId::from_bytes(&body)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid public id".into()))?;
    // Bind the request to the slot: only the key whose hash *is* this slot may
    // authenticate for it.
    if arvolo_core::presence::slot_for(&body) != slot {
        return Err((
            StatusCode::FORBIDDEN,
            "public id does not match slot".into(),
        ));
    }
    let nonce: [u8; INBOX_NONCE_LEN] = rand::random();
    let sealed = arvolo_core::presence::seal_session_nonce(&pubid, &nonce)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let exp = now_unix().saturating_add(INBOX_SESSION_TTL);
    let mac = inbox_mac(&state.auth_secret, &slot, &nonce, exp);
    let challenge = arvolo_core::presence::SessionChallenge {
        encapped_key: sealed.encapped_key,
        ciphertext: sealed.ciphertext,
        exp,
        mac: mac.to_vec(),
    };
    let bytes = arvolo_core::presence::encode_session_challenge(&challenge)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Bytes::from(bytes))
}

/// Optional `?ttl=` on an inbox deposit: an offline offer (pointing at a mailbox
/// blob) must outlive the short live-offer default, up to the blob TTL.
#[derive(Deserialize)]
struct InboxDepositQuery {
    ttl: Option<u64>,
}

/// Deposit a sealed offer in a recipient's inbox slot. Returns the relay-assigned
/// id (the recipient deletes by it after handling). An optional
/// `x-arvolo-poster-hash` header (base32 BLAKE3 of a retract token) lets the
/// poster later delete this offer.
async fn inbox_post_handler(
    State(state): State<AppState>,
    AxumPath(slot): AxumPath<String>,
    Query(q): Query<InboxDepositQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<String, (StatusCode, String)> {
    if body.len() > MAX_INBOX_VALUE_BYTES {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "offer too large".into()));
    }
    let now = now_unix();
    // Global disk-fill guard, then per-slot flood guard (one victim's inbox can't
    // be filled without bound, and the sender can't drown real offers).
    if state.mailbox.inbox_count() >= max_inbox_rows() {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "relay at capacity".into()));
    }
    if state.mailbox.inbox_count_slot(&slot, now) >= MAX_INBOX_PER_SLOT {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "inbox full".into()));
    }
    let poster_hash = headers
        .get(INBOX_POSTER_HASH_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            // The base32 alphabet is uppercase; accept a lowercased header too.
            data_encoding::BASE32_NOPAD
                .decode(s.trim().to_uppercase().as_bytes())
                .ok()
        })
        .unwrap_or_default();
    let id = random_claim();
    // Clamp a caller-requested TTL to the cap; default to the short live-offer TTL.
    let ttl = q.ttl.unwrap_or(INBOX_TTL_SECS).min(INBOX_MAX_TTL_SECS);
    let exp = now.saturating_add(ttl);
    state
        .mailbox
        .inbox_put(&slot, &id, &body, exp, &poster_hash)
        .map_err(err_response)?;
    Ok(id)
}

/// Refresh a presence beacon: mark this slot's owner online for `PRESENCE_TTL_SECS`.
/// Unauthenticated by design — a spoofed "online" only triggers the sender's
/// offline fallback, and no one can force another slot offline (there is no delete).
async fn presence_post_handler(
    State(state): State<AppState>,
    AxumPath(slot): AxumPath<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if state.mailbox.beacon_count() >= max_presence_rows()
        && !state.mailbox.beacon_alive(&slot, now_unix())
    {
        // At capacity and this is a new slot — refuse (refreshing an existing
        // beacon is always allowed so live clients don't drop offline).
        return Err((StatusCode::INSUFFICIENT_STORAGE, "relay at capacity".into()));
    }
    let exp = now_unix().saturating_add(PRESENCE_TTL_SECS);
    state.mailbox.beacon_put(&slot, exp).map_err(err_response)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Is the owner of this slot currently online? 200 if a live beacon exists, else 404.
async fn presence_get_handler(
    State(state): State<AppState>,
    AxumPath(slot): AxumPath<String>,
) -> StatusCode {
    if state.mailbox.beacon_alive(&slot, now_unix()) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

/// Long-poll a recipient's inbox. Returns immediately with any queued offers;
/// otherwise holds the connection up to `?wait=<secs>` (capped) before returning
/// an empty body. Reading does **not** burn — the recipient acks with DELETE.
async fn inbox_get_handler(
    State(state): State<AppState>,
    AxumPath(slot): AxumPath<String>,
    Query(q): Query<InboxWait>,
    headers: HeaderMap,
) -> Result<Bytes, (StatusCode, String)> {
    if !inbox_authorized(&headers, &slot, &state.auth_secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "inbox read requires a session".into(),
        ));
    }
    let deadline = now_unix().saturating_add(q.wait.min(INBOX_MAX_WAIT_SECS));
    loop {
        let rows = state.mailbox.inbox_list(&slot, now_unix());
        if !rows.is_empty() {
            // A live recipient is polling: mark these offers seen so their posters
            // can tell "delivered to an online client" from "stale presence".
            let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
            state.mailbox.inbox_mark_fetched(&slot, &ids, now_unix());
            let items: Vec<arvolo_core::presence::InboxItem> = rows
                .into_iter()
                .map(|(id, blob)| arvolo_core::presence::InboxItem { id, blob })
                .collect();
            let bytes = arvolo_core::presence::encode_inbox_items(&items)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            return Ok(Bytes::from(bytes));
        }
        if now_unix() >= deadline {
            return Ok(Bytes::new());
        }
        tokio::time::sleep(std::time::Duration::from_millis(INBOX_POLL_MS)).await;
    }
}

/// Delete an offer. Authorized either by the recipient's session token (their ack
/// after handling) or by the poster's retract token (the sender withdrawing its
/// own offer — e.g. a live offer superseded by an offline fallback).
async fn inbox_delete_handler(
    State(state): State<AppState>,
    AxumPath((slot, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> StatusCode {
    // Poster retract: present the token whose hash we stored at POST.
    if let Some(token) = headers
        .get(INBOX_POSTER_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        if state
            .mailbox
            .inbox_delete_by_poster(&slot, &id, token.trim())
        {
            return StatusCode::NO_CONTENT;
        }
    }
    // Recipient ack: owner of the slot, proven by a session token.
    if !inbox_authorized(&headers, &slot, &state.auth_secret) {
        return StatusCode::UNAUTHORIZED;
    }
    state.mailbox.inbox_delete(&slot, &id);
    StatusCode::NO_CONTENT
}

/// Poster-only status of one offer: has a live recipient seen it? Authorized by
/// the retract token (only the poster holds it). Body is a plain word:
/// `pending` / `fetched` / `gone`; a wrong/missing token is 401.
async fn inbox_status_handler(
    State(state): State<AppState>,
    AxumPath((slot, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Result<&'static str, StatusCode> {
    let token = headers
        .get(INBOX_POSTER_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    match state
        .mailbox
        .inbox_status_by_poster(&slot, &id, token.trim())
    {
        InboxStatus::BadToken => Err(StatusCode::UNAUTHORIZED),
        InboxStatus::Gone => Ok("gone"),
        InboxStatus::Pending => Ok("pending"),
        InboxStatus::Fetched => Ok("fetched"),
    }
}

/// Status of a deposited offline blob for its depositor: `pending` if still on the
/// relay, `gone` (404) if fetched (burn-after-read) or expired. The `claim` is a
/// secret capability, so this needs no extra auth.
async fn entry_status_handler(
    State(state): State<AppState>,
    AxumPath(claim): AxumPath<String>,
) -> Result<&'static str, StatusCode> {
    if state.mailbox.entry_exists(&claim, now_unix()) {
        Ok("pending")
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

fn status_for(e: &MailboxError) -> StatusCode {
    match e {
        MailboxError::NotFound => StatusCode::NOT_FOUND,
        MailboxError::Expired | MailboxError::Exhausted => StatusCode::GONE,
        MailboxError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        MailboxError::Capacity => StatusCode::INSUFFICIENT_STORAGE,
        MailboxError::Forbidden => StatusCode::FORBIDDEN,
        MailboxError::Backend(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Map a mailbox error to an HTTP response. Internal backend errors (SQL, IO)
/// are logged server-side but never echoed to the client, to avoid leaking
/// implementation detail; all other variants are safe, caller-facing states.
fn err_response(e: MailboxError) -> (StatusCode, String) {
    match e {
        MailboxError::Backend(detail) => {
            tracing::error!(%detail, "relay backend error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
        }
        other => (status_for(&other), other.to_string()),
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

    // A link deposit carries no HPKE recipient (empty encapped key). If the
    // administrator disabled links, refuse it here too (defense in depth beside
    // the /dl page and /v1/features advertisement).
    if !state.links_enabled && encapped_key.is_empty() {
        return Err((StatusCode::FORBIDDEN, LINKS_DISABLED_MSG.to_string()));
    }

    // Optional revoke-hash: base32 BLAKE3 of the sender's revoke token.
    let revoke_hash = headers
        .get(REVOKE_HASH_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            data_encoding::BASE32_NOPAD
                .decode(s.to_uppercase().as_bytes())
                .ok()
        })
        .unwrap_or_default();

    let deposit = Deposit {
        encapped_key,
        ciphertext: body.to_vec(),
        ttl_secs: q.ttl,
        max_downloads: q.max,
        revoke_hash,
    };
    mb.deposit(deposit, now_unix()).map_err(err_response)
}

/// Revoke (delete) an entry, authorized by the sender's revoke token supplied in
/// the `x-arvolo-revoke-token` header.
async fn revoke_handler(
    State(state): State<AppState>,
    AxumPath(claim): AxumPath<String>,
    headers: HeaderMap,
) -> Result<String, (StatusCode, String)> {
    let token = headers
        .get(REVOKE_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or((
            StatusCode::BAD_REQUEST,
            format!("missing {REVOKE_TOKEN_HEADER} header"),
        ))?;
    state.mailbox.revoke(&claim, token).map_err(err_response)?;
    Ok("revoked".into())
}

async fn fetch_handler(
    State(state): State<AppState>,
    AxumPath(claim): AxumPath<String>,
) -> Result<Response, (StatusCode, String)> {
    // Metadata decision under the DB lock (fast); the blob file is then read
    // off-lock and off the async worker via tokio::fs, so one large/slow download
    // can't hold the SQLite mutex and stall every other relay request.
    let plan = state
        .mailbox
        .fetch_plan(&claim, now_unix())
        .map_err(err_response)?;
    let ciphertext = tokio::fs::read(&plan.blob_path).await.map_err(|e| {
        tracing::error!(error = %e, "read blob file");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?;
    if plan.burn {
        let _ = tokio::fs::remove_file(&plan.blob_path).await;
    }
    let mut resp = ciphertext.into_response();
    let encoded = data_encoding::BASE32_NOPAD.encode(&plan.encapped_key);
    if let Ok(val) = encoded.parse() {
        resp.headers_mut().insert(ENCAPPED_KEY_HEADER, val);
    }
    Ok(resp)
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
    if req.chunks.len() > MAX_SEED_CHUNKS_PER_REQ {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("too many chunks (max {MAX_SEED_CHUNKS_PER_REQ})"),
        ));
    }
    // Aggregate disk-footprint guard for the unauthenticated seed path: refuse if
    // storing these chunks would exceed the total pending-seed cap.
    if state
        .mailbox
        .seeded_count()
        .saturating_add(req.chunks.len() as i64)
        > max_seeded_rows()
    {
        return Err((
            StatusCode::INSUFFICIENT_STORAGE,
            "relay at seed capacity".into(),
        ));
    }
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
            .map_err(err_response)?;
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
