//! Resumable uploads: a deposit that arrives in pieces.
//!
//! The single-request deposit (`POST /v1/deposit`) streams a whole file in one
//! body, and a body that breaks off is thrown away — the relay cannot tell a
//! truncated upload from a finished one, so the partial is deleted and the
//! sender starts again from the first byte. On a phone, on a train, or simply
//! with a file that takes longer to send than the network stays up, that is
//! the difference between arriving and never arriving.
//!
//! An upload session splits the same deposit into three steps: **open** it,
//! declaring the full length up front (so the size caps and the disk budget are
//! settled before a byte is sent, and the space is reserved), **append** to it
//! in as many requests as it takes, each one saying where it starts, and
//! **commit** it, which turns the finished file into an ordinary mailbox entry
//! — the same row, the same claim, the same fetch path as a single-request
//! deposit. Nothing downstream knows how the bytes arrived.
//!
//! The session id is a capability, like a claim: whoever holds it can append
//! to, inspect, commit or abandon the upload, and nobody else can find it. The
//! ciphertext is still opaque here; the relay learns nothing new.
//!
//! How much has arrived is read off the partial file itself, not kept in a
//! column: the file is the truth after a crash, and a counter that drifted from
//! it would make the client resume from the wrong place — which, for a stream
//! of authenticated chunks, is a download that fails at the end.

use std::collections::HashSet;
use std::sync::Mutex;

use rusqlite::params;

use super::{backend, Mailbox, MailboxError};
use crate::state::env_usize;
use crate::util::random_claim;

/// How long an open upload may sit without a byte arriving before the reaper
/// takes it back, partial file and reservation together. Long enough to ride
/// out a night with the laptop closed; short enough that an abandoned upload
/// does not hold disk budget hostage for a week.
pub const UPLOAD_IDLE_SECS: u64 = 24 * 3600;

/// How long a committed session is remembered after its commit, so a client
/// whose commit answer was lost can ask again and get the same claim instead
/// of an error — or, worse, a second copy.
pub const UPLOAD_COMMITTED_GRACE_SECS: u64 = 3600;

/// Default cap on uploads open at once, across every client.
pub const DEFAULT_MAX_OPEN_UPLOADS: usize = 256;

fn max_open_uploads() -> i64 {
    env_usize("ARVOLO_MAX_OPEN_UPLOADS", DEFAULT_MAX_OPEN_UPLOADS) as i64
}

/// One upload, as a client asking about it needs to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadInfo {
    /// The full ciphertext length declared when the session was opened.
    pub length: u64,
    /// Bytes already on disk: where the next append must start.
    pub received: u64,
    /// The claim and granted TTL, once committed.
    pub committed: Option<(String, u64)>,
}

/// The parameters fixed when an upload is opened — everything a single-request
/// deposit carries in its query and headers, plus the length.
pub struct UploadOpen {
    pub encapped_key: Vec<u8>,
    pub revoke_hash: Vec<u8>,
    pub ttl_secs: u64,
    pub max_downloads: u32,
    pub length: u64,
}

/// Held while one request appends to an upload. Two appends racing on the
/// same file would interleave their bytes; the second is turned away instead.
pub struct AppendGuard<'a> {
    busy: &'a Mutex<HashSet<String>>,
    id: String,
}

impl Drop for AppendGuard<'_> {
    fn drop(&mut self) {
        self.busy.lock().unwrap().remove(&self.id);
    }
}

