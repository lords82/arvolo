//! Presence beacon over a real relay, plus the offline (mailbox) round-trip that
//! the `send_to` fallback relies on: deposit an `arvm` blob and fetch it back.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::presence::{check_online, presence_slot_for, publish_beacon};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{now_unix, router, AppState, Mailbox};

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

#[tokio::test]
async fn beacon_makes_a_contact_appear_online() {
    let (relay, mailbox) = spawn_relay().await;
    let me = Identity::generate();
    let client = reqwest::Client::new();

    // No beacon yet → offline.
    assert!(!check_online(&client, &relay, &me.public()).await.unwrap());

    // Publish a beacon → the contact reads as online.
    publish_beacon(&client, &relay, &me).await.expect("beacon");
    assert!(check_online(&client, &relay, &me.public()).await.unwrap());

    // Once the beacon expires and is reaped, the contact is offline again.
    // (Reap with a far-future "now" to force expiry without waiting the TTL.)
    let slot = presence_slot_for(&me.public().to_bytes());
    assert!(mailbox.beacon_alive(&slot, now_unix()));
    mailbox.beacon_reap(now_unix() + 10 * 24 * 3600);
    assert!(!check_online(&client, &relay, &me.public()).await.unwrap());
}

#[tokio::test]
async fn offline_deposit_round_trips() {
    use arvolo_core::flow::{deposit_offline, fetch_offline};

    let (relay, _mailbox) = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("payload.bin");
    let bytes: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src, &bytes).unwrap();

    // Deposit sealed to the recipient (the `send_to` offline path).
    let deposited = deposit_offline(&src, &recipient.public(), &sender, &relay, 3600, 1, None)
        .await
        .expect("deposit");
    let ticket = deposited.ticket.encode();
    assert!(
        ticket.starts_with("arvm"),
        "offline ticket is an arvm ticket"
    );

    // The accept path (arvm branch) fetches it back, byte-identical.
    let out = dir.path().join("out.bin");
    let (path, n) = fetch_offline(&ticket, Some(out.clone()), &recipient, None)
        .await
        .expect("fetch");
    assert_eq!(n, bytes.len());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
