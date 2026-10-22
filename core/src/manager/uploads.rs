//! Uploads that survive a dropped connection, as engine transfers.
//!
//! Two kinds of transfer put a whole file on a relay: a browser **link**, and a
//! **mailbox deposit** (asked for explicitly, or the fallback of a held send).
//! Both used to be a single request that either finished or started over, and
//! the link was not even a transfer — it ran inside one IPC request, so a large
//! file outlived the client's patience and was reported as a stuck daemon while
//! it was still uploading.
//!
//! Here each upload keeps a [`Resumable`]: the seal it was started with and the
//! relay session it is filling (see [`crate::upload`]). The pair lives in memory
//! for as long as the transfer does and, when the transfer can come back after a
//! restart (a link, a held send), on disk beside the transfer's own record. A
//! retry — after a network change, a pause, a restart — picks up the same seal
//! and the same session, and the relay is asked only for what it is missing.

use std::time::Duration;

use super::*;
use crate::flow::MailboxSeal;
use crate::link::LinkSeal;
use crate::source::SendSource;
use crate::upload::{UploadError, UploadHooks, UploadSession};

/// What an upload was sealed with. Fixed when it starts; see the seal types.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum Sealing {
    Link(LinkSeal),
    Mailbox(MailboxSeal),
}

/// An upload's progress, as far as resuming it is concerned.
#[derive(Clone, Debug)]
pub(super) struct Resumable {
    pub(super) seal: Sealing,
    pub(super) session: Option<UploadSession>,
}

/// The on-disk mirror of a [`Resumable`], beside the record of the transfer it
/// belongs to (a [`LinkRecord`], or a held send's `SendToRecord`).
///
/// Beside it rather than inside it for the reason every sidecar here exists:
/// postcard is not self-describing, so a new field on an existing record would
/// make every record an older build wrote undecodable.
///
/// Carries what the payload looked like when the upload began. A session is
/// only worth resuming into the *same* bytes; a file that changed since would
/// be stitched together from two versions under one key, and every check on the
/// receiving side would pass chunk by chunk. A stamp that no longer matches
/// throws the record away and the upload starts over.
///
/// Holds a content key in the clear: written through `write_record_private`.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct UploadRecord {
    pub(super) id: u64,
    pub(super) seal: Sealing,
    pub(super) session: Option<UploadSession>,
    pub(super) payload: String,
    pub(super) len: u64,
    pub(super) mtime_secs: u64,
    pub(super) mtime_nanos: u32,
    pub(super) dev: u64,
    pub(super) ino: u64,
}

pub(super) fn upload_record_path(dir: &std::path::Path, id: u64) -> PathBuf {
    dir.join(format!("upl-{id}.pc"))
}

pub(super) fn remove_upload_record(dir: &std::path::Path, id: u64) {
    let _ = std::fs::remove_file(upload_record_path(dir, id));
}

fn load_upload_records(dir: &std::path::Path) -> Vec<UploadRecord> {
    load_records(dir, "upl-")
}

/// A link upload still in flight, so a restart carries it on. Its seal and
/// session are in the [`UploadRecord`] under the same id.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct LinkRecord {
    pub(super) id: u64,
    pub(super) payload: String,
    pub(super) relay: String,
    pub(super) ttl: u64,
    pub(super) max: u32,
}

fn link_record_path(dir: &std::path::Path, id: u64) -> PathBuf {
    dir.join(format!("link-{id}.pc"))
}

fn persist_link(inner: &Inner, rec: &LinkRecord) {
    if let (Some(dir), Ok(bytes)) = (&inner.state_dir, postcard::to_allocvec(rec)) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&link_record_path(dir, rec.id), &bytes);
    }
}

pub(super) fn remove_link_record(dir: &std::path::Path, id: u64) {
    let _ = std::fs::remove_file(link_record_path(dir, id));
}

fn remove_link(inner: &Inner, id: u64) {
    if let Some(dir) = &inner.state_dir {
        let _ = std::fs::remove_file(link_record_path(dir, id));
    }
}

pub(super) fn load_link_records(dir: &std::path::Path) -> Vec<LinkRecord> {
    load_records(dir, "link-")
}