impl Mailbox {
    pub(super) fn init_uploads(conn: &rusqlite::Connection) -> Result<(), MailboxError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS uploads (
                id            TEXT PRIMARY KEY,
                encapped_key  BLOB NOT NULL,
                revoke_hash   BLOB,
                ttl           INTEGER NOT NULL,
                max_downloads INTEGER NOT NULL,
                length        INTEGER NOT NULL,
                idle_expires  INTEGER NOT NULL,
                claim         TEXT,
                granted       INTEGER
            );",
        )
        .map_err(backend)
    }

    /// Where an upload's partial ciphertext lives until it is committed.
    pub fn upload_path(&self, id: &str) -> std::path::PathBuf {
        self.blob_dir.join(format!("{id}.part"))
    }

    /// Bytes promised to uploads still open: the full declared length of each,
    /// not what has arrived so far. Counted against the disk budget from the
    /// moment the session opens, so a dozen large uploads that each fit on
    /// their own cannot together promise more disk than there is.
    pub fn reserved_upload_bytes(&self) -> u64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COALESCE(SUM(length), 0) FROM uploads WHERE claim IS NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            .max(0) as u64
    }

    /// How many uploads are open (not yet committed).
    pub fn open_uploads(&self) -> usize {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM uploads WHERE claim IS NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n.max(0) as usize)
            .unwrap_or(0)
    }

    /// Open an upload session and return its id. The caller has already checked
    /// the size caps and the budget; this only enforces the count of open
    /// sessions, which is this module's own bound.
    pub fn open_upload(&self, open: UploadOpen, now: u64) -> Result<String, MailboxError> {
        let conn = self.conn.lock().unwrap();
        let open_now: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM uploads WHERE claim IS NULL",
                [],
                |r| r.get(0),
            )
            .map_err(backend)?;
        if open_now >= max_open_uploads() {
            return Err(MailboxError::Capacity);
        }
        let id = random_claim();
        std::fs::File::create(self.upload_path(&id)).map_err(backend)?;
        let revoke_hash: Option<Vec<u8>> =
            (!open.revoke_hash.is_empty()).then_some(open.revoke_hash);
        conn.execute(
            "INSERT INTO uploads
                (id, encapped_key, revoke_hash, ttl, max_downloads, length, idle_expires)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                open.encapped_key,
                revoke_hash,
                open.ttl_secs.min(i64::MAX as u64) as i64,
                open.max_downloads as i64,
                open.length.min(i64::MAX as u64) as i64,
                now.saturating_add(UPLOAD_IDLE_SECS) as i64,
            ],
        )
        .map_err(|e| {
            let _ = std::fs::remove_file(self.upload_path(&id));
            backend(e)
        })?;
        Ok(id)
    }

    /// Where an upload stands. `NotFound` for an id this relay never issued,
    /// one already reaped, and one idle past its deadline that the reaper has
    /// not reached yet — the client's answer to all three is the same: start a
    /// new session.
    pub fn upload_info(&self, id: &str, now: u64) -> Result<UploadInfo, MailboxError> {
        let (length, idle_expires, claim, granted) = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT length, idle_expires, claim, granted FROM uploads WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => MailboxError::NotFound,
                other => backend(other),
            })?;
        let length = length.max(0) as u64;
        if let Some(claim) = claim {
            return Ok(UploadInfo {
                length,
                received: length,
                committed: Some((claim, granted.unwrap_or(0).max(0) as u64)),
            });
        }
        if idle_expires as u64 <= now {
            return Err(MailboxError::NotFound);
        }
        let received = std::fs::metadata(self.upload_path(id))
            .map(|m| m.len())
            .map_err(|_| MailboxError::NotFound)?;
        Ok(UploadInfo {
            length,
            received: received.min(length),
            committed: None,
        })
    }

    /// Claim the right to append to `id`, or `None` if another request is
    /// already appending to it.
    pub fn begin_append(&self, id: &str) -> Option<AppendGuard<'_>> {
        let mut busy = self.uploads_busy.lock().unwrap();
        if !busy.insert(id.to_string()) {
            return None;
        }
        Some(AppendGuard {
            busy: &self.uploads_busy,
            id: id.to_string(),
        })
    }

    /// Push an open upload's idle deadline forward: bytes are still arriving.
    pub fn touch_upload(&self, id: &str, now: u64) {
        let _ = self.conn.lock().unwrap().execute(
            "UPDATE uploads SET idle_expires = ?2 WHERE id = ?1 AND claim IS NULL",
            params![id, now.saturating_add(UPLOAD_IDLE_SECS) as i64],
        );
    }

    /// Turn a complete upload into a mailbox entry, returning its claim and the
    /// TTL actually granted. Idempotent: committing an upload already committed
    /// answers with the same claim.
    ///
    /// `Ok(Err(received))` when the file is not complete yet — the caller
    /// reports where it stands rather than an error, since the client's next
    /// move is simply to send the rest.
    pub fn commit_upload(
        &self,
        id: &str,
        now: u64,
    ) -> Result<Result<(String, u64), u64>, MailboxError> {
        let info = self.upload_info(id, now)?;
        if let Some(done) = info.committed {
            return Ok(Ok(done));
        }
        if info.received != info.length {
            return Ok(Err(info.received));
        }
        let (encapped_key, revoke_hash, ttl, max_downloads) = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT encapped_key, revoke_hash, ttl, max_downloads FROM uploads WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, Option<Vec<u8>>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .map_err(backend)?;
        let claim = self.new_claim();
        std::fs::rename(self.upload_path(id), self.blob_path(&claim)).map_err(backend)?;
        let granted = match self.commit_deposit(
            &claim,
            encapped_key,
            ttl.max(0) as u64,
            max_downloads.clamp(0, u32::MAX as i64) as u32,
            revoke_hash.unwrap_or_default(),
            info.length,
            now,
        ) {
            Ok(g) => g,
            Err(e) => {
                // Put the file back where the session expects it, so a retry
                // of the commit finds a complete upload and not a missing one.
                let _ = std::fs::rename(self.blob_path(&claim), self.upload_path(id));
                return Err(e);
            }
        };
        self.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE uploads SET claim = ?2, granted = ?3, idle_expires = ?4 WHERE id = ?1",
                params![
                    id,
                    claim,
                    granted as i64,
                    now.saturating_add(UPLOAD_COMMITTED_GRACE_SECS) as i64
                ],
            )
            .map_err(backend)?;
        Ok(Ok((claim, granted)))
    }

    /// Abandon an upload: its partial file and its reservation go now rather
    /// than at the idle deadline. A committed session only loses its memo; the
    /// entry it became is revoked the ordinary way.
    pub fn abort_upload(&self, id: &str) -> Result<(), MailboxError> {
        let conn = self.conn.lock().unwrap();
        let committed: Option<String> = conn
            .query_row(
                "SELECT claim FROM uploads WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => MailboxError::NotFound,
                other => backend(other),
            })?;
        if committed.is_none() {
            let _ = std::fs::remove_file(self.upload_path(id));
        }
        conn.execute("DELETE FROM uploads WHERE id = ?1", params![id])
            .map_err(backend)?;
        Ok(())
    }

    /// Drop sessions past their deadline: idle uploads with their partial
    /// files, and committed memos whose grace has run out.
    pub fn upload_reap(&self, now: u64) -> usize {
        let conn = self.conn.lock().unwrap();
        let Ok(mut stmt) = conn.prepare("SELECT id, claim FROM uploads WHERE idle_expires <= ?1")
        else {
            return 0;
        };
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map(params![now as i64], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default();
        drop(stmt);
        for (id, claim) in &rows {
            // An upload being appended to right now is not idle, whatever its
            // deadline says; leave it for the next pass.
            if self.uploads_busy.lock().unwrap().contains(id) {
                continue;
            }
            if claim.is_none() {
                let _ = std::fs::remove_file(self.upload_path(id));
            }
            let _ = conn.execute("DELETE FROM uploads WHERE id = ?1", params![id]);
        }
        rows.len()
    }

    /// Every open upload id; `None` if the table could not be read.
    pub(super) fn open_upload_ids(&self) -> Option<HashSet<String>> {
        self.string_column("SELECT id FROM uploads WHERE claim IS NULL")
    }
}
