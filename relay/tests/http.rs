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
    let state = AppState::new(
        Arc::new(Mailbox::in_memory().expect("mailbox")),
        Arc::new(node),
    );
    router(state)
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    resp.into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
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
async fn deposit_streams_large_multiframe_body() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;

    // A multi-frame body of several MiB must stream straight to disk and come back
    // byte-identical — this exercises the streaming deposit path (no in-RAM buffer)
    // that lets the mailbox carry large files.
    let frame = vec![7u8; 512 * 1024];
    let n = 9usize; // 4.5 MiB across 9 frames
    let expected = vec![7u8; frame.len() * n];
    let stream = futures_util::stream::iter((0..n).map(move |_| {
        Ok::<axum::body::Bytes, std::io::Error>(axum::body::Bytes::from(frame.clone()))
    }));

    let resp = app
        .clone()
        .oneshot(
            Request::post("/v1/deposit?ttl=3600&max=1")
                .header("x-arvolo-encapped-key", "AAAAAAAA")
                .body(Body::from_stream(stream))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let claim = String::from_utf8(body_bytes(resp).await).unwrap();

    let resp = app
        .oneshot(
            Request::get(format!("/v1/fetch/{claim}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp).await, expected);
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
    assert_eq!(
        rz_post(&app, "42", "ms", b"sender-pake").await,
        StatusCode::OK
    );
    let (st, v) = rz_get(&app, "42", "ms").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v, b"sender-pake");

    // A second claim on the same slot is refused.
    assert_eq!(
        rz_post(&app, "42", "ms", b"other").await,
        StatusCode::CONFLICT
    );

    // Receiver posts its message once (first write wins).
    assert_eq!(
        rz_post(&app, "42", "mr", b"recv-pake").await,
        StatusCode::OK
    );
    assert_eq!(rz_get(&app, "42", "mr").await.1, b"recv-pake");

    // A second write to ANY key (not just the slot claim) is refused, so a
    // stranger who guesses the slot can't clobber an in-flight message/ticket
    // and grief the pairing (F6). The stored value is unchanged.
    assert_eq!(
        rz_post(&app, "42", "mr", b"attacker").await,
        StatusCode::CONFLICT
    );
    assert_eq!(rz_get(&app, "42", "mr").await.1, b"recv-pake");
}

#[tokio::test]
async fn rz_value_too_large_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    // A rendezvous value must stay tiny; oversize bodies are refused (413) so the
    // unauthenticated rz table can't be stuffed with large blobs.
    let big = vec![0u8; 64 * 1024 + 1];
    assert_eq!(
        rz_post(&app, "big", "ms", &big).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

// ---- rendezvous v2 (long-lived, multi-session slots) ----------------------

/// A v2 owner token and the `own` body that claims a slot for it: the relay only
/// ever sees the hash.
fn owner_token() -> (String, Vec<u8>) {
    let raw: [u8; 32] = rand::random();
    let token = data_encoding::BASE32_NOPAD.encode(&raw).to_lowercase();
    let hash = blake3::hash(&raw).as_bytes().to_vec();
    (token, hash)
}

async fn rz_post_auth(
    app: &axum::Router,
    slot: &str,
    key: &str,
    token: Option<&str>,
    body: &[u8],
) -> StatusCode {
    let mut req = Request::post(format!("/v1/rz/{slot}/{key}"));
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    app.clone()
        .oneshot(req.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap()
        .status()
}

async fn rz_get_auth(
    app: &axum::Router,
    slot: &str,
    key: &str,
    token: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::get(format!("/v1/rz/{slot}/{key}"));
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_bytes(resp).await)
}

async fn rz_delete_auth(
    app: &axum::Router,
    slot: &str,
    key: &str,
    token: Option<&str>,
) -> StatusCode {
    let mut req = Request::delete(format!("/v1/rz/{slot}/{key}"));
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

/// Claim a v2 slot and return its owner token.
async fn rz_claim_v2(app: &axum::Router, slot: &str) -> String {
    let (token, hash) = owner_token();
    assert_eq!(
        rz_post_auth(app, slot, "own", None, &hash).await,
        StatusCode::OK,
        "claiming slot {slot}"
    );
    token
}

#[tokio::test]
async fn rz2_own_never_echoes_the_hash() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    assert_eq!(
        rz_get_auth(&app, "50", "own", None).await.0,
        StatusCode::NOT_FOUND
    );

    let (token, hash) = owner_token();
    assert_eq!(
        rz_post_auth(&app, "50", "own", None, &hash).await,
        StatusCode::OK
    );

    // Anyone may ask *whether* a slot speaks v2 — that's how a receiver picks a
    // protocol. Nobody may read back the verifier: holding it would be as good as
    // holding the token.
    let (st, body) = rz_get_auth(&app, "50", "own", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body, b"2");
    assert_ne!(body, hash);
    // Even the owner gets the marker, not the hash.
    assert_eq!(rz_get_auth(&app, "50", "own", Some(&token)).await.1, b"2");

    // A second claim is refused, so the nameplate retry loop still works.
    assert_eq!(
        rz_post_auth(&app, "50", "own", None, &owner_token().1).await,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn rz2_own_body_must_be_a_32_byte_hash() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    assert_eq!(
        rz_post_auth(&app, "51", "own", None, b"short").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        rz_post_auth(&app, "51", "own", None, &[0u8; 33]).await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn rz2_owner_keys_require_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    let token = rz_claim_v2(&app, "52").await;
    let (wrong, _) = owner_token();

    // The receiver's half is open to whoever shows up…
    assert_eq!(
        rz_post_auth(&app, "52", "r.abc", None, b"recv-pake").await,
        StatusCode::OK
    );
    assert_eq!(
        rz_post_auth(&app, "52", "c.abc", None, b"confirm").await,
        StatusCode::OK
    );

    // …but the sender's half is not. Without this a hostile receiver, who
    // legitimately knows its own session id, could pre-write the answer to its own
    // session and poison it.
    for key in ["s.abc", "t.abc"] {
        assert_eq!(
            rz_post_auth(&app, "52", key, None, b"x").await,
            StatusCode::FORBIDDEN,
            "{key} without a token"
        );
        assert_eq!(
            rz_post_auth(&app, "52", key, Some(&wrong), b"x").await,
            StatusCode::FORBIDDEN,
            "{key} with the wrong token"
        );
        assert_eq!(
            rz_post_auth(&app, "52", key, Some(&token), b"x").await,
            StatusCode::OK,
            "{key} with the right token"
        );
    }

    // Symmetrically, the receiver's messages are readable only by the owner: the
    // list of live session ids must not leak, or unguessable sids buy nothing.
    assert_eq!(
        rz_get_auth(&app, "52", "r.abc", None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        rz_get_auth(&app, "52", "r.abc", Some(&token)).await.1,
        b"recv-pake"
    );
    // The sender's messages are public — the receiver has no token.
    assert_eq!(rz_get_auth(&app, "52", "s.abc", None).await.1, b"x");
}

#[tokio::test]
async fn rz2_session_keys_need_a_claimed_slot() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    // No `own` row: there is nothing to have a session with, so junk sessions
    // can't be parked on an unclaimed nameplate.
    assert_eq!(
        rz_post_auth(&app, "53", "r.abc", None, b"x").await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn rz2_sessions_lists_only_unanswered_and_is_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    let token = rz_claim_v2(&app, "54").await;
    let (wrong, _) = owner_token();

    // Unauthenticated and wrongly-authenticated listings are refused.
    assert_eq!(
        rz_get_auth(&app, "54", "sessions", None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        rz_get_auth(&app, "54", "sessions", Some(&wrong)).await.0,
        StatusCode::FORBIDDEN
    );
    // It is read-only: nobody posts to it.
    assert_eq!(
        rz_post_auth(&app, "54", "sessions", Some(&token), b"x").await,
        StatusCode::METHOD_NOT_ALLOWED
    );

    // Empty to start.
    assert_eq!(
        rz_get_auth(&app, "54", "sessions", Some(&token)).await.1,
        b""
    );

    // Two receivers turn up.
    assert_eq!(
        rz_post_auth(&app, "54", "r.aaa", None, b"1").await,
        StatusCode::OK
    );
    assert_eq!(
        rz_post_auth(&app, "54", "r.bbb", None, b"2").await,
        StatusCode::OK
    );
    let (st, body) = rz_get_auth(&app, "54", "sessions", Some(&token)).await;
    assert_eq!(st, StatusCode::OK);
    let mut sids: Vec<&str> = std::str::from_utf8(&body).unwrap().lines().collect();
    sids.sort();
    assert_eq!(sids, vec!["aaa", "bbb"]);

    // Answering one drops it from the listing; the other still waits.
    assert_eq!(
        rz_post_auth(&app, "54", "s.aaa", Some(&token), b"sender-pake").await,
        StatusCode::OK
    );
    assert_eq!(
        rz_get_auth(&app, "54", "sessions", Some(&token)).await.1,
        b"bbb"
    );
}

#[tokio::test]
async fn rz2_ticket_get_burns_only_that_session() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    let token = rz_claim_v2(&app, "55").await;

    for sid in ["aaa", "bbb"] {
        assert_eq!(
            rz_post_auth(&app, "55", &format!("r.{sid}"), None, b"r").await,
            StatusCode::OK
        );
        assert_eq!(
            rz_post_auth(&app, "55", &format!("s.{sid}"), Some(&token), b"s").await,
            StatusCode::OK
        );
        assert_eq!(
            rz_post_auth(&app, "55", &format!("t.{sid}"), Some(&token), b"sealed").await,
            StatusCode::OK
        );
    }

    // Fetching one session's ticket delivers it and burns that session…
    let (st, v) = rz_get_auth(&app, "55", "t.aaa", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v, b"sealed");
    assert_eq!(
        rz_get_auth(&app, "55", "t.aaa", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        rz_get_auth(&app, "55", "s.aaa", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        rz_get_auth(&app, "55", "r.aaa", Some(&token)).await.0,
        StatusCode::NOT_FOUND
    );

    // …and nothing else. This is the whole difference from v1, where the first
    // fetch destroyed the slot and with it the code.
    assert_eq!(rz_get_auth(&app, "55", "t.bbb", None).await.1, b"sealed");
    assert_eq!(rz_get_auth(&app, "55", "own", None).await.1, b"2");
}

#[tokio::test]
async fn rz2_delete_frees_the_nameplate() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    let token = rz_claim_v2(&app, "56").await;
    let (wrong, _) = owner_token();

    assert_eq!(
        rz_delete_auth(&app, "56", "own", None).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        rz_delete_auth(&app, "56", "own", Some(&wrong)).await,
        StatusCode::FORBIDDEN
    );
    // Only the claim key is deletable — sessions go away with their ticket.
    assert_eq!(
        rz_delete_auth(&app, "56", "r.aaa", Some(&token)).await,
        StatusCode::METHOD_NOT_ALLOWED
    );

    assert_eq!(
        rz_delete_auth(&app, "56", "own", Some(&token)).await,
        StatusCode::OK
    );
    assert_eq!(
        rz_get_auth(&app, "56", "own", None).await.0,
        StatusCode::NOT_FOUND
    );
    // Cancelling a code hands the nameplate back rather than squatting it.
    let _ = rz_claim_v2(&app, "56").await;
}

#[tokio::test]
async fn rz2_and_v1_never_share_a_slot() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;

    // A v1 pairing holds the nameplate against a v2 claim…
    assert_eq!(rz_post(&app, "57", "ms", b"v1-pake").await, StatusCode::OK);
    assert_eq!(
        rz_post_auth(&app, "57", "own", None, &owner_token().1).await,
        StatusCode::CONFLICT
    );

    // …and a v2 slot holds it against a v1 one. Both 409s are what the sender's
    // retry loop already reacts to by picking a fresh nameplate.
    let _ = rz_claim_v2(&app, "58").await;
    assert_eq!(
        rz_post(&app, "58", "ms", b"v1-pake").await,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn rz2_v1_receiver_on_a_v2_slot_fails_fast() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    let _ = rz_claim_v2(&app, "59").await;

    // An old client polls `ms` and treats 404 as "not yet", so it would hang for
    // its full two-minute timeout. 410 is fatal to `poll_get`, so it fails at once
    // with something a user can act on.
    assert_eq!(rz_get(&app, "59", "ms").await.0, StatusCode::GONE);
}

#[tokio::test]
async fn rz_rejects_malformed_key() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    // Keys are part of a URL, of a log line, and (once sessions arrive) of a
    // delete predicate — so the vocabulary is fixed to lowercase ascii, digits
    // and `._-`, at most 64 chars. (Characters that can't appear in a URI at all,
    // like a space or a stray `%`, never reach the handler — the client refuses
    // to build the request. These are the ones that do reach it.)
    for bad in ["MS", "a~b", "a+b", "a:b", "-lead", ".lead", &"x".repeat(65)] {
        assert_eq!(
            rz_post(&app, "42", bad, b"v").await,
            StatusCode::BAD_REQUEST,
            "POST {bad:?} should be refused"
        );
        assert_eq!(
            rz_get(&app, "42", bad).await.0,
            StatusCode::BAD_REQUEST,
            "GET {bad:?} should be refused"
        );
    }
    // The shapes a real pairing uses are accepted *as keys*. What each one then
    // requires — a claimed slot for `r.`, the owner token for `t.` — is the
    // business of the tests below; here only the grammar is under test, so the
    // bar is "not rejected as malformed". (`own` is given a well-formed body,
    // since a bad one is also a 400 and would say nothing about the key.)
    for ok in ["ms", "mr", "tkt", "r.abc123", "t.a-b_c"] {
        assert_ne!(
            rz_post(&app, "43", ok, b"v").await,
            StatusCode::BAD_REQUEST,
            "{ok:?} is a well-formed key"
        );
    }
    assert_ne!(
        rz_post(&app, "44", "own", &[0u8; 32]).await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn rz_slot_row_cap() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;
    // 32 rows fit…
    for i in 0..32 {
        assert_eq!(
            rz_post(&app, "9", &format!("k{i}"), b"v").await,
            StatusCode::OK,
            "row {i} should fit"
        );
    }
    // …the 33rd is refused, and nothing already stored is disturbed.
    assert_eq!(
        rz_post(&app, "9", "k32", b"v").await,
        StatusCode::INSUFFICIENT_STORAGE
    );
    assert_eq!(rz_get(&app, "9", "k0").await.1, b"v");
    // The cap is per slot, so a different nameplate is unaffected.
    assert_eq!(rz_post(&app, "10", "ms", b"v").await, StatusCode::OK);
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

#[tokio::test]
async fn addr_is_rate_limited_per_ip() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path()).await;

    // `/v1/addr` is unauthenticated and mints a transfer token per call, so it
    // draws from the same per-IP write budget as the real writes. The tests
    // above run without connect info, which bypasses the limiter — here we
    // attach one, the way a real listener would.
    let from = |ip: &str| {
        let addr: std::net::SocketAddr = format!("{ip}:9").parse().unwrap();
        let mut req = Request::get("/v1/addr").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(addr));
        req
    };

    let cap = arvolo_relay::DEFAULT_WRITES_PER_MIN;
    for i in 0..cap {
        let st = app
            .clone()
            .oneshot(from("192.0.2.7"))
            .await
            .unwrap()
            .status();
        assert_eq!(st, StatusCode::OK, "request {i} of {cap} is within budget");
    }
    assert_eq!(
        app.clone()
            .oneshot(from("192.0.2.7"))
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS,
        "one past the budget is refused"
    );
    // The budget is per IP, not global: a different caller is not starved.
    assert_eq!(
        app.clone()
            .oneshot(from("192.0.2.8"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