/// Whether this transfer comes back after a restart, and so whether its upload
/// state is worth writing down. A deposit asked for explicitly does not — it may
/// carry a password, which is never written to disk — and an upload record with
/// nothing to come back to would only be swept up at the next start.
fn restorable(inner: &Inner, id: u64) -> bool {
    inner.held.lock().unwrap().contains_key(&id)
        || inner
            .state_dir
            .as_ref()
            .is_some_and(|dir| link_record_path(dir, id).exists())
}

/// Write the upload state for `id` to disk, when it is worth it and the payload
/// is a file that can be stamped. A handed-off descriptor dies with the process
/// that holds it, so there is nothing to resume from after a restart anyway.
fn persist_upload(inner: &Inner, id: u64, r: &Resumable, source: &SendSource) {
    let Some(dir) = &inner.state_dir else { return };
    let SendSource::Path(path) = source else {
        return;
    };
    if !restorable(inner, id) {
        return;
    }
    let Some(stamp) = payload_stamp(path) else {
        return;
    };
    let rec = UploadRecord {
        id,
        seal: r.seal.clone(),
        session: r.session.clone(),
        payload: path.display().to_string(),
        len: stamp.len,
        mtime_secs: stamp.mtime_secs,
        mtime_nanos: stamp.mtime_nanos,
        dev: stamp.dev,
        ino: stamp.ino,
    };
    if let Ok(bytes) = postcard::to_allocvec(&rec) {
        let _ = std::fs::create_dir_all(dir);
        let _ = write_record_private(&upload_record_path(dir, id), &bytes);
    }
}

/// The upload state a restored transfer left on disk under `old_id`, if the
/// payload is still the file it was taken from. Removes the record either way;
/// the caller re-files it under the transfer's new id with [`remember`].
pub(super) fn take_upload_record(inner: &Inner, old_id: u64, payload: &str) -> Option<Resumable> {
    let dir = inner.state_dir.as_ref()?;
    let rec: Option<UploadRecord> = std::fs::read(upload_record_path(dir, old_id))
        .ok()
        .and_then(|b| postcard::from_bytes(&b).ok());
    remove_upload_record(dir, old_id);
    let rec = rec?;
    let now = payload_stamp(std::path::Path::new(payload));
    let then = PayloadStamp {
        len: rec.len,
        mtime_secs: rec.mtime_secs,
        mtime_nanos: rec.mtime_nanos,
        dev: rec.dev,
        ino: rec.ino,
    };
    if rec.payload != payload || now != Some(then) {
        // The file moved or changed: what is on the relay is half of another
        // file. Let the relay have its space back.
        if let Some(s) = rec.session {
            tokio::spawn(async move { crate::upload::abandon(&s).await });
        }
        return None;
    }
    Some(Resumable {
        seal: rec.seal,
        session: rec.session,
    })
}

/// Upload records whose transfer did not come back, at startup: the session is
/// abandoned on its relay and the record (which holds a content key) deleted.
pub(super) fn sweep_upload_records(dir: &std::path::Path) {
    for rec in load_upload_records(dir) {
        remove_upload_record(dir, rec.id);
        if let Some(s) = rec.session {
            tokio::spawn(async move { crate::upload::abandon(&s).await });
        }
    }
}

/// Keep `r` as the upload state of transfer `id`.
pub(super) fn remember(inner: &Inner, id: u64, r: Resumable, source: &SendSource) {
    persist_upload(inner, id, &r, source);
    inner.uploads.lock().unwrap().insert(id, r);
}

/// The upload state of transfer `id`, if it has one.
pub(super) fn current(inner: &Inner, id: u64) -> Option<Resumable> {
    inner.uploads.lock().unwrap().get(&id).cloned()
}

/// Drop the upload state of transfer `id`. With `abandon`, the relay session it
/// was filling is abandoned too — for an upload that will not be finished
/// (cancelled, failed), as opposed to one that just was.
pub(super) fn forget(inner: &Inner, id: u64, abandon: bool) {
    let r = inner.uploads.lock().unwrap().remove(&id);
    if let Some(dir) = &inner.state_dir {
        remove_upload_record(dir, id);
    }
    if abandon {
        if let Some(s) = r.and_then(|r| r.session) {
            tokio::spawn(async move { crate::upload::abandon(&s).await });
        }
    }
}

