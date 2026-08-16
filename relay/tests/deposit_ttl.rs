//! A relay that keeps deposits for less time than it was asked has to say so.
//!
//! The silence used to be actively harmful rather than merely unhelpful: the sender
//! built the recipient's inbox offer out of the TTL it *requested*, so a relay
//! capping deposits at a day still advertised the arrival for the whole week it was
//! asked for. The recipient then saw an ordinary offer, approved it on the second
//! day, and got a 404 from a claim reaped hours earlier — with `arvolo status`
//! insisting the file was there the entire time.
//!
//! Its own test binary because `ARVOLO_MAX_TTL` is read from the process
//! environment: set inside a shared test file it would leak into every other test
//! in it.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::flow::deposit_offline;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

/// What this relay will agree to keep, well under any TTL asked below.
const RELAY_MAX_TTL: u64 = 60;

async fn spawn_relay() -> (String, tempfile::TempDir) {
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
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), dir)
}

#[tokio::test]
async fn a_clamped_deposit_reports_the_ttl_it_actually_got() {
    std::env::set_var("ARVOLO_MAX_TTL", RELAY_MAX_TTL.to_string());
    let (relay, _dir) = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();

    let src = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(src.path(), b"a few bytes").unwrap();

    let asked = 7 * 24 * 3600;
    let deposited = deposit_offline(
        src.path(),
        "payload.bin",
        &recipient.public(),
        &sender,
        &relay,
        asked,
        1,
        None,
    )
    .await
    .expect("deposit");

    assert_eq!(
        deposited.ttl_secs, RELAY_MAX_TTL,
        "the deposit must report the relay's TTL, not the one requested — everything \
         downstream (the inbox offer above all) deadlines off this number"
    );
}

#[tokio::test]
async fn a_ttl_within_the_cap_comes_back_unchanged() {
    std::env::set_var("ARVOLO_MAX_TTL", RELAY_MAX_TTL.to_string());
    let (relay, _dir) = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();

    let src = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(src.path(), b"a few bytes").unwrap();

    // Asking for less than the cap must not be rounded *up* to it: the reported TTL
    // is what the relay agreed to, and it agreed to what was asked.
    let asked = RELAY_MAX_TTL / 2;
    let deposited = deposit_offline(
        src.path(),
        "payload.bin",
        &recipient.public(),
        &sender,
        &relay,
        asked,
        1,
        None,
    )
    .await
    .expect("deposit");

    assert_eq!(deposited.ttl_secs, asked);
}
