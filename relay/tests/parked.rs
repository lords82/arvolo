//! Parking has to be as good a promise as depositing, or it should not replace it.
//!
//! A mailbox deposit says three things to the person who made it: it lasts a week,
//! you can take it back, and you will learn whether it was collected. Parking a
//! send as chunks said none of them — the pieces lapsed in a day, nothing could
//! withdraw them, and nothing watched. These tests pin all three, plus the clamp
//! that stops a caller asking for a lifetime the relay never agreed to.
//!
//! Own test binary: two of these set process-global env.

use std::sync::Arc;

use arvolo_core::backfill::{relay_have, revoke_seeded, BlobNode};
use arvolo_core::chunked::ChunkTicket;
use arvolo_core::flow;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

async fn spawn_relay() -> (String, Arc<Mailbox>) {
    let dir = tempfile::tempdir().unwrap();
    let node = BlobNode::spawn(dir.path(), RelayChoice::Disabled)
        .await
        .expect("blob node");
    let mailbox = Arc::new(Mailbox::in_memory().expect("mailbox"));
    let state = AppState::new(mailbox.clone(), Arc::new(node));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _dir = dir;
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), mailbox)
}

async fn parked_session(relay: &str, dir: &std::path::Path) -> flow::SendSession {
    let src = dir.join("f.bin");
    std::fs::write(&src, vec![5u8; 2 * 1024 * 1024]).unwrap();
    flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.to_string()),
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send")
}

/// The regression this whole file exists for. Parked pieces used to get the
/// backfill's day, while the offer naming them lived a week — so a recipient who
/// came back on day two found an arrival pointing at nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_send_lasts_as_long_as_the_offer_that_names_it() {
    let (relay, mailbox) = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let session = parked_session(&relay, dir.path()).await;

    let granted = session
        .park_on_relay(flow::PARK_TTL_SECS, &CancellationToken::new(), |_| {})
        .await
        .expect("the relay should have taken it");
    assert_eq!(
        granted,
        flow::PARK_TTL_SECS,
        "asked for the offer's lifetime and must be told what was granted"
    );

    // And the relay really wrote that deadline down, not the old fixed day.
    let ticket = ChunkTicket::decode(&session.ticket).unwrap();
    let now = arvolo_relay::now_unix();
    let expiries = mailbox.seed_expiries(&ticket.relay.as_ref().unwrap().token);
    assert!(!expiries.is_empty(), "nothing was recorded as seeded");
    for exp in expiries {
        assert!(
            exp > now + 6 * 24 * 3600,
            "a parked chunk expiring in {}s is the one-day backfill TTL, not the week asked for",
            exp.saturating_sub(now)
        );
    }
    session.shutdown().await;
}

/// Withdrawal. Cancelling a parked send has to take the pieces off the relay, not
/// merely stop pointing at them — otherwise "cancel" hides a row while the file
/// stays collectable, which is the one thing a cancel must never mean.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_send_can_be_taken_back() {
    let (relay, mailbox) = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let session = parked_session(&relay, dir.path()).await;
    assert!(session
        .park_on_relay(flow::PARK_TTL_SECS, &CancellationToken::new(), |_| {})
        .await
        .is_some());

    let ticket = ChunkTicket::decode(&session.ticket).unwrap();
    let token = ticket.relay.as_ref().unwrap().token.clone();
    let client = reqwest::Client::new();

    // Before: the relay serves them.
    let held = relay_have(&client, &relay, &ticket.chunks).await.unwrap();
    assert_eq!(held.len(), ticket.chunks.len(), "the park did not land");

    let dropped = revoke_seeded(&client, &relay, &token)
        .await
        .expect("withdrawal");
    assert_eq!(dropped, ticket.chunks.len(), "every piece should have gone");

    // After: nothing of ours is there, by the relay's own account and by its
    // bookkeeping both — a file left behind that no row names can never be freed.
    let held = relay_have(&client, &relay, &ticket.chunks).await.unwrap();
    assert!(held.is_empty(), "the relay still serves withdrawn pieces");
    assert_eq!(mailbox.seeded_bytes(), 0, "the rows outlived the files");

    // Idempotent: asking again is not an error, because what the caller wanted is
    // already true.
    assert_eq!(revoke_seeded(&client, &relay, &token).await.unwrap(), 0);
    session.shutdown().await;
}

/// One transfer's withdrawal is not everyone's. The token is per-transfer, and a
/// relay that took it as "delete the chunks named here" rather than "delete this
/// transfer's chunks" would let one sender clear another's parked send — the two
/// are told apart only by the token, so this is the test that says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn withdrawing_one_parked_send_leaves_the_others_alone() {
    let (relay, _mailbox) = spawn_relay().await;
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    // Different bytes, so the two have different chunk hashes.
    std::fs::write(dir_a.path().join("f.bin"), vec![1u8; 2 * 1024 * 1024]).unwrap();
    std::fs::write(dir_b.path().join("f.bin"), vec![2u8; 2 * 1024 * 1024]).unwrap();

    let a = parked_session(&relay, dir_a.path()).await;
    let b = flow::prepare_send(
        &dir_b.path().join("f.bin"),
        "f.bin",
        false,
        None,
        Some(relay.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    for s in [&a, &b] {
        assert!(s
            .park_on_relay(flow::PARK_TTL_SECS, &CancellationToken::new(), |_| {})
            .await
            .is_some());
    }

    let ta = ChunkTicket::decode(&a.ticket).unwrap();
    let tb = ChunkTicket::decode(&b.ticket).unwrap();
    let client = reqwest::Client::new();
    revoke_seeded(&client, &relay, &ta.relay.as_ref().unwrap().token)
        .await
        .unwrap();

    assert!(relay_have(&client, &relay, &ta.chunks)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        relay_have(&client, &relay, &tb.chunks).await.unwrap().len(),
        tb.chunks.len(),
        "withdrawing one send took another's pieces with it"
    );
    a.shutdown().await;
    b.shutdown().await;
}