/// Record a new session (or its end) for transfer `id`, in memory and on disk.
fn session_changed(inner: &Inner, id: u64, s: Option<&UploadSession>, source: &SendSource) {
    let updated = {
        let mut map = inner.uploads.lock().unwrap();
        let Some(r) = map.get_mut(&id) else { return };
        r.session = s.cloned();
        r.clone()
    };
    persist_upload(inner, id, &updated, source);
}

/// Report upload progress on the row, as plaintext bytes — the number the
/// person knows their file by. At most a couple of times a second: the
/// uploader calls this on every tick it sees bytes move.
fn report_progress(inner: &Inner, id: u64, cipher: u64, cipher_len: u64, plain_len: u64) {
    let plain = if cipher_len == 0 {
        0
    } else {
        ((cipher.min(cipher_len) as u128 * plain_len as u128) / cipher_len as u128) as u64
    };
    inner.set_progress(id, plain);
    inner.emit(ManagerEvent::Progress {
        id,
        transferred: plain,
        total_size: plain_len,
    });
}

/// The seal a mailbox deposit for transfer `id` goes out under: the one it
/// already has, when the payload is still the size it was sealed at, or a fresh
/// one (kept for next time).
pub(super) fn mailbox_seal(
    inner: &Inner,
    id: u64,
    source: &SendSource,
    recipient: &PublicId,
    password: Option<&str>,
) -> Result<(MailboxSeal, Option<UploadSession>)> {
    let size = source.len()?;
    if let Some(Resumable {
        seal: Sealing::Mailbox(seal),
        session,
    }) = current(inner, id)
    {
        if seal.size == size {
            return Ok((seal, session));
        }
        forget(inner, id, true);
    }
    let seal = MailboxSeal::new(source, recipient, &inner.me, password)?;
    remember(
        inner,
        id,
        Resumable {
            seal: Sealing::Mailbox(seal.clone()),
            session: None,
        },
        source,
    );
    Ok((seal, None))
}

/// What [`TransferHooks`] calls when the relay session changes.
type SessionHook = Box<dyn Fn(Option<&UploadSession>) + Send + Sync>;

/// Upload hooks that keep transfer `id`'s session and progress. Owns the
/// callbacks; [`Self::hooks`] lends them to an upload.
pub(super) struct TransferHooks {
    session: SessionHook,
    progress: Box<dyn Fn(u64) + Send + Sync>,
}

impl TransferHooks {
    pub(super) fn new(
        inner: &Arc<Inner>,
        id: u64,
        source: &SendSource,
        cipher_len: u64,
        plain_len: u64,
    ) -> Self {
        let session = {
            let inner = inner.clone();
            let source = source.clone();
            move |s: Option<&UploadSession>| session_changed(&inner, id, s, &source)
        };
        let progress = {
            let inner = inner.clone();
            move |c: u64| report_progress(&inner, id, c, cipher_len, plain_len)
        };
        Self {
            session: Box::new(session),
            progress: Box::new(progress),
        }
    }

    pub(super) fn hooks(&self) -> UploadHooks<'_> {
        UploadHooks {
            session: &*self.session,
            progress: &*self.progress,
        }
    }
}

/// How long to wait before trying an interrupted upload again: from a few
/// seconds, doubling, to five minutes. [`crate::upload::upload`] already
/// retried for a minute or two before giving up, so this is the slow rhythm
/// for an outage, not the fast one for a blip.
fn next_pause(p: Duration) -> Duration {
    (p * 2).min(Duration::from_secs(300))
}

const FIRST_PAUSE: Duration = Duration::from_secs(10);

/// Why the transfer is waiting, in words, for an error worth waiting out.
/// `None` for one that is final.
fn worth_waiting(e: &UploadError) -> Option<String> {
    match e {
        UploadError::Unavailable(m) => Some(m.clone()),
        UploadError::Busy => Some(e.to_string()),
        _ => None,
    }
}

