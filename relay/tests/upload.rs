//! Resumable upload sessions (`/v1/upload`), from the wire up.
//!
//! The relay half is driven in-process through the router; the client half
//! (`arvolo_core::upload`, and the link and mailbox deposits built on it) runs
//! against a real listening relay, including one with sessions turned off — the
//! shape of every relay deployed before them.

use std::sync::{Arc, Mutex};

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::flow::{deposit_offline_with, fetch_offline, MailboxSeal};
use arvolo_core::link::{decode_key, decrypt_link, deposit_link_with, link_layout, LinkSeal};
use arvolo_core::manager::{ManagerEvent, TransferManager, TransferStatus};
use arvolo_core::source::SendSource;
use arvolo_core::transfer::RelayChoice;
use arvolo_core::upload::{UploadHooks, UploadSession};
use arvolo_relay::{router, AppState, Mailbox, UPLOAD_IDLE_SECS};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

const OFFSET: &str = "x-arvolo-upload-offset";
const LENGTH: &str = "x-arvolo-upload-length";

async fn state(dir: &std::path::Path) -> (AppState, Arc<Mailbox>) {
    let node = BlobNode::spawn(dir, RelayChoice::Disabled)
        .await
        .expect("blob node");
    let mb = Arc::new(Mailbox::in_memory().expect("mailbox"));
    (AppState::new(mb.clone(), Arc::new(node)), mb)
}

async fn body(resp: axum::response::Response) -> Vec<u8> {
    resp.into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

fn header(resp: &axum::response::Response, name: &str) -> Option<u64> {
    resp.headers().get(name)?.to_str().ok()?.parse().ok()
}

async fn open(app: &axum::Router, length: u64) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post("/v1/upload?ttl=3600&max=1")
                .header("x-arvolo-encapped-key", "AAAAAAAA")
                .header(LENGTH, length.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn append(app: &axum::Router, id: &str, at: u64, bytes: Vec<u8>) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::patch(format!("/v1/upload/{id}"))
                .header(OFFSET, at.to_string())
                .body(Body::from(bytes))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn get(app: &axum::Router, uri: String) -> axum::response::Response {
    app.clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn commit(app: &axum::Router, id: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post(format!("/v1/upload/{id}/commit"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// The whole life of a session: open, two appends, a wrong offset answered
/// with the right one, commit — and what comes out of the fetch is what went in.
#[tokio::test]
async fn a_session_takes_the_file_in_pieces_and_becomes_an_ordinary_entry() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _mb) = state(dir.path()).await;
    let app = router(st);
    let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();

    let resp = open(&app, data.len() as u64).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let id = String::from_utf8(body(resp).await).unwrap();

    let resp = get(&app, format!("/v1/upload/{id}")).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&resp, OFFSET), Some(0));
    assert_eq!(header(&resp, LENGTH), Some(data.len() as u64));

    let resp = append(&app, &id, 0, data[..4000].to_vec()).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&resp, OFFSET), Some(4000));

    // A client that lost track — its last answer never arrived — is told where
    // the upload really stands, in the same round trip.
    let resp = append(&app, &id, 1000, data[1000..].to_vec()).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(header(&resp, OFFSET), Some(4000));

    // Not complete: committing says where it stands rather than inventing a claim.
    let resp = commit(&app, &id).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(header(&resp, OFFSET), Some(4000));

    let resp = append(&app, &id, 4000, data[4000..].to_vec()).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&resp, OFFSET), Some(data.len() as u64));

    let resp = commit(&app, &id).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header(&resp, "x-arvolo-ttl"), Some(3600));
    let claim = String::from_utf8(body(resp).await).unwrap();

    // A commit whose answer was lost is asked again: the same claim, not a copy.
    let resp = commit(&app, &id).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(String::from_utf8(body(resp).await).unwrap(), claim);

    let resp = get(&app, format!("/v1/fetch/{claim}")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body(resp).await, data);
}

