//! The `/v1/have` contract: what the relay will answer about what it holds, and
//! the two limits that keep a cheap batched read from being a cheap way to load
//! the box.
//!
//! The question itself discloses nothing new — the chunk protocol already serves
//! any held chunk to whoever names its hash, with no token — so what is worth
//! pinning down here is the shape of the answer and the refusals.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::chunked::{decode_have_bitfield, HaveRequest};
use arvolo_core::hash::Hash;
use arvolo_core::swarm::bitfield_has;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox, MAX_HAVE_CHUNKS_PER_REQ};

async fn spawn_relay() -> String {
    let dir = tempfile::tempdir().unwrap();
    let node = BlobNode::spawn(dir.path(), RelayChoice::Disabled)
        .await
        .expect("blob node");
    let state = AppState::new(
        Arc::new(Mailbox::in_memory().expect("mailbox")),
        Arc::new(node),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _dir = dir;
        axum::serve(listener, router(state)).await.unwrap();
    });
    format!("http://{addr}")
}

fn hashes(n: usize) -> Vec<Hash> {
    (0..n)
        .map(|i| Hash::new(format!("chunk-{i}").as_bytes()))
        .collect()
}

#[tokio::test]
async fn a_relay_holding_nothing_answers_no_to_everything() {
    let relay = spawn_relay().await;
    let chunks = hashes(3);
    let body = HaveRequest {
        chunks: chunks.clone(),
    }
    .encode()
    .unwrap();

    let resp = reqwest::Client::new()
        .post(format!("{relay}/v1/have"))
        .body(body)
        .send()
        .await
        .expect("have");
    assert!(resp.status().is_success());
    let bits = decode_have_bitfield(&resp.text().await.unwrap(), chunks.len()).expect("bitfield");
    for i in 0..chunks.len() {
        assert!(!bitfield_has(&bits, i), "claimed to hold chunk {i}");
    }
}

#[tokio::test]
async fn a_question_longer_than_the_cap_is_refused() {
    let relay = spawn_relay().await;
    let body = HaveRequest {
        chunks: hashes(MAX_HAVE_CHUNKS_PER_REQ + 1),
    }
    .encode()
    .unwrap();

    let resp = reqwest::Client::new()
        .post(format!("{relay}/v1/have"))
        .body(body)
        .send()
        .await
        .expect("have");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        "an unbounded batch is the whole risk of this route"
    );
}

#[tokio::test]
async fn a_body_that_is_not_a_question_is_refused() {
    let relay = spawn_relay().await;
    let resp = reqwest::Client::new()
        .post(format!("{relay}/v1/have"))
        .body("not-a-have-request")
        .send()
        .await
        .expect("have");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

/// A client has to be able to tell a relay that knows the route from one that
/// does not, because the two need different behaviour from it (ask, versus fall
/// back to being told by the sender).
#[tokio::test]
async fn the_route_is_advertised() {
    let relay = spawn_relay().await;
    let body = reqwest::get(format!("{relay}/v1/features"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("\"have\":true"), "features said: {body}");
}

/// An answer claiming more pieces than were asked about is an answer to a
/// different question — the client must not read it as its own.
#[test]
fn an_overlong_answer_is_rejected() {
    let long = arvolo_core::chunked::encode_have_bitfield(&[0xff, 0xff, 0xff]);
    assert!(
        decode_have_bitfield(&long, 3).is_err(),
        "3 pieces need one byte; three bytes answer something else"
    );
}

#[test]
fn a_question_survives_the_round_trip() {
    let chunks = hashes(5);
    let encoded = HaveRequest {
        chunks: chunks.clone(),
    }
    .encode()
    .unwrap();
    assert!(encoded.starts_with("arvh"));
    let back = HaveRequest::decode(&encoded).expect("decode");
    assert_eq!(back.chunks, chunks);
}