impl TransferManager {
    /// Start a browser download link for `payload` and return its transfer id.
    ///
    /// Returns at once: the upload is a transfer like any other, with progress,
    /// a cancel, and — when the connection drops — a wait that carries on from
    /// where it stopped rather than from the start. A restart of the daemon
    /// carries it on too, when the payload is a file it can open again. The URL
    /// arrives with [`ManagerEvent::LinkReady`] and stays on the row.
    pub fn create_link(&self, payload: impl Into<SendSource>, ttl: u64, max: u32) -> Result<u64> {
        let source = payload.into();
        let relay = self
            .inner
            .relay
            .clone()
            .context("no relay configured — links need a relay")?;
        let seal = LinkSeal::new(&source)?;
        let (id, cancel) = self.register(Direction::Send, None, seal.name.clone(), seal.size);
        persist_link(
            &self.inner,
            &LinkRecord {
                id,
                payload: match &source {
                    SendSource::Path(p) => p.display().to_string(),
                    other => other.label(),
                },
                relay: relay.clone(),
                ttl,
                max,
            },
        );
        remember(
            &self.inner,
            id,
            Resumable {
                seal: Sealing::Link(seal),
                session: None,
            },
            &source,
        );
        tokio::spawn(run_link(
            self.inner.clone(),
            id,
            cancel,
            source,
            relay,
            ttl,
            max,
        ));
        Ok(id)
    }

    /// Bring back a link upload a previous run left unfinished, under a fresh id.
    pub(super) fn restore_link(&self, rec: LinkRecord) {
        let source = SendSource::Path(PathBuf::from(&rec.payload));
        let resumed = take_upload_record(&self.inner, rec.id, &rec.payload);
        let seal = match resumed.as_ref().map(|r| &r.seal) {
            Some(Sealing::Link(seal)) => seal.clone(),
            // Nothing to resume (the file changed, or it was never written down):
            // start the same link over, if the file is still there to start from.
            _ => match LinkSeal::new(&source) {
                Ok(seal) => seal,
                Err(e) => {
                    let name = std::path::Path::new(&rec.payload)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| rec.payload.clone());
                    let (id, _cancel) = self.register(Direction::Send, None, name, 0);
                    finish(&self.inner, id, false, Err(e));
                    return;
                }
            },
        };
        let session = resumed.and_then(|r| r.session);
        let (id, cancel) = self.register(Direction::Send, None, seal.name.clone(), seal.size);
        persist_link(
            &self.inner,
            &LinkRecord {
                id,
                payload: rec.payload,
                relay: rec.relay.clone(),
                ttl: rec.ttl,
                max: rec.max,
            },
        );
        remember(
            &self.inner,
            id,
            Resumable {
                seal: Sealing::Link(seal),
                session,
            },
            &source,
        );
        tokio::spawn(run_link(
            self.inner.clone(),
            id,
            cancel,
            source,
            rec.relay,
            rec.ttl,
            rec.max,
        ));
    }
}

/// Drive a link upload until it is on the relay, cancelled, or impossible.
async fn run_link(
    inner: Arc<Inner>,
    id: u64,
    cancel: CancellationToken,
    source: SendSource,
    relay: String,
    ttl: u64,
    max: u32,
) {
    let mut pause = FIRST_PAUSE;
    loop {
        let Some(Resumable {
            seal: Sealing::Link(seal),
            session,
        }) = current(&inner, id)
        else {
            remove_link(&inner, id);
            finish(
                &inner,
                id,
                false,
                Err(anyhow::anyhow!("the link's upload state was lost")),
            );
            return;
        };
        let cipher_len = crate::link::link_layout(source.clone(), &seal)
            .map(|l| l.len())
            .unwrap_or(0);
        let owned_hooks = TransferHooks::new(&inner, id, &source, cipher_len, seal.size);
        let hooks = owned_hooks.hooks();
        let attempt = crate::link::deposit_link_with(
            source.clone(),
            &seal,
            &relay,
            ttl,
            max,
            session,
            &hooks,
        );
        let outcome = tokio::select! {
            _ = cancel.cancelled() => {
                forget(&inner, id, true);
                remove_link(&inner, id);
                finish(&inner, id, true, Ok(None));
                return;
            }
            r = attempt => r,
        };
        let e = match outcome {
            Ok(out) => {
                forget(&inner, id, false);
                remove_link(&inner, id);
                inner.set_progress(id, out.size);
                inner.set_link(id, out.link.clone());
                inner.emit(ManagerEvent::LinkReady {
                    id,
                    info: DepositInfo {
                        relay: relay.trim_end_matches('/').to_string(),
                        claim: out.claim,
                        revoke_token: out.revoke_token,
                        name: out.name,
                        size: out.size,
                        expires: unix_now().saturating_add(out.ttl_secs),
                        max,
                        recipient: None,
                        offer_id: String::new(),
                        poster_token: String::new(),
                        ticket: String::new(),
                        link: Some(out.link),
                    },
                });
                finish(&inner, id, false, Ok(None));
                return;
            }
            Err(e) => e,
        };
        let Some(why) = worth_waiting(&e) else {
            forget(&inner, id, true);
            remove_link(&inner, id);
            let err = match e {
                UploadError::Fatal(err) => err,
                other => anyhow::anyhow!("{other}"),
            };
            finish(&inner, id, false, Err(err));
            return;
        };
        set_waiting(&inner, id, WaitingReason::UploadRetry(why));
        tokio::select! {
            _ = cancel.cancelled() => {
                forget(&inner, id, true);
                remove_link(&inner, id);
                finish(&inner, id, true, Ok(None));
                return;
            }
            _ = tokio::time::sleep(pause) => {}
        }
        pause = next_pause(pause);
        inner.set_status_live(id, TransferStatus::Active);
    }
}

