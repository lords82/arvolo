//! A held `send --to` survives a restart as *the same send*.
//!
//! The preparation — the chunk digests, and the content key and node seed they
//! belong to — used to die with the daemon. Coming back, the send prepared again:
//! half a minute of re-encryption on a large file, and, worse, a fresh key and a
//! fresh node id. The recipient's transfer was then pointing at a node nobody was
//! serving, and the only way back was another offer for them to approve by hand.
//!
//! What has to be reproduced is what a ticket already in someone's hands resolves
//! and verifies through: the node id and the content. Not the ticket string — the
//! provider address changes with every socket bind and the sealed key blob is
//! randomised by HPKE on every seal.
//!
//! Own test binary: it sets the process-global blob cap, which cannot be shared
//! with the other `deliver_to` tests.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arvolo_core::backfill::BlobNode;
use arvolo_core::chunked::ChunkTicket;
use arvolo_core::crypto::Identity;
use arvolo_core::manager::{TransferManager, TransferStatus};
use arvolo_core::presence::{publish_beacon, InboxSubscription};
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

/// Keep the recipient looking online for the whole test, so the loop keeps taking
/// the live path (nothing ever connects, so every attempt is abandoned — which is
/// the shape of a recipient whose daemon is up and whose human has not decided yet).
fn keep_beacon(relay: String, who: Identity) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            let _ = publish_beacon(&client, &relay, &who).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
}

/// The offer standing in `who`'s inbox, whose id is not `not`. `None` until one is.
async fn wait_offer(
    relay: &str,
    who: &Identity,
    not: Option<&str>,
) -> (String, arvolo_core::chunked::ChunkTicket) {
    let sub = InboxSubscription::new(relay.to_string(), who);
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Ok(offers) = sub.poll_wait(0).await {
            if let Some(o) = offers
                .iter()
                .find(|o| not.map(|n| o.id != n).unwrap_or(true))
            {
                let t = ChunkTicket::decode(&o.offer.ticket).expect("a live ticket");
                return (o.id.clone(), t);
            }
        }
        assert!(Instant::now() < deadline, "no offer arrived");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// How many stored preparations are in the state dir. One per held send, and none
/// once it ends — the record holds a content key.
fn prep_records(state: &std::path::Path) -> usize {
    std::fs::read_dir(state)
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("prep-"))
                .count()
        })
        .unwrap_or(0)
}

async fn wait_status<F: Fn(&TransferStatus) -> bool>(m: &TransferManager, id: u64, pred: F) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(t) = m.list().into_iter().find(|t| t.id == id) {
            if pred(&t.status) {
                return;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for status");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Both halves in one test, because they share the process-global blob cap: the
/// preparation is reused when the payload is untouched, and thrown away when it is
/// not. The second half is the control — without it the first passes on a guard
/// that never says no.
#[tokio::test]
async fn a_restarted_send_is_the_same_send_unless_the_payload_moved_on() {
    // Any real deposit is refused (413), so the send is held and can only go live.
    std::env::set_var("ARVOLO_MAX_BLOB_BYTES", "64");
    let (relay, _dir) = spawn_relay().await;

    let me = Identity::generate().secret_bytes();
    let recipient = Identity::generate();
    let beacon = keep_beacon(
        relay.clone(),
        Identity::from_secret_bytes(&recipient.secret_bytes()).unwrap(),
    );

    let dl = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let payload = work.path().join("big.bin");
    std::fs::write(&payload, vec![7u8; 4096]).unwrap();

    let manager = || {
        TransferManager::with_state_dir(
            Identity::from_secret_bytes(&me).unwrap(),
            Some(relay.clone()),
            dl.path().to_path_buf(),
            Some(state.path().to_path_buf()),
        )
    };

    // First daemon: offer the file, then pause and shut down.
    let first = {
        let m = manager();
        let id = m
            .send_to(
                &recipient.public(),
                payload.clone(),
                "big.bin".into(),
                false,
                String::new(),
            )
            .await
            .unwrap();
        let offer = wait_offer(&relay, &recipient, None).await;
        assert!(m.pause(id), "a held send is pausable");
        wait_status(&m, id, |s| matches!(s, TransferStatus::Paused(_))).await;
        offer
        // `m` drops: the daemon is gone, the send is paused on disk.
    };
    assert_eq!(
        prep_records(state.path()),
        1,
        "the preparation must be on disk before the daemon goes away"
    );

    // Second daemon, same identity and state dir: the send comes back paused, and
    // resuming it offers the *same* send again.
    let same = {
        let m = manager();
        assert!(m.resume_incomplete() >= 1, "the paused send must come back");
        let id = m
            .list()
            .into_iter()
            .find(|t| matches!(t.status, TransferStatus::Paused(_)))
            .expect("restored paused")
            .id;
        assert!(m.resume(id), "and be resumable");
        let offer = wait_offer(&relay, &recipient, Some(&first.0)).await;
        assert!(m.pause(id));
        wait_status(&m, id, |s| matches!(s, TransferStatus::Paused(_))).await;
        offer
    };

    assert_eq!(
        first.1.content_id(),
        same.1.content_id(),
        "a restart must not re-encrypt the payload into a different send"
    );
    assert_eq!(
        first.1.providers[0].id, same.1.providers[0].id,
        "and the node id an already-handed-out ticket resolves to must come back"
    );

    // The control: touch the payload while the daemon is down. The stored
    // preparation describes a file that no longer exists as it was, so the guard
    // refuses it and the send prepares again — which is what every restart did
    // before the preparation was written down at all.
    tokio::time::sleep(Duration::from_millis(1100)).await; // a filesystem mtime tick
    std::fs::write(&payload, vec![8u8; 4096]).unwrap();

    let after = {
        let m = manager();
        assert!(m.resume_incomplete() >= 1);
        let id = m
            .list()
            .into_iter()
            .find(|t| matches!(t.status, TransferStatus::Paused(_)))
            .expect("restored paused")
            .id;
        assert!(m.resume(id));
        let offer = wait_offer(&relay, &recipient, Some(&same.0)).await;
        m.cancel(id);
        offer
    };
    assert_ne!(
        same.1.content_id(),
        after.1.content_id(),
        "a payload that changed must be prepared again, not served under stale digests"
    );
    // Cancelled above: the send is over, so its content key must not be left lying
    // in the state dir.
    let deadline = Instant::now() + Duration::from_secs(10);
    while prep_records(state.path()) != 0 {
        assert!(
            Instant::now() < deadline,
            "a finished send must take its preparation with it"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    beacon.abort();
    std::env::remove_var("ARVOLO_MAX_BLOB_BYTES");
}
