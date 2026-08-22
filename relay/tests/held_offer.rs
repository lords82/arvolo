//! One held `send --to`, one offer.
//!
//! A file the relay refuses as too large can only go peer-to-peer, so the delivery
//! loop keeps trying while the recipient is away. Each attempt used to post its own
//! offer and withdraw it on the way out, which left the recipient's inbox holding a
//! row that was already dead and the sender waiting on an id nobody would ever name.
//! Whoever accepted the copy from ten minutes ago accepted an offer the loop had
//! thrown away — and the send sat at 0 B until the next attempt happened to overlap
//! with them.
//!
//! Own test binary: it sets the process-global blob cap, which cannot be shared with
//! the other `deliver_to` tests.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::manager::{TransferManager, TransferStatus};
use arvolo_core::presence::{publish_beacon, slot_for, InboxSubscription};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

/// Long enough for one live attempt to be abandoned: the watchdog gives a recipient
/// who never connects `LIVE_CONFIRM_SECS` (12s) before it gives up on them.
const ONE_ABANDONED_ATTEMPT: Duration = Duration::from_secs(15);

async fn spawn_relay() -> (String, Arc<Mailbox>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let node = BlobNode::spawn(dir.path(), RelayChoice::Disabled)
        .await
        .expect("blob node");
    let mailbox = Arc::new(Mailbox::in_memory().expect("mailbox"));
    let state = AppState::new(mailbox.clone(), Arc::new(node));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), mailbox, dir)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[tokio::test]
async fn a_held_send_keeps_one_offer_standing() {
    // Any real deposit is refused (413), so the send is held and can only go live.
    std::env::set_var("ARVOLO_MAX_BLOB_BYTES", "64");
    let (relay, mailbox, _dir) = spawn_relay().await;

    let sender = Identity::generate();
    let recipient = Identity::generate();
    let client = reqwest::Client::new();
    // Seen as online, so the loop keeps taking the live path — but with nothing on
    // the other end, so every attempt is abandoned. That is the shape of the bug:
    // a recipient whose daemon is up and whose human has not decided yet.
    publish_beacon(&client, &relay, &recipient)
        .await
        .expect("beacon");

    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(sender, Some(relay.clone()), dl.path().to_path_buf());
    let src = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(src.path(), vec![0u8; 4096]).unwrap();
    let id = m
        .send_to(
            &recipient.public(),
            src.path().to_path_buf(),
            "big.bin".into(),
            false,
            String::new(),
        )
        .await
        .unwrap();

    // Read the slot straight from the store: an HTTP listing would stamp the offer
    // `arrived`, which is one of the things under test here.
    let slot = slot_for(&recipient.public());
    let rows_now = || mailbox.inbox_list(&slot, now_unix());

    let deadline = Instant::now() + Duration::from_secs(30);
    let first = loop {
        let rows = rows_now();
        if let Some((id, _)) = rows.first() {
            break id.clone();
        }
        assert!(Instant::now() < deadline, "no offer was ever posted");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    // Let the first attempt run out on a recipient who never connects.
    tokio::time::sleep(ONE_ABANDONED_ATTEMPT).await;
    let rows = rows_now();
    assert_eq!(
        rows.len(),
        1,
        "the send is still held, so its offer must still be there to accept"
    );
    assert_eq!(
        rows[0].0, first,
        "the offer the recipient can see must stay the one the sender is waiting on"
    );

    // Their daemon lists the inbox: that is the wake-up, and it starts the next
    // attempt at once instead of at the end of the backoff.
    let sub = InboxSubscription::new(relay.clone(), &recipient);
    assert_eq!(
        sub.poll_wait(0).await.expect("poll").len(),
        1,
        "the recipient sees exactly one offer, not one per attempt"
    );
    tokio::time::sleep(Duration::from_secs(3)).await;

    let rows = rows_now();
    assert_eq!(
        rows.len(),
        1,
        "a second attempt must reuse the standing offer, not leave another copy"
    );
    assert_eq!(rows[0].0, first, "and it must be the same offer");

    let st = m.list().into_iter().find(|t| t.id == id).map(|t| t.status);
    assert!(
        matches!(
            st,
            Some(TransferStatus::Active)
                | Some(TransferStatus::Preparing)
                | Some(TransferStatus::Waiting(_))
        ),
        "the send should still be trying, got {st:?}"
    );

    m.cancel(id);
    std::env::remove_var("ARVOLO_MAX_BLOB_BYTES");
}
