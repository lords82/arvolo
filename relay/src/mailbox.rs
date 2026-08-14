//! The persistent zero-knowledge mailbox: SQLite metadata + ciphertext files.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection};

use crate::state::{
    max_blob_bytes, max_entries, max_total_blob_bytes, max_ttl_secs, MAX_DOWNLOADS_CAP,
};
use crate::util::{constant_time_eq, random_claim};

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
///
/// Three things can be true of an offer and they are not the same thing: nobody
/// has read it, a recipient's client holds it, and the recipient actually took the
/// file. The middle one used to be the end of the ladder — [`Self::Taken`] was
/// indistinguishable from an expiry, both reported as [`Self::Gone`] — which left
/// the sender's only positive signal carrying a claim it can't support: any
/// authenticated poll sets it, including one that merely *lists* what is waiting.
/// Giving "they took it" a state of its own is what lets the middle one go back to
/// answering only the question it can.
///
/// Note what is *not* here: whether a person looked. The relay sees reads of a
/// slot, never eyes on a screen, so no name on this ladder may imply one —
/// [`Self::Arrived`] is the most the middle state can honestly claim.
#[derive(Debug, PartialEq, Eq)]
pub enum InboxStatus {
    /// The offer is no longer here and was never taken: it expired, or its poster
    /// retracted it.
    Gone,
    /// The presented retract token does not match this offer.
    BadToken,
    /// Still queued: no recipient client has read it.
    Pending,
    /// A recipient's client has read the offer — it reached one of their devices.
    /// Says nothing about the person: a `recv`/`status` listing sets this exactly
    /// as a daemon poll does, and neither means anyone looked, let alone decided.
    Arrived,
    /// The recipient took it: the file was fetched and the offer acked. The only
    /// state that reports a human acting.
    Taken,
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
                taken_at    INTEGER,
                PRIMARY KEY (slot, id)
            );
            CREATE INDEX IF NOT EXISTS inbox_by_slot ON inbox (slot, expires_at);
            CREATE TABLE IF NOT EXISTS beacon (
                slot        TEXT PRIMARY KEY,
                expires_at  INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS session_bytes (
                swarm_id    TEXT PRIMARY KEY,
                bytes       INTEGER NOT NULL,
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
        // Migrate databases whose inbox predates taken_at. Before it, the ack that
        // follows a completed download deleted the row, so "they took it" and "it
        // expired" were the same answer. Rows already gone stay gone: an offer
        // taken before this column existed keeps reporting `Gone`, which is what
        // that relay always said.
        let _ = conn.execute("ALTER TABLE inbox ADD COLUMN taken_at INTEGER", []);
        // Migrate entries that predate the size column (backs the aggregate
        // stored-bytes cap; pre-existing rows count as 0 until they expire).
        let _ = conn.execute("ALTER TABLE entries ADD COLUMN size INTEGER", []);
        Ok(Self {
            conn: Mutex::new(conn),
            blob_dir,
        })
    }

    /// On-disk path of a blob's ciphertext. The streaming deposit handler writes
    /// here directly; `commit_deposit` then records the metadata row.
    pub fn blob_path(&self, claim: &str) -> PathBuf {
        self.blob_dir.join(format!("{claim}.bin"))
    }

    /// Whether the store is at its entry cap (disk-fill guard) — checked before a
    /// streaming deposit starts writing.
    pub fn at_capacity(&self) -> bool {
        self.len() as i64 >= max_entries()
    }

    /// Aggregate bytes of all stored blobs (per the metadata rows; rows that
    /// predate the size column count as 0 until they expire). Backs the
    /// `ARVOLO_MAX_TOTAL_BLOB_BYTES` disk-budget cap.
    pub fn stored_bytes(&self) -> u64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COALESCE(SUM(COALESCE(size, 0)), 0) FROM entries",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            .max(0) as u64
    }

    /// A fresh, random claim token to write a blob under.
    pub fn new_claim(&self) -> String {
        random_claim()
    }

    /// Record the metadata row for a blob whose ciphertext is already written at
    /// [`blob_path`](Self::blob_path). Clamps the caller's TTL (no immortal entries
    /// / no i64 overflow) and download budget.
    ///
    /// Returns the TTL **actually granted**, which is what the caller has to report
    /// back to the depositor. A relay run with a tight `ARVOLO_MAX_TTL` used to keep
    /// a fraction of what was asked without saying so, and a sender who believes
    /// they have a week posts an offer that outlives its own payload by six days.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_deposit(
        &self,
        claim: &str,
        encapped_key: Vec<u8>,
        ttl_secs: u64,
        max_downloads: u32,
        revoke_hash: Vec<u8>,
        size: u64,
        now: u64,
    ) -> Result<u64, MailboxError> {
        let ttl = ttl_secs.min(max_ttl_secs());
        let max_downloads = max_downloads.clamp(1, MAX_DOWNLOADS_CAP);
        let revoke_hash: Option<Vec<u8>> = if revoke_hash.is_empty() {
            None
        } else {
            Some(revoke_hash)
        };
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO entries
                (claim, encapped_key, expires_at, max_downloads, downloads, revoke_hash, size)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
                params![
                    claim,
                    encapped_key,
                    now.saturating_add(ttl) as i64,
                    max_downloads as i64,
                    revoke_hash,
                    size as i64,
                ],
            )
            .map_err(backend)?;
        Ok(ttl)
    }

    /// Store an in-memory `deposit`, returning a random claim token. Used by tests
    /// and any non-streaming caller; the HTTP path streams to disk instead. `now`
    /// is unix seconds. A `max_blob_bytes()` of 0 means unlimited.
    pub fn deposit(&self, deposit: Deposit, now: u64) -> Result<String, MailboxError> {
        let cap = max_blob_bytes();
        if cap != 0 && deposit.ciphertext.len() > cap {
            return Err(MailboxError::TooLarge);
        }
        if self.at_capacity() {
            return Err(MailboxError::Capacity);
        }
        // Aggregate disk-budget guard (`0` = unlimited).
        let total_cap = max_total_blob_bytes();
        if total_cap != 0
            && self
                .stored_bytes()
                .saturating_add(deposit.ciphertext.len() as u64)
                > total_cap
        {
            return Err(MailboxError::Capacity);
        }
        let claim = self.new_claim();
        let size = deposit.ciphertext.len() as u64;
        std::fs::write(self.blob_path(&claim), &deposit.ciphertext).map_err(backend)?;
        // The granted TTL is dropped here: this path has no wire response to carry
        // it on, and its callers pass a TTL they already know fits.
        let _granted = self.commit_deposit(
            &claim,
            deposit.encapped_key,
            deposit.ttl_secs,
            deposit.max_downloads,
            deposit.revoke_hash,
            size,
            now,
        )?;
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

    /// Download accounting for a live (unexpired) entry: `(downloads, max)`.
    /// `None` if the claim is unknown or expired. Lets a depositor see how many
    /// times a link/mailbox blob has been fetched.
    pub fn entry_counts(&self, claim: &str, now: u64) -> Option<(u32, u32)> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT downloads, max_downloads FROM entries \
                 WHERE claim = ?1 AND expires_at > ?2",
                params![claim, now as i64],
                |row| {
                    let d: i64 = row.get(0)?;
                    let m: i64 = row.get(1)?;
                    Ok((d.max(0) as u32, m.max(0) as u32))
                },
            )
            .ok()
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

    // ---- per-session relay-offload metering -------------------------------

    /// Bytes already offloaded to the relay for `swarm_id` within the live TTL
    /// window (0 if unknown / expired).
    pub fn session_bytes(&self, swarm_id: &str, now: u64) -> u64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT bytes FROM session_bytes WHERE swarm_id = ?1 AND expires_at > ?2",
                params![swarm_id, now as i64],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            .max(0) as u64
    }

    /// Add `delta` bytes to `swarm_id`'s running total and (re)set its expiry.
    /// Cumulative within the TTL window: the row is reaped once no bytes are added
    /// for the seed TTL, so a genuinely new transfer of the same file later starts
    /// fresh while a suspend/resume/restart keeps counting toward the same cap.
    pub fn add_session_bytes(
        &self,
        swarm_id: &str,
        delta: u64,
        expires_at: u64,
    ) -> Result<(), MailboxError> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO session_bytes (swarm_id, bytes, expires_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(swarm_id) DO UPDATE SET
                     bytes = bytes + excluded.bytes,
                     expires_at = excluded.expires_at",
                params![swarm_id, delta as i64, expires_at as i64],
            )
            .map_err(backend)?;
        Ok(())
    }

    /// Drop per-session meters whose TTL has passed.
    pub fn reap_session_bytes(&self, now: u64) {
        let _ = self.conn.lock().unwrap().execute(
            "DELETE FROM session_bytes WHERE expires_at <= ?1",
            params![now as i64],
        );
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

    /// Push every unexpired row of `slot` out to `expires_at`, unless it is
    /// already later. Returns how many rows moved. This is the whole of slot
    /// renewal: a long-lived rendezvous stays alive because its owner keeps
    /// asking for its sessions, not because it holds a separate lease.
    pub fn rz_touch_slot(&self, slot: &str, expires_at: u64, now: u64) -> usize {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE rendezvous SET expires_at = ?2
                 WHERE slot = ?1 AND expires_at > ?3 AND expires_at < ?2",
                params![slot, expires_at as i64, now as i64],
            )
            .unwrap_or(0)
    }

    /// Delete named keys of one slot. Exact keys only — never a `LIKE` pattern
    /// built from a peer-supplied session id, which `_`/`%` would turn into a
    /// wildcard that reaches into other sessions' rows.
    pub fn rz_delete_keys(&self, slot: &str, keys: &[String]) {
        let conn = self.conn.lock().unwrap();
        for key in keys {
            let _ = conn.execute(
                "DELETE FROM rendezvous WHERE slot = ?1 AND key = ?2",
                params![slot, key],
            );
        }
    }

    /// Unexpired keys of `slot` starting with `prefix`. The prefix is always one
    /// of our own constants (`r.`, `s.`), never peer input — so the `LIKE` pattern
    /// below carries no wildcard a caller could have chosen.
    pub fn rz_slot_keys_prefixed(&self, slot: &str, prefix: &str, now: u64) -> Vec<String> {
        let pattern = format!("{prefix}%");
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT key FROM rendezvous
             WHERE slot = ?1 AND key LIKE ?2 AND expires_at > ?3
             ORDER BY key",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map(params![slot, pattern, now as i64], |r| {
            r.get::<_, String>(0)
        });
        match rows {
            Ok(it) => it.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
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

    /// Number of unexpired rows held by one slot (per-slot capacity guard).
    ///
    /// The fixed three-key pairing could never grow a slot, so nothing bounded it.
    /// A slot whose keys are chosen by whoever shows up can, so every write past
    /// the claim is checked against [`MAX_RZ_KEYS_PER_SLOT`]: guessing a 4-digit
    /// nameplate must not buy an unbounded number of rows.
    pub fn rz_slot_row_count(&self, slot: &str, now: u64) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM rendezvous WHERE slot = ?1 AND expires_at > ?2",
                params![slot, now as i64],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
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

    /// Mark the given offer `ids` in `slot` as read by a live, authenticated
    /// recipient poll — first time only. Lets the poster tell a client that is
    /// really there from stale presence.
    ///
    /// Deliberately *not* named for delivery: a listing sets this exactly as a
    /// daemon poll does, so all it can honestly report is that the offer reached
    /// one of the recipient's devices. Whether the person then took it is
    /// [`Self::inbox_mark_taken`]'s answer to give. (The column keeps its original
    /// name — renaming it would be a migration for nothing.)
    pub fn inbox_mark_arrived(&self, slot: &str, ids: &[String], now: u64) {
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
    /// `Gone` if the row isn't here, `BadToken` if the token doesn't match, else
    /// the ladder `Pending` → `Arrived` → `Taken`.
    pub fn inbox_status_by_poster(&self, slot: &str, id: &str, token: &str) -> InboxStatus {
        /// One offer's row, as the status question needs it: who may ask, and the
        /// two stamps that place it on the ladder. (`fetched_at` is the original
        /// column name for the arrival stamp — see [`Mailbox::inbox_mark_arrived`].)
        struct StatusRow {
            poster_hash: Option<Vec<u8>>,
            arrived_at: Option<i64>,
            taken_at: Option<i64>,
        }

        let conn = self.conn.lock().unwrap();
        let row: Option<StatusRow> = conn
            .query_row(
                "SELECT poster_hash, fetched_at, taken_at FROM inbox WHERE slot = ?1 AND id = ?2",
                params![slot, id],
                |r| {
                    Ok(StatusRow {
                        poster_hash: r.get(0)?,
                        arrived_at: r.get(1)?,
                        taken_at: r.get(2)?,
                    })
                },
            )
            .ok();
        let Some(StatusRow {
            poster_hash: stored,
            arrived_at: arrived,
            taken_at: taken,
        }) = row
        else {
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
        // Taken outranks arrived: taking it implies having read it, and the later
        // fact is the one worth reporting.
        if taken.is_some() {
            InboxStatus::Taken
        } else if arrived.is_some() {
            InboxStatus::Arrived
        } else {
            InboxStatus::Pending
        }
    }

    /// All unexpired offers queued in `slot`, as `(id, value)` pairs. Tombstones
    /// of already-taken offers are excluded — they exist only to answer the
    /// poster, and handing one back would offer the recipient the same file twice
    /// (with an emptied payload, at that).
    pub fn inbox_list(&self, slot: &str, now: u64) -> Vec<(String, Vec<u8>)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, value FROM inbox
             WHERE slot = ?1 AND expires_at > ?2 AND taken_at IS NULL
             ORDER BY expires_at",
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

    /// The recipient's ack, once they have actually taken the file.
    ///
    /// Not a delete: the row becomes a **tombstone** so the poster can be told
    /// `Taken` rather than the `Gone` that an expiry also produces — the one thing
    /// a sender most wants to know was, until now, the same answer as "it lapsed".
    /// The sealed payload is dropped (it can be half a megabyte and nobody will
    /// read it again); what stays is the stamp and the poster hash that authorises
    /// the question. It costs nothing to keep: `expires_at` is untouched, so the
    /// existing reaper collects it at the TTL the offer already had.
    pub fn inbox_mark_taken(&self, slot: &str, id: &str, now: u64) {
        let _ = self.conn.lock().unwrap().execute(
            "UPDATE inbox SET taken_at = ?3, value = x''
             WHERE slot = ?1 AND id = ?2 AND taken_at IS NULL",
            params![slot, id, now as i64],
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
    ///
    /// Tombstones don't count. The guard exists so one victim's inbox can't be
    /// filled against them, and an offer they have already dealt with is not
    /// occupying anything — counting their own tombstones would let a busy
    /// recipient lock their own slot at the cap and turn "inbox full" into a
    /// punishment for using it.
    pub fn inbox_count_slot(&self, slot: &str, now: u64) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM inbox
                 WHERE slot = ?1 AND expires_at > ?2 AND taken_at IS NULL",
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

#[cfg(test)]
mod stored_bytes_tests {
    use super::*;

    #[test]
    fn stored_bytes_tracks_deposits_and_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Mailbox::open(
            dir.path().join("db.sqlite").to_str().unwrap(),
            dir.path().join("blobs").to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(mb.stored_bytes(), 0);

        let claim = mb
            .deposit(
                Deposit {
                    encapped_key: vec![1, 2, 3],
                    ciphertext: vec![0u8; 1000],
                    ttl_secs: 60,
                    max_downloads: 1,
                    revoke_hash: Vec::new(),
                },
                1,
            )
            .unwrap();
        mb.deposit(
            Deposit {
                encapped_key: vec![4, 5, 6],
                ciphertext: vec![0u8; 500],
                ttl_secs: 60,
                max_downloads: 1,
                revoke_hash: Vec::new(),
            },
            1,
        )
        .unwrap();
        assert_eq!(mb.stored_bytes(), 1500);

        // A burn-after-read fetch deletes the row and frees its budget.
        let _plan = mb.fetch_plan(&claim, 2).unwrap();
        assert_eq!(mb.stored_bytes(), 500);
    }
}
