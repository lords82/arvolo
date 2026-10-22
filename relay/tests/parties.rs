//! Who is at each end, and what that costs the relay.
//!
//! The previous suite varied the *sources* during one download. This one varies
//! the **people**: a recipient who is not there at all, one whose daemon is there
//! while they are not, one who says yes — crossed with a sender that stays and one
//! that hands over and goes. Each combination has a route it is supposed to take,
//! and each route has a price in relay storage that is supposed to be paid only
//! when it has to be.
//!
//! So every test here asserts two things: the file is collectable by the route
//! that combination implies, and nothing was spent on the relay that the
//! combination did not require. The second half is the one that used to be
//! nobody's job — a full copy of a file could end up on the relay because a human
//! had not clicked yet.
//!
//! Own test binary: it sets `ARVOLO_DEPOSIT_AFTER`, which is process-global.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::flow::{self, fetch_offline};
use arvolo_core::manager::{TransferManager, TransferStatus};
use arvolo_core::presence::{check_online, publish_beacon, InboxSubscription, ReceivedOffer};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

/// A recipient who is merely undecided must not cost the relay anything, however
/// long they take. Long enough here that any deposit inside a test is a deposit
/// that was *not* waiting for them.
fn patient_grace() {
    std::env::set_var("ARVOLO_DEPOSIT_AFTER", "3600");
}

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

