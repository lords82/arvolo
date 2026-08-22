//! The wiring behind "don't ask me twice": an offer that arrives about a file
//! already coming down never reaches the user, and is acked off the relay.
//!
//! `core/tests/offer_dedup.rs` pins the decision — what counts as the same send.
//! This pins that the decision is actually taken where offers arrive, and that the
//! offer does not simply vanish: the ack marks it *taken*, which is what wakes a
//! sender that is sitting on its standing offer waiting for exactly that.

use std::sync::Arc;
use std::time::Duration;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::flow;
use arvolo_core::manager::{ManagerEvent, TransferManager};
use arvolo_core::presence::{self, Offer};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

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
async fn an_offer_for_a_download_already_running_never_reaches_the_user() {
    let (relay, _dir) = spawn_relay().await;
    let work = tempfile::tempdir().unwrap();
    let dl = tempfile::tempdir().unwrap();

    let sender = Identity::generate();
    let me = Identity::generate();
    let my_id = me.public();

    // A real live ticket whose sender is not serving: the download stays trying,
    // which is the state a paused sender leaves its recipient in.
    let payload = work.path().join("big.bin");
    std::fs::write(&payload, vec![5u8; 64 * 1024]).unwrap();
    let session = flow::prepare_send(
        payload.as_path(),
        "big.bin",
        false,
        Some((&sender, &my_id)),
        None,
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket = session.ticket.clone();
    let size = session.total_size;
    drop(session);

    let m = TransferManager::new(me, Some(relay.clone()), dl.path().to_path_buf());
    let mut events = m.subscribe();
    let _inbox = m.spawn_inbox().expect("listen");
    let id = m.start_download(
        ticket.clone(),
        dl.path().join("big.bin"),
        Some(sender.public()),
        "big.bin".into(),
        size,
    );

    // The sender offers it again — a resume, or just the next attempt.
    let client = reqwest::Client::new();
    let posted = presence::post_offer(
        &client,
        &relay,
        &my_id,
        &sender,
        &Offer {
            name: "big.bin".into(),
            size,
            chunks: 1,
            ticket: ticket.clone(),
            note: String::new(),
            sender_name: "the sender".into(),
            origin: None,
        },
        None,
    )
    .await
    .expect("post the offer");

    // Nothing to decide: no row parked, no event, and the relay told the sender
    // somebody took it.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(ev) = events.try_recv() {
            assert!(
                !matches!(ev, ManagerEvent::OfferReceived { .. }),
                "an offer for a file already downloading must not ask again"
            );
        }
        let status =
            presence::offer_status(&client, &relay, &my_id, &posted.id, &posted.poster_token)
                .await
                .expect("status");
        if matches!(status, presence::OfferStatus::Taken) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the swallowed offer was never acked — the sender would keep waiting on it"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Drain whatever else the transfer emitted while we waited: the offer must not
    // be in there either.
    while let Ok(ev) = events.try_recv() {
        assert!(
            !matches!(ev, ManagerEvent::OfferReceived { .. }),
            "an offer for a file already downloading must not ask again"
        );
    }

    m.cancel(id);
}
