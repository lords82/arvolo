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

/// What a share has actually done: kept **beside** its [`SendRecord`], not inside
/// it.
///
/// The reason is the same one that made the paused flag a marker file. These
/// records are postcard, which is not self-describing: a new field on `SendRecord`
/// makes every record an older build wrote fail to decode, and `load_records`
/// silently skips what it can't read — so an upgrade would quietly stop resuming
/// the shares that already existed, and leak the staged tars they own. A sidecar
/// has no such failure mode: absent simply means "no counters yet, start at zero",
/// which is also exactly right for a share that predates them.
///
/// Aggregates only, deliberately. Who fetched a file is not recorded — an
/// anonymous ticket carries no identity to record in the first place, and keeping
/// a log of other people's activity on someone's disk is a decision to be taken on
/// purpose, not arrived at by collecting whatever was easy to collect.
#[derive(Default, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(super) struct ShareRecord {
    /// Receivers that fetched **every** chunk: one per distinct receiving node,
    /// counted from [`crate::flow::SendEvent::Delivered`].
    ///
    /// Not "people" — an anonymous ticket carries no identity, so this counts nodes,
    /// and the same machine coming back to re-fetch is the one it already counted.
    /// (It used to count at most one per serving session whatever happened, which
    /// made a `--share-copies` limit above 1 unreachable.)
    pub(super) copies_served: u64,
    /// Bytes uploaded for this file, across every receiver. An **estimate**: the
    /// underlying progress is itself chunks-delivered × chunk size, and with
    /// several receivers at once their reports interleave. Right to within a chunk
    /// for the common case, and the only number that says what this share costs in
    /// bandwidth.
    pub(super) bytes_served: u64,
    /// Unix seconds of the last completed pickup; 0 = nobody has ever finished
    /// one. The line that actually answers "can I stop sharing this?".
    pub(super) last_pickup: u64,
    /// For a share that exists because a download finished (seed-after-complete):
    /// unix seconds of that download. 0 for a ticket or code the user asked for.
    ///
    /// Worth keeping precisely because the user did *not* create these rows: told
    /// "you downloaded this on the 9th, and are now making it available", a row
    /// that otherwise looks like a send of a file they never sent explains itself.
    pub(super) from_download: u64,
    /// Unix seconds when this share first began serving — the *original* moment,
    /// not the last resume.
    ///
    /// A resumed share is registered afresh and gets a new `created`, so an age
    /// measured from the transfer row restarts every time the daemon does, and a
    /// limit measured that way would never be reached on a machine that reboots.
    /// 0 in a sidecar written before this field: such a share is treated as
    /// beginning now, which errs towards keeping it rather than dropping it.
    pub(super) started: u64,
}

pub(super) fn share_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("share-{id}.pc"))
}

pub(super) fn persist_share(dir: &Path, id: u64, rec: &ShareRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&share_record_path(dir, id), &bytes);
    }
}

/// The counters for a share, or zeroes when there are none (a share older than
/// this record, or one that has yet to serve anybody).
pub(super) fn load_share(dir: &Path, id: u64) -> ShareRecord {
    std::fs::read(share_record_path(dir, id))
        .ok()
        .and_then(|b| postcard::from_bytes(&b).ok())
        .unwrap_or_default()
}

pub(super) fn remove_share(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(share_record_path(dir, id));
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
    /// The device this send is aimed at, carried across a restart.
    ///
    /// Without it a resumed send silently loses its aim and lands on whichever
    /// machine wakes first — and a send that had to be resumed is exactly the one
    /// whose target was not around to take it.
    pub(super) target: Option<crate::sync::DeviceId>,
    /// The recipient's relay, when they listen somewhere other than where we do.
    ///
    /// Without it a restored send goes back to routing through our own relay, and
    /// a cross-relay recipient becomes unreachable precisely on the retry path
    /// built to keep reaching them. `None` means "ours", as everywhere else.
    pub(super) relay: Option<String>,
}