/// A recipient whose daemon is up: the beacon is refreshed for as long as the
/// returned token is uncancelled. One publish lasts about thirty seconds, which is
/// shorter than several of these tests.
fn daemon_present(relay: &str, who: &Identity) -> CancellationToken {
    let stop = CancellationToken::new();
    // The identity is rebuilt inside the task from its seed: it is not `Clone`, and
    // a borrow cannot outlive this call.
    let (relay, seed, tok) = (relay.to_string(), who.secret_bytes(), stop.clone());
    tokio::spawn(async move {
        let who = Identity::from_secret_bytes(&seed).expect("identity");
        let client = reqwest::Client::new();
        while !tok.is_cancelled() {
            let _ = publish_beacon(&client, &relay, &who).await;
            tokio::select! {
                _ = tok.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    });
    stop
}

/// A daemon that reads its inbox but never accepts — the offer reaches the
/// machine, the human does not answer. Polling is what marks the offer as having
/// *arrived*, which is how the sender tells this apart from nobody being home.
fn daemon_reading_inbox(relay: &str, who: &Identity) -> CancellationToken {
    let stop = CancellationToken::new();
    let (relay, seed, tok) = (relay.to_string(), who.secret_bytes(), stop.clone());
    tokio::spawn(async move {
        let who = Identity::from_secret_bytes(&seed).expect("identity");
        let sub = InboxSubscription::new(relay, &who);
        while !tok.is_cancelled() {
            let _ = sub.poll_wait(0).await;
            tokio::select! {
                _ = tok.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    });
    stop
}

/// Block until the relay actually reports `who` as online.
///
/// Publishing a beacon is a request, not a state: the send's first look happens
/// within milliseconds of `send_to`, and a recipient whose beacon has not landed
/// yet is — correctly — indistinguishable from one who is not there, so the send
/// goes straight to the mailbox. Two of these tests were measuring that race
/// instead of what they meant to.
async fn await_present(relay: &str, who: &Identity) {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if check_online(&client, relay, &who.public())
            .await
            .unwrap_or(false)
        {
            return;
        }
        assert!(Instant::now() < deadline, "the beacon never came up");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The first offer to land in `who`'s inbox, or fail the test.
async fn wait_offer(relay: &str, who: &Identity, within: Duration) -> ReceivedOffer {
    let sub = InboxSubscription::new(relay.to_string(), who);
    let deadline = Instant::now() + within;
    loop {
        if let Ok(mut offers) = sub.poll_wait(0).await {
            if !offers.is_empty() {
                return offers.remove(0);
            }
        }
        assert!(Instant::now() < deadline, "no offer ever reached them");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

fn tmpfile(bytes: &[u8]) -> tempfile::NamedTempFile {
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), bytes).unwrap();
    f
}

async fn send_to(m: &TransferManager, to: &Identity, src: &std::path::Path) -> u64 {
    m.send_to(
        &to.public(),
        src.to_path_buf(),
        "f.bin".into(),
        false,
        String::new(),
        None,
        false,
        None,
    )
    .await
    .unwrap()
}

/// Wait until `pred` holds of the mailbox, or give up. Used to watch *where the
/// bytes went*, which is what separates these routes from one another.
async fn wait_for(mailbox: &Mailbox, within: Duration, pred: impl Fn(&Mailbox) -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if pred(mailbox) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ----------------------------------------------------------------- the five

/// 1. Nobody home. With no daemon on the far side there is no offer standing and
///    so no chunk ticket in anyone's hands — seeding the pieces would put the file
///    somewhere the recipient has no way to name. It has to be the mailbox, which
///    carries its own way in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_absent_recipient_gets_the_mailbox_not_the_chunk_store() {
    patient_grace();
    let (relay, mailbox, _dir) = spawn_relay().await;
    let (sender, recipient) = (Identity::generate(), Identity::generate());
    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(sender, Some(relay.clone()), dl.path().to_path_buf());

    let src = tmpfile(b"a file for somebody who is not there");
    let id = send_to(&m, &recipient, src.path()).await;

    assert!(
        wait_for(&mailbox, Duration::from_secs(30), |mb| mb.len() == 1).await,
        "an absent recipient leaves the mailbox as the only route"
    );
    assert_eq!(
        mailbox.seeded_count(),
        0,
        "and nothing may be seeded: with no offer standing, chunks on the relay \
         would be unreachable"
    );
    m.cancel(id);
}

/// 2. They were away and came back. The sender's part is over — the mailbox holds
///    a sealed copy that opens without it — so the collection has to work with the
///    sending daemon out of the picture.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_absent_recipient_collects_when_they_come_back() {
    patient_grace();
    let (relay, mailbox, _dir) = spawn_relay().await;
    let (sender, recipient) = (Identity::generate(), Identity::generate());
    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(sender, Some(relay.clone()), dl.path().to_path_buf());

    let body = b"posted while they were out".to_vec();
    let src = tmpfile(&body);
    let id = send_to(&m, &recipient, src.path()).await;
    assert!(
        wait_for(&mailbox, Duration::from_secs(30), |mb| mb.len() == 1).await,
        "the file should have been deposited"
    );

    // They turn up later and read their inbox for the first time.
    let offer = wait_offer(&relay, &recipient, Duration::from_secs(30)).await;
    let out = dl.path().join("collected.bin");
    let (path, n) = fetch_offline(&offer.offer.ticket, Some(out), &recipient, None)
        .await
        .expect("collect from the mailbox");
    assert_eq!(n, body.len());
    assert_eq!(std::fs::read(path).unwrap(), body, "byte-for-byte");
    m.cancel(id);
}

/// 3. Their daemon is there; they are not. The offer has *arrived* — which is a
///    different thing from nobody being home, and the difference is what stops the
///    sender uploading a copy of the file because a human has not clicked yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_present_but_undecided_recipient_spends_no_relay_at_all() {
    patient_grace();
    let (relay, mailbox, _dir) = spawn_relay().await;
    let (sender, recipient) = (Identity::generate(), Identity::generate());
    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(sender, Some(relay.clone()), dl.path().to_path_buf());

    let beacon = daemon_present(&relay, &recipient);
    let reading = daemon_reading_inbox(&relay, &recipient);
    await_present(&relay, &recipient).await;

    let src = tmpfile(&vec![7u8; 64 * 1024]);
    let id = send_to(&m, &recipient, src.path()).await;

    // Past the point where a live attempt gives up on a connection that never comes
    // (90s is the network budget), so the send has really concluded "their daemon
    // has it and nobody has answered" rather than still being mid-attempt.
    tokio::time::sleep(Duration::from_secs(105)).await;

    assert_eq!(
        mailbox.len(),
        0,
        "an undecided human is not a reason to spend the relay's storage"
    );
    assert_eq!(mailbox.seeded_count(), 0, "nor its chunk store");
    let st = m.list().into_iter().find(|t| t.id == id).map(|t| t.status);
    assert!(
        matches!(
            st,
            Some(TransferStatus::Waiting(_)) | Some(TransferStatus::Active)
        ),
        "the send should still be holding the offer open, got {st:?}"
    );

    beacon.cancel();
    reading.cancel();
    m.cancel(id);
}

/// 4. Both there, and they say yes. The relay brokered the introduction and must
///    hold none of the payload afterwards — not a chunk, not a blob.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn both_present_delivers_direct_and_spends_no_relay_storage() {
    patient_grace();
    let (relay, mailbox, _dir) = spawn_relay().await;
    let (sender, recipient) = (Identity::generate(), Identity::generate());
    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(sender, Some(relay.clone()), dl.path().to_path_buf());

    let beacon = daemon_present(&relay, &recipient);
    await_present(&relay, &recipient).await;
    let body = vec![3u8; 512 * 1024];
    let src = tmpfile(&body);
    let id = send_to(&m, &recipient, src.path()).await;

    let offer = wait_offer(&relay, &recipient, Duration::from_secs(60)).await;
    let out = dl.path().join("direct.bin");
    let outcome = tokio::time::timeout(
        Duration::from_secs(120),
        flow::recv_chunked(
            &offer.offer.ticket,
            Some(out),
            Some(&recipient),
            RelayChoice::Disabled,
            CancellationToken::new(),
            |_| {},
        ),
    )
    .await
    .expect("a direct delivery should not hang")
    .expect("receive");

    assert!(outcome.is_complete());
    assert_eq!(
        std::fs::read(outcome.path()).unwrap(),
        body,
        "byte-for-byte"
    );
    assert_eq!(
        mailbox.len(),
        0,
        "nothing to deposit when they are both here"
    );
    assert_eq!(
        mailbox.seeded_count(),
        0,
        "and nothing to backfill: the receiver never dropped"
    );

    beacon.cancel();
    m.cancel(id);
}

/// 5. Their daemon is there, they have not answered, and the sender says "park it,
///    I'm going". That is the case parking exists for — and because an offer is
///    standing, the recipient already holds a chunk ticket, so the pieces can go to
///    the chunk store rather than a second whole copy going to the mailbox.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn parking_a_watched_send_uses_the_chunk_store_not_the_mailbox() {
    patient_grace();
    let (relay, mailbox, _dir) = spawn_relay().await;
    let (sender, recipient) = (Identity::generate(), Identity::generate());
    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(sender, Some(relay.clone()), dl.path().to_path_buf());

    let beacon = daemon_present(&relay, &recipient);
    await_present(&relay, &recipient).await;
    let body = vec![11u8; 256 * 1024];
    let src = tmpfile(&body);
    let id = send_to(&m, &recipient, src.path()).await;

    // Wait until the offer is really in their inbox: that is what "an offer is
    // standing" means, and it is the condition parking turns on.
    let offer = wait_offer(&relay, &recipient, Duration::from_secs(60)).await;

    assert!(
        m.deposit_now(id),
        "a waiting send should accept being parked"
    );
    assert!(
        wait_for(&mailbox, Duration::from_secs(60), |mb| mb.seeded_count()
            > 0)
        .await,
        "parking a watched send should put its pieces in the chunk store"
    );
    assert_eq!(
        mailbox.len(),
        0,
        "and must not also upload the whole file as a sealed blob — that would \
         spend the relay twice and throw away any progress they had made"
    );

    // Still collectable, which is the point of parking it anywhere at all.
    let out = dl.path().join("parked.bin");
    let outcome = tokio::time::timeout(
        Duration::from_secs(120),
        flow::recv_chunked(
            &offer.offer.ticket,
            Some(out),
            Some(&recipient),
            RelayChoice::Disabled,
            CancellationToken::new(),
            |_| {},
        ),
    )
    .await
    .expect("collecting a parked send should not hang")
    .expect("receive");
    assert!(outcome.is_complete());
    assert_eq!(
        std::fs::read(outcome.path()).unwrap(),
        body,
        "byte-for-byte"
    );

    beacon.cancel();
    m.cancel(id);
}
