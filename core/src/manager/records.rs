use std::path::{Path, PathBuf};

/// On-disk record of an accepted, not-yet-finished chunked download, so the
/// daemon can resume it after a restart. Small (a ticket + a path) — one postcard
/// file per download under the manager's `state_dir`.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct DownloadRecord {
    pub(super) id: u64,
    pub(super) ticket: String,
    pub(super) out_path: String,
    pub(super) name: String,
    pub(super) size: u64,
}

pub(super) fn download_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("dl-{id}.pc"))
}

/// Write a resume record with owner-only permissions (`0o600`) on unix. These
/// records embed the ticket, which for a `Plain` key delivery carries the file's
/// content key in the clear — so another local user must not be able to read them.
/// Non-unix keeps the default perms (accepted limitation).
pub(super) fn write_record_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub(super) fn persist_download(dir: &Path, rec: &DownloadRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&download_record_path(dir, rec.id), &bytes);
    }
}

/// Removes the resume record **and** its paused marker together, so a terminal
/// download never leaves a stray "paused" flag behind that a restart would honor.
pub(super) fn remove_download(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(download_record_path(dir, id));
    clear_paused(dir, id);
}

/// Load one download resume record by id (`None` if absent/undecodable) — what
/// [`super::TransferManager::resume`] reads to restart a paused download.
pub(super) fn load_download(dir: &Path, id: u64) -> Option<DownloadRecord> {
    let bytes = std::fs::read(download_record_path(dir, id)).ok()?;
    postcard::from_bytes(&bytes).ok()
}

pub(super) fn load_downloads(dir: &Path) -> Vec<DownloadRecord> {
    load_records(dir, "dl-")
}

/// A download's "the user paused this" flag, kept as a **sidecar marker file**
/// (`dl-paused-<id>`) rather than a field on [`DownloadRecord`]. Two reasons: the
/// record is postcard (not self-describing), so adding a field would silently drop
/// every download already in flight at an upgrade; and a marker is atomic to set and
/// clear. Present iff the paused download's record is present — [`remove_download`]
/// clears both — so the two never desync. Empty file: it carries no secret, only its
/// existence matters.
pub(super) fn paused_marker_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("dl-paused-{id}"))
}

pub(super) fn mark_paused(dir: &Path, id: u64) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(paused_marker_path(dir, id), []);
}

pub(super) fn clear_paused(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(paused_marker_path(dir, id));
}

pub(super) fn is_paused_marked(dir: &Path, id: u64) -> bool {
    paused_marker_path(dir, id).exists()
}

// `load-<prefix>` scan skips our marker files: they have no `.pc` extension, and
// `load_records` only reads records it can decode — a marker decodes to nothing.

/// On-disk record of an active send (serving an anonymous `arvc…` ticket), so the
/// daemon can resume serving after a restart. Stores the file path, the ticket
/// (which carries the content key + chunk hashes + name), and the node seed — so
/// the same node id and hashes are reproduced and the ticket already handed out
/// keeps working.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct SendRecord {
    pub(super) id: u64,
    pub(super) path: String,
    pub(super) node_seed: Vec<u8>,
    pub(super) ticket: String,
    /// A file this send *owns* and should delete when it's removed (cancel /
    /// finish) — e.g. the staged archive tar a seeder keeps only to serve. `None`
    /// for a normal send of a user's own file (never delete that).
    #[serde(default)]
    pub(super) owned_stage: Option<String>,
}

pub(super) fn send_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("send-{id}.pc"))
}

pub(super) fn persist_send(dir: &Path, rec: &SendRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&send_record_path(dir, rec.id), &bytes);
    }
}

pub(super) fn remove_send(dir: &Path, id: u64) {
    let path = send_record_path(dir, id);
    // Delete any file this send owns (a staged archive tar we kept only to seed)
    // before dropping the record, so a cancelled session leaves nothing behind.
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(rec) = postcard::from_bytes::<SendRecord>(&bytes) {
            if let Some(stage) = &rec.owned_stage {
                let _ = std::fs::remove_file(stage);
            }
        }
    }
    let _ = std::fs::remove_file(path);
}

pub(super) fn load_sends(dir: &Path) -> Vec<SendRecord> {
    load_records(dir, "send-")
}

/// On-disk record of an in-progress `send --to` delivery, so the daemon can
/// restore it after a restart: a paused one comes back paused, an active one
/// resumes delivering. Small (a recipient id + a path + status).
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct SendToRecord {
    pub(super) id: u64,
    pub(super) recipient: Vec<u8>,
    pub(super) payload: String,
    pub(super) name: String,
    pub(super) archive: bool,
    pub(super) paused: bool,
    pub(super) reason: String,
    #[serde(default)]
    pub(super) note: String,
}

pub(super) fn sendto_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("sendto-{id}.pc"))
}

pub(super) fn persist_sendto(dir: &Path, rec: &SendToRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&sendto_record_path(dir, rec.id), &bytes);
    }
}

pub(super) fn remove_sendto(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(sendto_record_path(dir, id));
}

pub(super) fn load_sendtos(dir: &Path) -> Vec<SendToRecord> {
    load_records(dir, "sendto-")
}

/// On-disk record of a mailbox **deposit awaiting pickup**, so a daemon restart
/// keeps showing the transfer as Deposited (and keeps confirming its delivery)
/// instead of silently dropping it from the list. Removed once the pickup is
/// confirmed, or skipped on load after the relay-side TTL has lapsed.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct DepositedRecord {
    pub(super) id: u64,
    pub(super) recipient: Vec<u8>,
    pub(super) name: String,
    pub(super) size: u64,
    pub(super) relay: String,
    pub(super) claim: String,
    /// Unix seconds when the relay auto-expires the blob (deposit time + TTL).
    pub(super) expires: u64,
    /// Sender-only secret that authorizes deleting the blob from the relay, so a
    /// cancel can actually withdraw the file instead of only hiding the row.
    pub(super) revoke_token: String,
    /// The posted offer sitting in the recipient's inbox, and the token that
    /// authorizes retracting it (so a cancel also removes their pickup notice).
    pub(super) offer_id: String,
    pub(super) poster_token: String,
}

// NOTE on evolving these records: postcard is a compact, **non-self-describing**
// format — fields are read positionally and `#[serde(default)]` does nothing for
// a missing one. Adding or reordering a field therefore makes every previously
// written record of that type unparsable; `load_records` skips those silently, so
// the state they tracked is quietly forgotten. If one of these structs ever needs
// a new field after a release, bump the filename prefix (e.g. `dep2-`) and migrate
// the old files, rather than editing the struct in place.

pub(super) fn deposited_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("dep-{id}.pc"))
}

pub(super) fn persist_deposited(dir: &Path, rec: &DepositedRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&deposited_record_path(dir, rec.id), &bytes);
    }
}

pub(super) fn remove_deposited(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(deposited_record_path(dir, id));
}

pub(super) fn load_depositeds(dir: &Path) -> Vec<DepositedRecord> {
    load_records(dir, "dep-")
}

/// Read every postcard record whose filename starts with `prefix` from `dir`.
pub(super) fn load_records<T: serde::de::DeserializeOwned>(dir: &Path, prefix: &str) -> Vec<T> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_match = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with(prefix) && n.ends_with(".pc"))
            .unwrap_or(false);
        if !is_match {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(rec) = postcard::from_bytes::<T>(&bytes) {
                out.push(rec);
            }
        }
    }
    out
}