/// Bytes past the declared length are refused, and what was declared stays.
#[tokio::test]
async fn an_append_cannot_run_past_the_declared_length() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _mb) = state(dir.path()).await;
    let app = router(st);
    let id = String::from_utf8(body(open(&app, 100).await).await).unwrap();

    let resp = append(&app, &id, 0, vec![1u8; 150]).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = get(&app, format!("/v1/upload/{id}")).await;
    assert!(
        header(&resp, OFFSET).unwrap() <= 100,
        "the file is never longer than what was declared"
    );
}

/// The per-file cap is met when the session opens — before a byte is sent — and
/// names itself, as the single-request deposit's refusal does.
#[tokio::test]
async fn a_file_over_the_cap_is_refused_at_the_door() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _mb) = state(dir.path()).await;
    let app = router(st);
    let resp = open(&app, u64::MAX / 2).await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(resp.headers().contains_key("x-arvolo-max-blob-bytes"));
}

/// An open session reserves its whole length against the disk budget, and an
/// idle one gives it back to the reaper together with its partial file.
#[tokio::test]
async fn an_idle_session_is_reaped_with_its_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let (st, mb) = state(dir.path()).await;
    let app = router(st);
    let before = mb.disk_bytes();
    let id = String::from_utf8(body(open(&app, 5000).await).await).unwrap();
    assert_eq!(mb.disk_bytes(), before + 5000, "reserved in full, at once");
    append(&app, &id, 0, vec![7u8; 100]).await;
    assert!(mb.upload_path(&id).exists());

    let later = arvolo_relay::now_unix() + UPLOAD_IDLE_SECS + 1;
    assert_eq!(mb.upload_reap(later), 1);
    assert_eq!(mb.disk_bytes(), before);
    assert!(!mb.upload_path(&id).exists());
    let resp = get(&app, format!("/v1/upload/{id}")).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// The disk sweep leaves a live session's partial file alone, and takes one
/// whose session is gone.
#[tokio::test]
async fn the_sweep_keeps_live_partials_and_takes_orphans() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = dir.path().join("blobs");
    let mb = Mailbox::open(dir.path().join("db.sqlite"), &blobs).unwrap();
    let id = mb
        .open_upload(
            arvolo_relay::UploadOpen {
                encapped_key: Vec::new(),
                revoke_hash: Vec::new(),
                ttl_secs: 60,
                max_downloads: 1,
                length: 10,
            },
            arvolo_relay::now_unix(),
        )
        .unwrap();
    std::fs::write(blobs.join("orphan.part"), b"x").unwrap();
    assert_eq!(mb.sweep_orphan_blobs(0), 1);
    assert!(mb.upload_path(&id).exists());
    assert!(!blobs.join("orphan.part").exists());
}

/// Turned off by the operator, the routes are unknown and the feature is not
/// advertised — so a client takes the single-request path.
#[tokio::test]
async fn a_relay_can_turn_sessions_off() {
    let dir = tempfile::tempdir().unwrap();
    let (mut st, _mb) = state(dir.path()).await;
    st.uploads_enabled = false;
    let app = router(st);
    let resp = get(&app, "/v1/features".into()).await;
    let doc = String::from_utf8(body(resp).await).unwrap();
    assert!(doc.contains("\"upload\":false"), "{doc}");
    assert_eq!(open(&app, 10).await.status(), StatusCode::NOT_FOUND);
}

async fn spawn_relay(uploads: bool) -> (String, Arc<Mailbox>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let (mut st, mb) = state(dir.path()).await;
    st.uploads_enabled = uploads;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(st)).await.unwrap();
    });
    (format!("http://{addr}"), mb, dir)
}

fn payload(len: usize) -> (tempfile::NamedTempFile, Vec<u8>) {
    let data: Vec<u8> = (0..len).map(|i| (i * 7 % 253) as u8).collect();
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), &data).unwrap();
    (f, data)
}