/// [`SendToRecord`] as written before it carried a relay, and before that a
/// device target. Read-only, for the one-time migration in [`load_sendtos`]:
/// newest shape first, because postcard ignores trailing bytes and an older shape
/// would happily mis-read a newer record as "everything I know plus junk".
#[derive(serde::Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
struct SendToRecordV2 {
    id: u64,
    recipient: Vec<u8>,
    payload: String,
    name: String,
    archive: bool,
    paused: bool,
    reason: String,
    note: String,
    target: Option<crate::sync::DeviceId>,
}

#[derive(serde::Deserialize)]
struct SendToRecordV1 {
    id: u64,
    recipient: Vec<u8>,
    payload: String,
    name: String,
    archive: bool,
    paused: bool,
    reason: String,
    note: String,
}

pub(super) fn sendto_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("sendto2-{id}.pc"))
}

/// Where the pre-relay records live. Only read, never written.
fn legacy_sendto_glob() -> &'static str {
    "sendto-"
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

/// Every held `send --to` on disk, migrating any written under the old prefix.
///
/// A held send is the one piece of state that exists *because* delivery has not
/// happened yet, so dropping it on an upgrade is the worst time to drop it. The
/// old files are read in the shapes they could have been written in, rewritten
/// under the new prefix, and unlinked — so this costs one pass, once.
pub(super) fn load_sendtos(dir: &Path) -> Vec<SendToRecord> {
    let mut out: Vec<SendToRecord> = load_records(dir, "sendto2-");

    for path in record_paths(dir, legacy_sendto_glob()) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Some(rec) = decode_legacy_sendto(&bytes) else {
            continue;
        };
        // A restart between the write and the unlink would otherwise restore the
        // same send twice, under two ids.
        if !out.iter().any(|r| r.id == rec.id) {
            persist_sendto(dir, &rec);
            out.push(rec);
        }
        let _ = std::fs::remove_file(&path);
    }
    out
}

fn decode_legacy_sendto(bytes: &[u8]) -> Option<SendToRecord> {
    if let Ok(r) = postcard::from_bytes::<SendToRecordV2>(bytes) {
        return Some(SendToRecord {
            id: r.id,
            recipient: r.recipient,
            payload: r.payload,
            name: r.name,
            archive: r.archive,
            paused: r.paused,
            reason: r.reason,
            note: r.note,
            target: r.target,
            relay: None,
        });
    }
    let r: SendToRecordV1 = postcard::from_bytes(bytes).ok()?;
    Some(SendToRecord {
        id: r.id,
        recipient: r.recipient,
        payload: r.payload,
        name: r.name,
        archive: r.archive,
        paused: r.paused,
        reason: r.reason,
        note: r.note,
        target: None,
        relay: None,
    })
}

/// The preparation behind a held `send --to`: the content key, the transport seed,
/// and the digests taken under that key. **Beside** its [`SendToRecord`], never
/// inside it — same reason as [`ShareRecord`]: postcard is not self-describing, so a
/// new field would make every record an older build wrote undecodable, and
/// `load_records` skips silently what it cannot read.
///
/// It is what lets a restarted daemon resume *the same send*. Recomputing the
/// preparation mints a fresh key and seed, and the recipient is then looking at a
/// different file under a node id nobody is serving.
///
/// The pass that would notice a payload changed under us is exactly the one this
/// record exists to skip, so it also writes down what the payload looked like when
/// the digests were taken. See `work::PayloadStamp`.
///
/// Holds the content key in the clear: goes through [`write_record_private`], like
/// every other record that carries one.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct PrepRecord {
    pub(super) id: u64,
    pub(super) key: Vec<u8>,
    /// Spelled like [`SendRecord::node_seed`]: 32 bytes, the transport secret.
    pub(super) node_seed: Vec<u8>,
    pub(super) total_size: u64,
    /// One 32-byte digest per 16 MiB chunk — 22 KB for a 10 GB payload.
    pub(super) chunks: Vec<crate::hash::Hash>,
    /// The payload these digests were taken from, and what it looked like then.
    pub(super) payload: String,
    pub(super) len: u64,
    pub(super) mtime_secs: u64,
    pub(super) mtime_nanos: u32,
    /// Device + inode on unix (0 elsewhere): a replaced file is caught outright,
    /// which length and mtime alone can miss.
    pub(super) dev: u64,
    pub(super) ino: u64,
}