/// The retry loop of an explicit mailbox deposit, run detached: what
/// [`TransferManager::deposit_to`] used to do inside the caller's request, plus
/// waiting out a dropped connection instead of failing on it.
///
/// Never restored after a restart — see [`restorable`] — so the password, when
/// there is one, lives only in the seal in memory.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_deposit(
    inner: Arc<Inner>,
    id: u64,
    cancel: CancellationToken,
    deposit_relay: String,
    offer_relay: String,
    recipient: PublicId,
    payload: SendSource,
    name: String,
    note: String,
    opts: MailboxOpts,
) -> std::result::Result<String, String> {
    let mut pause = FIRST_PAUSE;
    loop {
        let attempt = deposit_offline_and_offer(
            &inner,
            id,
            &deposit_relay,
            &offer_relay,
            &recipient,
            &payload,
            &name,
            &note,
            &opts,
        );
        let outcome = tokio::select! {
            _ = cancel.cancelled() => {
                forget(&inner, id, true);
                finish(&inner, id, true, Ok(None));
                return Err("deposit cancelled".into());
            }
            r = attempt => r,
        };
        let e = match outcome {
            Ok(out) => {
                let ticket = out.ticket.clone();
                // The relay's granted TTL, not the one `opts` asked for: it clamps
                // to its own maximum, and a record deadlined off the request would
                // keep watching (and keep telling the user about) a blob already
                // reaped.
                let expires = unix_now().saturating_add(out.ttl_secs);
                spawn_offline_confirm(&inner, id, deposit_relay, offer_relay, out, expires);
                return Ok(ticket);
            }
            Err(e) => e,
        };
        let why = match &e {
            flow::DepositError::Unavailable(m) => m.clone(),
            flow::DepositError::Busy => e.to_string(),
            _ => {
                forget(&inner, id, true);
                let msg = e.to_string();
                finish(&inner, id, false, Err(anyhow::anyhow!("{msg}")));
                return Err(msg);
            }
        };
        set_waiting(&inner, id, WaitingReason::UploadRetry(why));
        tokio::select! {
            _ = cancel.cancelled() => {
                forget(&inner, id, true);
                finish(&inner, id, true, Ok(None));
                return Err("deposit cancelled".into());
            }
            _ = tokio::time::sleep(pause) => {}
        }
        pause = next_pause(pause);
        inner.set_status_live(id, TransferStatus::Active);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A relay nothing listens on: every upload waits, which is the state a
    /// restart finds a link in.
    const DEAD: &str = "http://127.0.0.1:1";

    fn manager(state: &std::path::Path, dl: &std::path::Path) -> TransferManager {
        TransferManager::with_state_dir(
            Identity::generate(),
            Some(DEAD.into()),
            dl.to_path_buf(),
            Some(state.to_path_buf()),
        )
    }

    fn session() -> UploadSession {
        UploadSession {
            relay: DEAD.into(),
            id: "sess-1".into(),
            length: 4242,
        }
    }

    fn link_seal(m: &TransferManager, id: u64) -> LinkSeal {
        match current(&m.inner, id).map(|r| r.seal) {
            Some(Sealing::Link(s)) => s,
            other => panic!("no link seal: {other:?}"),
        }
    }

    /// The link comes back under a new id with the same seal and the same relay
    /// session, so the upload carries on into the bytes already there.
    #[tokio::test]
    async fn a_restart_brings_a_link_back_with_its_seal_and_session() {
        let state = tempfile::tempdir().unwrap();
        let dl = tempfile::tempdir().unwrap();
        let f = state.path().join("payload.bin");
        std::fs::write(&f, vec![5u8; 3000]).unwrap();

        let m1 = manager(state.path(), dl.path());
        let id = m1.create_link(f.as_path(), 3600, 0).unwrap();
        session_changed(
            &m1.inner,
            id,
            Some(&session()),
            &SendSource::from(f.as_path()),
        );
        let seal = link_seal(&m1, id);

        let m2 = manager(state.path(), dl.path());
        assert!(m2.resume_incomplete() >= 1);
        let row = m2
            .list()
            .into_iter()
            .find(|t| t.name == "payload.bin")
            .expect("the link is back");
        let back = current(&m2.inner, row.id).expect("with its upload state");
        assert_eq!(back.seal, Sealing::Link(seal));
        assert_eq!(back.session, Some(session()));
    }

    /// A file that changed since is not resumed into: the relay has half of
    /// another file. The link starts over with a fresh seal.
    #[tokio::test]
    async fn a_changed_file_starts_its_link_over() {
        let state = tempfile::tempdir().unwrap();
        let dl = tempfile::tempdir().unwrap();
        let f = state.path().join("payload.bin");
        std::fs::write(&f, vec![5u8; 3000]).unwrap();

        let m1 = manager(state.path(), dl.path());
        let id = m1.create_link(f.as_path(), 3600, 0).unwrap();
        session_changed(
            &m1.inner,
            id,
            Some(&session()),
            &SendSource::from(f.as_path()),
        );
        let seal = link_seal(&m1, id);

        std::fs::write(&f, vec![6u8; 3001]).unwrap();
        let m2 = manager(state.path(), dl.path());
        m2.resume_incomplete();
        let row = m2
            .list()
            .into_iter()
            .find(|t| t.name == "payload.bin")
            .expect("the link is back");
        let back = current(&m2.inner, row.id).expect("with upload state");
        assert_ne!(back.seal, Sealing::Link(seal), "a fresh key for a new file");
        assert_eq!(back.session, None);
    }

    /// Upload state nothing came back for is swept: it holds a content key.
    #[tokio::test]
    async fn upload_state_without_its_transfer_is_swept() {
        let state = tempfile::tempdir().unwrap();
        let dl = tempfile::tempdir().unwrap();
        let rec = UploadRecord {
            id: 99,
            seal: Sealing::Link(LinkSeal {
                key: [1u8; 32],
                revoke_token: "t".into(),
                name: "x".into(),
                size: 1,
            }),
            session: None,
            payload: "/nowhere".into(),
            len: 1,
            mtime_secs: 0,
            mtime_nanos: 0,
            dev: 0,
            ino: 0,
        };
        write_record_private(
            &upload_record_path(state.path(), 99),
            &postcard::to_allocvec(&rec).unwrap(),
        )
        .unwrap();
        let m = manager(state.path(), dl.path());
        m.resume_incomplete();
        assert!(!upload_record_path(state.path(), 99).exists());
    }

    /// Cancelling a link forgets its upload state, in memory and on disk.
    #[tokio::test]
    async fn a_cancelled_link_leaves_nothing_behind() {
        let state = tempfile::tempdir().unwrap();
        let dl = tempfile::tempdir().unwrap();
        let f = state.path().join("payload.bin");
        std::fs::write(&f, vec![5u8; 3000]).unwrap();
        let m = manager(state.path(), dl.path());
        let id = m.create_link(f.as_path(), 3600, 0).unwrap();
        assert!(m.cancel(id));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while m.get(id).map(|t| t.status) != Some(TransferStatus::Cancelled) {
            assert!(std::time::Instant::now() < deadline, "never cancelled");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(current(&m.inner, id).is_none());
        assert!(!upload_record_path(state.path(), id).exists());
        assert!(load_link_records(state.path()).is_empty());
    }
}