async fn fetch_link(link: &str) -> (String, Vec<u8>) {
    let (base, key) = link.split_once('#').unwrap();
    let claim = base.rsplit('/').next().unwrap();
    let relay = base.split("/dl/").next().unwrap();
    let blob = reqwest::get(format!("{relay}/v1/fetch/{claim}"))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    decrypt_link(&blob, &decode_key(key).unwrap()).unwrap()
}

/// The point of all of it: a link whose upload broke off half-way is finished
/// from where the relay stands, not from the first byte — and the file that
/// comes out is the file that went in.
#[tokio::test]
async fn an_interrupted_link_upload_resumes_from_where_it_stopped() {
    let (relay, _mb, _dir) = spawn_relay(true).await;
    let (f, data) = payload(3 * 1024 * 1024 + 4321);
    let source = SendSource::from(f.path());
    let seal = LinkSeal::new(&source).unwrap();
    let blob = link_layout(source.clone(), &seal)
        .unwrap()
        .to_vec()
        .unwrap();

    // What a first attempt left behind: a session, and the first part of it.
    let c = reqwest::Client::new();
    let id = c
        .post(format!("{relay}/v1/upload?ttl=3600&max=0"))
        .header("x-arvolo-encapped-key", "")
        .header(
            "x-arvolo-revoke-hash",
            data_encoding::BASE32_NOPAD
                .encode(blake3::hash(seal.revoke_token.as_bytes()).as_bytes()),
        )
        .header(LENGTH, blob.len().to_string())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let cut = 1_500_000usize;
    let resp = c
        .patch(format!("{relay}/v1/upload/{id}"))
        .header(OFFSET, "0")
        .body(blob[..cut].to_vec())
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let first_progress = Arc::new(Mutex::new(None::<u64>));
    let seen = first_progress.clone();
    let progress = move |n: u64| {
        let mut s = seen.lock().unwrap();
        if s.is_none() {
            *s = Some(n);
        }
    };
    let hooks = UploadHooks {
        session: &|_| {},
        progress: &progress,
    };
    let session = UploadSession {
        relay: relay.clone(),
        id,
        length: blob.len() as u64,
    };
    let out = deposit_link_with(source, &seal, &relay, 3600, 0, Some(session), &hooks)
        .await
        .expect("the resumed upload lands");
    assert_eq!(
        *first_progress.lock().unwrap(),
        Some(cut as u64),
        "it starts from what the relay already had"
    );
    let (name, got) = fetch_link(&out.link).await;
    assert_eq!(name, f.path().file_name().unwrap().to_str().unwrap());
    assert_eq!(got, data);
}

/// A relay from before sessions gets the single request it always got.
#[tokio::test]
async fn a_relay_without_sessions_still_takes_a_link() {
    let (relay, _mb, _dir) = spawn_relay(false).await;
    let (f, data) = payload(2 * 1024 * 1024 + 17);
    let out = arvolo_core::link::deposit_link(f.path(), &relay, 3600, 0)
        .await
        .expect("deposited");
    let (_, got) = fetch_link(&out.link).await;
    assert_eq!(got, data);
}