pub(super) fn prep_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("prep-{id}.pc"))
}

pub(super) fn persist_prep(dir: &Path, rec: &PrepRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&prep_record_path(dir, rec.id), &bytes);
    }
}

pub(super) fn load_prep(dir: &Path, id: u64) -> Option<PrepRecord> {
    let bytes = std::fs::read(prep_record_path(dir, id)).ok()?;
    postcard::from_bytes(&bytes).ok()
}

pub(super) fn remove_prep(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(prep_record_path(dir, id));
}

/// Every preparation on disk — for the sweep that drops the ones whose held send
/// did not come back. Each restore re-keys onto a fresh id, so a record left under
/// an old one would sit there for ever, holding a content key nobody can use.
pub(super) fn load_preps(dir: &Path) -> Vec<PrepRecord> {
    load_records(dir, "prep-")
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
    /// Where the blob is — the relay we uploaded to, which is the one named inside
    /// the `arvm…` the recipient fetches with. Revocation goes here.
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
    /// Where the *notice* is — the recipient's relay, which is not always the one
    /// holding the blob. Retracting the offer goes here, revoking the blob goes to
    /// `relay`; collapsing the two is what a cross-relay deposit breaks.
    pub(super) offer_relay: String,
}

/// [`DepositedRecord`] as written when one relay served both roles. Read-only,
/// for the migration in [`load_depositeds`].
#[derive(serde::Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
struct DepositedRecordV1 {
    id: u64,
    recipient: Vec<u8>,
    name: String,
    size: u64,
    relay: String,
    claim: String,
    expires: u64,
    revoke_token: String,
    offer_id: String,
    poster_token: String,
}

// NOTE on evolving these records: postcard is a compact, **non-self-describing**
// format — fields are read positionally and `#[serde(default)]` does nothing for
// a missing one. Adding or reordering a field therefore makes every previously
// written record of that type unparsable; `load_records` skips those silently, so
// the state they tracked is quietly forgotten. If one of these structs ever needs
// a new field after a release, bump the filename prefix (e.g. `dep2-`) and migrate
// the old files, rather than editing the struct in place.
//
// This is not theoretical, and `#[serde(default)]` is not an escape hatch: it does
// nothing here, because postcard's `SeqAccess` hands the missing field's decode
// straight to a buffer that has already ended, and the error propagates before
// serde ever reaches for a default. `load_sendtos` and `load_depositeds` show the
// shape of the migration — read the old prefix in every shape it could have been
// written in (newest first, since postcard ignores trailing bytes), rewrite under
// the new prefix, unlink. `postcard_ignores_serde_default` pins the behaviour.

pub(super) fn deposited_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("dep2-{id}.pc"))
}

/// Awaiting-pickup deposits, migrating any written before the offer's relay was
/// tracked apart from the blob's. Both roles were one relay then, so that is what
/// the migrated record says.
pub(super) fn load_depositeds(dir: &Path) -> Vec<DepositedRecord> {
    let mut out: Vec<DepositedRecord> = load_records(dir, "dep2-");

    for path in record_paths(dir, "dep-") {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(r) = postcard::from_bytes::<DepositedRecordV1>(&bytes) else {
            continue;
        };
        let rec = DepositedRecord {
            id: r.id,
            recipient: r.recipient,
            name: r.name,
            size: r.size,
            offer_relay: r.relay.clone(),
            relay: r.relay,
            claim: r.claim,
            expires: r.expires,
            revoke_token: r.revoke_token,
            offer_id: r.offer_id,
            poster_token: r.poster_token,
        };
        if !out.iter().any(|x| x.id == rec.id) {
            persist_deposited(dir, &rec);
            out.push(rec);
        }
        let _ = std::fs::remove_file(&path);
    }
    out
}

