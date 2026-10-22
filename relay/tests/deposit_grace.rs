//! Waiting for a person is not waiting for a network.
//!
//! A `send --to` posts an offer and waits for the recipient to open the
//! connection — but the recipient opens it *when a human accepts*, so the
//! watchdog's connect timeout was measuring a decision with a stopwatch meant for
//! a handshake. When it ran out, the send uploaded the whole file to the relay.
//! Measured against a real recipient who was online and simply had not clicked:
//! 101 seconds from send to deposit, every time, for anybody who was not sitting
//! at their screen.
//!
//! What decides it now is evidence rather than the clock: an offer that reached
//! their device (`arrived`) and a beacon that is still alive mean the route works
//! and the answer has not come, so the send keeps waiting. A beacon that was up
//! and is now down means they left, and *that* deposits at once.
//!
//! These are slow on purpose — the grace only begins after the watchdog's own
//! 12 + 90 seconds, and shortening either would test something other than what
//! ships.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::manager::{TransferManager, TransferStatus};
use arvolo_core::presence::{publish_beacon, slot_for, InboxSubscription};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

/// 12s to be seen + 90s to connect, and room for a slow machine on top.
const PAST_THE_WATCHDOG: Duration = Duration::from_secs(125);

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

/// A recipient whose client is up: the beacon lapses after 30s on the relay, so
/// being online means saying so again and again, exactly as a listening manager
/// does. Returns the token that stops it — dropping the beacon is how this test
/// says "they closed the laptop".
fn keep_beacon_alive(relay: String, who: Arc<Identity>) -> CancellationToken {
    let stop = CancellationToken::new();
    let token = stop.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            let _ = publish_beacon(&client, &relay, &who).await;
            tokio::select! {
                _ = token.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    });
    stop
}

/// Start a send and wait until its offer is in the recipient's inbox *and* has
/// been read by them, which is the state the whole change turns on.
///
/// The payload comes from the caller and stays there: a `NamedTempFile` deletes
/// itself when it drops, and a send whose file vanishes fails with "is not a
/// file" long before it reaches anything this test is about.
async fn send_and_let_them_see_it(
    m: &TransferManager,
    relay: &str,
    mailbox: &Mailbox,
    recipient: &Identity,
    src: &std::path::Path,
) -> u64 {
    let id = m
        .send_to(
            &recipient.public(),
            src.to_path_buf(),
            "grace.bin".into(),
            false,
            String::new(),
            None,
            false,
            None,
        )
        .await
        .unwrap();

    let slot = slot_for(&recipient.public());
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if !mailbox.inbox_list(&slot, now_unix()).is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "no offer was ever posted");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Their daemon lists the inbox — over HTTP, because that is what stamps the
    // offer `arrived`. It never accepts: this is the offer sitting in front of
    // somebody who has not answered yet.
    let sub = InboxSubscription::new(relay.to_string(), recipient);
    assert_eq!(sub.poll_wait(0).await.expect("poll").len(), 1);
    id
}

fn status_of(m: &TransferManager, id: u64) -> Option<TransferStatus> {
    m.list().into_iter().find(|t| t.id == id).map(|t| t.status)
}

async fn wait_deposited(m: &TransferManager, id: u64, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        if matches!(status_of(m, id), Some(TransferStatus::Deposited)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected the send to be deposited, got {:?}",
            status_of(m, id)
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The case that used to cost a relay copy every time: they are there, their app
/// has the offer, and nobody has clicked yet. Nothing is uploaded — and the way
/// out is asking for it, not waiting for a timer.
#[tokio::test]
async fn a_recipient_who_is_there_but_has_not_answered_is_waited_for() {
    let (relay, mailbox, _dir) = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Arc::new(Identity::generate());
    let beacon = keep_beacon_alive(relay.clone(), recipient.clone());

    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(sender, Some(relay.clone()), dl.path().to_path_buf());
    let src = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(src.path(), vec![7u8; 4096]).unwrap();
    let id = send_and_let_them_see_it(&m, &relay, &mailbox, &recipient, src.path()).await;

    tokio::time::sleep(PAST_THE_WATCHDOG).await;

    let st = status_of(&m, id);
    assert!(
        matches!(st, Some(TransferStatus::Waiting(_))),
        "an offer they have seen must keep waiting, not be parked on the relay — got {st:?}"
    );

    // And the way out of the wait works, without cancelling and sending again.
    assert!(m.deposit_now(id), "a waiting send can be parked by hand");
    wait_deposited(&m, id, Duration::from_secs(60)).await;

    beacon.cancel();
    m.cancel(id);
}

/// The other half, and the reason the wait is safe to make long: somebody closing
/// their laptop is not somebody thinking about it. A beacon that was up and is
/// now gone deposits at once, without serving out the grace.
#[tokio::test]
async fn a_recipient_who_leaves_is_not_waited_for() {
    let (relay, mailbox, _dir) = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Arc::new(Identity::generate());
    let beacon = keep_beacon_alive(relay.clone(), recipient.clone());

    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(sender, Some(relay.clone()), dl.path().to_path_buf());
    let src = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(src.path(), vec![7u8; 4096]).unwrap();
    let id = send_and_let_them_see_it(&m, &relay, &mailbox, &recipient, src.path()).await;

    // They go. The relay's beacon lapses on its own within 30 seconds, long before
    // the watchdog reaches its verdict.
    beacon.cancel();

    wait_deposited(&m, id, PAST_THE_WATCHDOG + Duration::from_secs(60)).await;
    m.cancel(id);
}

/// `deposit_now` is not a second way to spell "deposit": it answers a question
/// only a waiting send is being asked. Saying `Ok` for anything else would let a
/// front end report a file parked on the relay when nothing had happened.
#[tokio::test]
async fn only_a_waiting_send_can_be_parked_by_hand() {
    let (relay, _mailbox, _dir) = spawn_relay().await;
    let m = TransferManager::new(
        Identity::generate(),
        Some(relay),
        tempfile::tempdir().unwrap().path().to_path_buf(),
    );
    assert!(
        !m.deposit_now(9_999),
        "a transfer that does not exist cannot be parked"
    );
}
