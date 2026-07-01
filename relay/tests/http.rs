//! HTTP integration tests for the relay: drive the axum `router` in-process
//! (no socket) via `tower::ServiceExt::oneshot`. Covers the mailbox endpoints,
//! burn-after-read, unknown claims, the addr endpoint, and idempotent release.

use std::path::Path;
use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot

async fn app(store_dir: &Path) -> axum::Router {
    let node = BlobNode::spawn(store_dir, RelayChoice::Disabled)
        .await
        .expect("blob node");
    let state = AppState {
        mailbox: Arc::new(Mailbox::in_memory().expect("mailbox")),
        blobs: Arc::new(node),
    };
    router(state)
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    resp.into_body().collect().await.unwrap().to_bytes().to_vec()
}

#[tokio::test]
async fn healthz_ok() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    let resp = app
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp).await, b"ok");
}

#[tokio::test]
async fn deposit_fetch_then_burn() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    let ciphertext = b"opaque-ciphertext-payload".to_vec();

    // Deposit (ttl 1h, max 1 download). encapped key header is base32.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/v1/deposit?ttl=3600&max=1")
                .header("x-arvolo-encapped-key", "AAAAAAAA")
                .body(Body::from(ciphertext.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let claim = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(!claim.is_empty());

    // First fetch returns the ciphertext + the encapped-key header.
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/v1/fetch/{claim}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().contains_key("x-arvolo-encapped-key"));
    assert_eq!(body_bytes(resp).await, ciphertext);

    // Second fetch: burned after one download -> no longer available. The entry
    // is deleted on its last allowed read, so this is 404 (or 410 if a backend
    // ever kept an exhausted marker).
    let resp = app
        .oneshot(
            Request::get(format!("/v1/fetch/{claim}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        matches!(resp.status(), StatusCode::NOT_FOUND | StatusCode::GONE),
        "burned claim must be gone, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn fetch_unknown_claim_404() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    let resp = app
        .oneshot(
            Request::get("/v1/fetch/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deposit_rejects_missing_encapped_header() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    let resp = app
        .oneshot(
            Request::post("/v1/deposit")
                .body(Body::from(vec![1, 2, 3]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn addr_returns_address_and_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    let resp = app
        .oneshot(Request::get("/v1/addr").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8(body_bytes(resp).await).unwrap();
    let mut lines = body.lines();
    let addr = lines.next().unwrap_or("");
    let token = lines.next().unwrap_or("");
    assert!(!addr.is_empty(), "addr line present");
    assert!(!token.is_empty(), "token line present");
}

#[tokio::test]
async fn release_unseeded_is_noop_ok() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    // Releasing a (token, hash) that was never seeded is a harmless no-op.
    let resp = app
        .oneshot(
            Request::post("/v1/release/faketoken/deadbeef")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp).await, b"ok");
}

// ---- rendezvous (short-code pairing) --------------------------------------

async fn rz_post(app: &axum::Router, slot: &str, key: &str, body: &[u8]) -> StatusCode {
    app.clone()
        .oneshot(
            Request::post(format!("/v1/rz/{slot}/{key}"))
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn rz_get(app: &axum::Router, slot: &str, key: &str) -> (StatusCode, Vec<u8>) {
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/v1/rz/{slot}/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_bytes(resp).await)
}

#[tokio::test]
async fn rz_claim_put_get_and_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;

    // Unposted key is 404.
    assert_eq!(rz_get(&app, "42", "ms").await.0, StatusCode::NOT_FOUND);

    // Sender claims the slot.
    assert_eq!(rz_post(&app, "42", "ms", b"sender-pake").await, StatusCode::OK);
    let (st, v) = rz_get(&app, "42", "ms").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v, b"sender-pake");

    // A second claim on the same slot is refused.
    assert_eq!(
        rz_post(&app, "42", "ms", b"other").await,
        StatusCode::CONFLICT
    );

    // Receiver posts its message (non-claim key overwrites freely).
    assert_eq!(rz_post(&app, "42", "mr", b"recv-pake").await, StatusCode::OK);
    assert_eq!(rz_get(&app, "42", "mr").await.1, b"recv-pake");
}

#[tokio::test]
async fn rz_ticket_fetch_burns_slot() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    assert_eq!(rz_post(&app, "7", "ms", b"x").await, StatusCode::OK);
    assert_eq!(
        rz_post(&app, "7", "tkt", b"encrypted-ticket").await,
        StatusCode::OK
    );

    // First fetch of the ticket returns it…
    let (st, v) = rz_get(&app, "7", "tkt").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v, b"encrypted-ticket");

    // …and burns the whole slot: ticket and the claim key are both gone.
    assert_eq!(rz_get(&app, "7", "tkt").await.0, StatusCode::NOT_FOUND);
    assert_eq!(rz_get(&app, "7", "ms").await.0, StatusCode::NOT_FOUND);
}