/// A send parked on the relay as **chunks** rather than as a mailbox blob.
///
/// The same three promises a deposit makes, which is the whole reason this is
/// written down: it can be withdrawn (`token`), asked after (`chunks`, against the
/// relay's `have`), and it has a deadline (`expires`). A parked send that nothing
/// recorded would be a file on somebody's relay that its sender could neither take
/// back nor learn the fate of.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct ParkedRecord {
    pub(super) id: u64,
    pub(super) recipient: Vec<u8>,
    pub(super) name: String,
    pub(super) size: u64,
    /// The relay holding the pieces — where a withdrawal and a "have you still
    /// got these?" both go.
    pub(super) relay: String,
    /// Per-transfer token: the capability that authorizes withdrawing them.
    pub(super) token: String,
    /// The digests the parked copy is addressed by. Also what the pickup poll asks
    /// about: a chunk the relay no longer holds is one the recipient released as
    /// they took it.
    pub(super) chunks: Vec<crate::hash::Hash>,
    /// Unix seconds when the relay drops them of its own accord.
    pub(super) expires: u64,
    /// The offer in the recipient's inbox that names these pieces, and the token
    /// that retracts it — so a cancel removes the notice as well as the file.
    pub(super) offer_id: String,
    pub(super) poster_token: String,
    /// Where the notice is, which is not always where the pieces are.
    pub(super) offer_relay: String,
}

pub(super) fn parked_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("park-{id}.pc"))
}

pub(super) fn load_parkeds(dir: &Path) -> Vec<ParkedRecord> {
    load_records(dir, "park-")
}

pub(super) fn persist_parked(dir: &Path, rec: &ParkedRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&parked_path(dir, rec.id), &bytes);
    }
}

pub(super) fn remove_parked(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(parked_path(dir, id));
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

/// Read every postcard record whose filename starts with `prefix` from `dir`.
pub(super) fn load_records<T: serde::de::DeserializeOwned>(dir: &Path, prefix: &str) -> Vec<T> {
    record_paths(dir, prefix)
        .into_iter()
        .filter_map(|p| std::fs::read(&p).ok())
        .filter_map(|b| postcard::from_bytes::<T>(&b).ok())
        .collect()
}

/// Every `<prefix>*.pc` in `dir`. A prefix ending in `-` never catches the next
/// generation of itself (`dep-` does not match `dep2-`), which is what makes a
/// prefix bump a clean cut rather than a double read.
pub(super) fn record_paths(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix) && n.ends_with(".pc"))
        })
        .collect()
}

/// On-disk record of a live short pairing code the daemon is hosting, so it comes
/// back after a restart. Kept in its own file, with its own prefix, rather than as
/// fields on [`SendRecord`] — see the note above on evolving these records: adding
/// a field to an existing struct silently orphans every record already written.
///
/// It holds two secrets — the code itself and the slot's owner token — plus a
/// ticket whose `Plain` key delivery carries the content key in the clear, so it
/// goes through [`write_record_private`] like the rest.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct CodeRecord {
    /// The transfer this code belongs to (the same id its [`SendRecord`] has).
    pub(super) id: u64,
    pub(super) slot: String,
    pub(super) secret: String,
    pub(super) relay: String,
    pub(super) owner_token: Vec<u8>,
    /// The exact bytes handed to a receiver that pairs. Stored rather than
    /// re-derived from the send, so hosting a code is a pure function of this
    /// record and needs no ordering with the send's own resume.
    pub(super) payload: Vec<u8>,
    /// The code as it was shown to the user (with `@relay` when embedded).
    pub(super) shown: String,
    /// `None` = serve until cancelled (`--keep`).
    pub(super) max_sessions: Option<u32>,
    pub(super) max_failures: u32,
    /// Receivers served and wrong-code attempts so far. Persisted on every change:
    /// a restart that reset the failure count would hand an attacker its guess
    /// budget back.
    pub(super) sessions_done: u32,
    pub(super) failures: u32,
}

pub(super) fn code_record_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("code-{id}.pc"))
}

pub(super) fn persist_code(dir: &Path, rec: &CodeRecord) {
    if let Ok(bytes) = postcard::to_allocvec(rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&code_record_path(dir, rec.id), &bytes);
    }
}