/// The mailbox deposit resumes the same way, and the recipient's fetch opens it.
#[tokio::test]
async fn an_interrupted_mailbox_deposit_resumes_and_opens() {
    let (relay, _mb, _dir) = spawn_relay(true).await;
    let me = Identity::generate();
    let them = Identity::generate();
    let (f, data) = payload(17 * 1024 * 1024 + 99);
    let source = SendSource::from(f.path());
    let seal = MailboxSeal::new(&source, &them.public(), &me, None).unwrap();

    // The first attempt: everything recorded, nothing finished.
    let opened = Arc::new(Mutex::new(None::<UploadSession>));
    let keep = opened.clone();
    let session_hook = move |s: Option<&UploadSession>| {
        if let Some(s) = s {
            *keep.lock().unwrap() = Some(s.clone());
        }
    };
    let layout = arvolo_core::upload::SealedLayout::mailbox(source.clone(), seal.key, seal.size);
    let c = reqwest::Client::new();
    let id = c
        .post(format!("{relay}/v1/upload?ttl=3600&max=1"))
        .header(
            "x-arvolo-encapped-key",
            data_encoding::BASE32_NOPAD.encode(&seal.encapped_key),
        )
        .header(LENGTH, layout.len().to_string())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let head = layout.to_vec().unwrap()[..9_000_000].to_vec();
    c.patch(format!("{relay}/v1/upload/{id}"))
        .header(OFFSET, "0")
        .body(head)
        .send()
        .await
        .unwrap();
    let session = UploadSession {
        relay: relay.clone(),
        id: id.clone(),
        length: layout.len(),
    };

    let hooks = UploadHooks {
        session: &session_hook,
        progress: &|_| {},
    };
    let deposited = deposit_offline_with(
        &source,
        "big.bin",
        &seal,
        &me,
        &relay,
        3600,
        1,
        Some(session),
        &hooks,
    )
    .await
    .expect("deposited");
    assert!(
        opened.lock().unwrap().is_none(),
        "the session it was given was the one it finished"
    );

    let out = tempfile::tempdir().unwrap();
    let (path, _) = fetch_offline(
        &deposited.ticket.encode(),
        Some(out.path().join("big.bin")),
        &them,
        None,
    )
    .await
    .expect("fetched");
    assert_eq!(std::fs::read(path).unwrap(), data);
}

/// A link through the engine: a transfer that returns at once, reports progress,
/// and ends with its URL on the row and in an event.
#[tokio::test]
async fn the_engine_makes_a_link_as_a_transfer() {
    let (relay, _mb, _dir) = spawn_relay(true).await;
    let (f, data) = payload(1_200_000);
    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(Identity::generate(), Some(relay), dl.path().to_path_buf());
    let mut events = m.subscribe();
    let id = m.create_link(f.path(), 3600, 0).expect("started");

    let link = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match events.recv().await.unwrap() {
                ManagerEvent::LinkReady { id: e, info } if e == id => {
                    return info.link.expect("a link carries its URL")
                }
                ManagerEvent::Failed { id: e, error, .. } if e == id => panic!("{error}"),
                _ => {}
            }
        }
    })
    .await
    .expect("the link lands");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let t = m.get(id).unwrap();
        if t.status == TransferStatus::Completed {
            assert_eq!(t.link.as_deref(), Some(link.as_str()));
            assert_eq!(t.transferred, data.len() as u64);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "row never completed");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let (_, got) = fetch_link(&link).await;
    assert_eq!(got, data);
}

/// An explicit deposit run in the background: the id comes back at once, the
/// ticket lands on the row with the upload, and it opens.
#[tokio::test]
async fn a_detached_deposit_puts_its_ticket_on_the_row() {
    let (relay, _mb, _dir) = spawn_relay(true).await;
    let them = Identity::generate();
    let (f, data) = payload(700_000);
    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(Identity::generate(), Some(relay), dl.path().to_path_buf());
    let id = m
        .deposit_to_detached(
            &them.public(),
            f.path(),
            "f.bin".into(),
            String::new(),
            Default::default(),
            None,
        )
        .expect("started");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let ticket = loop {
        let t = m.get(id).unwrap();
        if let Some(ticket) = t.ticket.clone() {
            assert_eq!(t.status, TransferStatus::Deposited);
            break ticket;
        }
        if let TransferStatus::Failed(e) = &t.status {
            panic!("{e}");
        }
        assert!(std::time::Instant::now() < deadline, "never deposited");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let out = tempfile::tempdir().unwrap();
    let (path, _) = fetch_offline(&ticket, Some(out.path().join("f.bin")), &them, None)
        .await
        .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), data);
}