pub(super) fn remove_code(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(code_record_path(dir, id));
}

pub(super) fn load_codes(dir: &Path) -> Vec<CodeRecord> {
    load_records(dir, "code-")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule the NOTE above states, pinned so it stops being folklore: a struct
    /// that grew a field cannot read what the shape before it wrote, and
    /// `#[serde(default)]` does not change that. Every time this was forgotten, the
    /// cost was silent — `load_records` drops what it cannot parse.
    #[test]
    fn postcard_ignores_serde_default() {
        #[derive(serde::Serialize)]
        struct Before {
            a: u32,
        }
        #[derive(serde::Deserialize, Debug)]
        struct After {
            #[allow(dead_code)]
            a: u32,
            #[serde(default)]
            #[allow(dead_code)]
            b: String,
        }

        let bytes = postcard::to_allocvec(&Before { a: 7 }).unwrap();
        assert!(
            postcard::from_bytes::<After>(&bytes).is_err(),
            "if this ever passes, the prefix-bump migrations here are dead weight — \
             check postcard's SeqAccess and simplify"
        );
    }

    /// The other half of why migrations read newest-first: trailing bytes are not
    /// an error, so the *older* shape reads a newer record without complaint.
    #[test]
    fn postcard_accepts_trailing_bytes() {
        #[derive(serde::Serialize)]
        struct After {
            a: u32,
            b: u32,
        }
        #[derive(serde::Deserialize)]
        struct Before {
            a: u32,
        }

        let bytes = postcard::to_allocvec(&After { a: 7, b: 9 }).unwrap();
        let old: Before = postcard::from_bytes(&bytes).expect("trailing bytes are ignored");
        assert_eq!(old.a, 7);
    }

    /// A held send written by the previous build comes back, keeps its id, and is
    /// rewritten under the new prefix — the upgrade path that would otherwise
    /// forget exactly the sends that had not been delivered yet.
    #[test]
    fn legacy_sendto_records_migrate() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = SendToRecordV2 {
            id: 42,
            recipient: vec![1, 2, 3],
            payload: "/tmp/a".into(),
            name: "a".into(),
            archive: false,
            paused: true,
            reason: "waiting".into(),
            note: "hi".into(),
            target: None,
        };
        let bytes = postcard::to_allocvec(&legacy).unwrap();
        std::fs::write(dir.path().join("sendto-42.pc"), &bytes).unwrap();

        let loaded = load_sendtos(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, 42);
        assert_eq!(loaded[0].reason, "waiting");
        assert!(loaded[0].paused);
        assert_eq!(
            loaded[0].relay, None,
            "an old record predates per-contact relays"
        );

        assert!(dir.path().join("sendto2-42.pc").exists(), "rewritten");
        assert!(
            !dir.path().join("sendto-42.pc").exists(),
            "and the old one unlinked"
        );

        // Idempotent: a second pass has nothing left to migrate and must not
        // duplicate the row it already moved.
        assert_eq!(load_sendtos(dir.path()).len(), 1);
    }

    /// A deposit awaiting pickup keeps pointing at the relay holding the blob, and
    /// starts naming that same relay as where the notice sits — which is what it
    /// was, back when one relay did both jobs.
    #[test]
    fn legacy_deposited_records_migrate() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = DepositedRecordV1 {
            id: 7,
            recipient: vec![9],
            name: "f".into(),
            size: 10,
            relay: "https://old.example".into(),
            claim: "c".into(),
            expires: 99,
            revoke_token: "rv".into(),
            offer_id: "oid".into(),
            poster_token: "pt".into(),
        };
        std::fs::write(
            dir.path().join("dep-7.pc"),
            postcard::to_allocvec(&legacy).unwrap(),
        )
        .unwrap();

        let loaded = load_depositeds(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].relay, "https://old.example");
        assert_eq!(loaded[0].offer_relay, "https://old.example");
        assert!(dir.path().join("dep2-7.pc").exists());
        assert!(!dir.path().join("dep-7.pc").exists());
    }
}
