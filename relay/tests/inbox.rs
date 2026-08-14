//! End-to-end presence/inbox over a real relay server: a sender deposits a sealed
//! offer in a recipient's inbox slot; the recipient long-polls, decrypts it,
//! authenticates the sender, and acks it. Reads/deletes are gated by a
//! proof-of-possession session — a stranger who knows the slot cannot drain it.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::presence::{
    offer_status, post_offer, retract_offer, slot_for, InboxSubscription, Offer, OfferStatus,
};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

/// Spawn the relay HTTP server on an ephemeral port; return its base URL.
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

#[tokio::test]
async fn offer_round_trips_and_acks() {
    let relay = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let client = reqwest::Client::new();

    let offer = Offer {
        name: "report.pdf".into(),
        size: 4096,
        chunks: 1,
        ticket: "arvcFAKETICKET".into(),
        note: String::new(),
        sender_name: String::new(),
    };
    post_offer(&client, &relay, &recipient.public(), &sender, &offer, None)
        .await
        .expect("post offer");

    let sub = InboxSubscription::new(relay.clone(), &recipient);
    let got = sub.poll().await.expect("poll");
    assert_eq!(got.len(), 1, "one offer delivered");
    let received = &got[0];
    assert_eq!(received.offer, offer, "offer survives the round trip");
    assert_eq!(
        received.sender.to_bytes(),
        sender.public().to_bytes(),
        "sender is HPKE-authenticated"
    );

    // Ack it, then the inbox is empty (wait=0 → return immediately, no long-poll).
    sub.ack(&received.id).await.expect("ack");
    let after = sub.poll_wait(0).await.expect("poll after ack");
    assert!(after.is_empty(), "acked offer does not reappear");
}

#[tokio::test]
async fn poster_can_retract_its_own_offer() {
    let relay = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let client = reqwest::Client::new();

    let posted = post_offer(
        &client,
        &relay,
        &recipient.public(),
        &sender,
        &Offer {
            name: "live.bin".into(),
            size: 1,
            chunks: 1,
            ticket: "arvcLIVE".into(),
            note: String::new(),
            sender_name: String::new(),
        },
        None,
    )
    .await
    .expect("post");

    let sub = InboxSubscription::new(relay.clone(), &recipient);
    assert_eq!(
        sub.poll_wait(0).await.expect("poll").len(),
        1,
        "offer present"
    );

    // A wrong token must NOT delete it.
    retract_offer(&client, &relay, &recipient.public(), &posted.id, "wrong")
        .await
        .expect("retract call");
    assert_eq!(
        sub.poll_wait(0).await.expect("poll").len(),
        1,
        "wrong token leaves the offer"
    );

    // The real poster token retracts it.
    retract_offer(
        &client,
        &relay,
        &recipient.public(),
        &posted.id,
        &posted.poster_token,
    )
    .await
    .expect("retract");
    assert!(
        sub.poll_wait(0).await.expect("poll").is_empty(),
        "poster retracted its own offer"
    );
}

/// The whole ladder, in the order a real offer walks it: `pending` → `arrived` →
/// `taken`.
///
/// The last step is the one worth guarding. The recipient's ack used to delete the
/// row, so "they took it" and "it expired unread" were the same answer — `gone` —
/// and the sender's only positive signal was the middle one, which any
/// authenticated read sets — a `recv` listing as much as a daemon poll. Reporting a
/// glance at a list as a delivery is the failure this separation exists to prevent,
/// so the assertions below pin both halves: a read must reach `Arrived` and stop
/// there, and only the ack may reach `Taken`.
#[tokio::test]
async fn offer_status_walks_pending_then_arrived_then_taken() {
    let relay = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let client = reqwest::Client::new();

    let posted = post_offer(
        &client,
        &relay,
        &recipient.public(),
        &sender,
        &Offer {
            name: "live.bin".into(),
            size: 1,
            chunks: 1,
            ticket: "arvcLIVE".into(),
            note: String::new(),
            sender_name: String::new(),
        },
        None,
    )
    .await
    .expect("post");

    // Before anyone polls: pending.
    let st = offer_status(
        &client,
        &relay,
        &recipient.public(),
        &posted.id,
        &posted.poster_token,
    )
    .await
    .expect("status");
    assert_eq!(st, OfferStatus::Pending);

    // A wrong poster token is rejected (401 → error).
    assert!(
        offer_status(&client, &relay, &recipient.public(), &posted.id, "wrong")
            .await
            .is_err()
    );

    // The recipient's authenticated read gets the offer onto their machine — and
    // no further. Reading twice must not promote it either: the second poll is
    // exactly what a `recv` listing followed by a `status` looks like, and neither
    // is anyone taking anything.
    let sub = InboxSubscription::new(relay.clone(), &recipient);
    assert_eq!(sub.poll_wait(0).await.expect("poll").len(), 1);
    assert_eq!(sub.poll_wait(0).await.expect("poll").len(), 1);
    let st = offer_status(
        &client,
        &relay,
        &recipient.public(),
        &posted.id,
        &posted.poster_token,
    )
    .await
    .expect("status");
    assert_eq!(
        st,
        OfferStatus::Arrived,
        "a read says the offer reached them, never that they took it"
    );

    // Only the ack — which the client sends once the file is actually saved —
    // reaches `Taken`.
    sub.ack(&posted.id).await.expect("ack");
    let st = offer_status(
        &client,
        &relay,
        &recipient.public(),
        &posted.id,
        &posted.poster_token,
    )
    .await
    .expect("status");
    assert_eq!(
        st,
        OfferStatus::Taken,
        "an expiry also produces `Gone`, so a taken offer must not report it"
    );

    // The tombstone answers the poster and nothing else: the recipient must not be
    // offered the same file a second time.
    assert!(
        sub.poll_wait(0).await.expect("poll").is_empty(),
        "a taken offer is out of the recipient's inbox"
    );
}

/// A retracted offer is `Gone`, not `Taken`: the tombstone must record that the
/// recipient acted, never that the sender did. They are opposite outcomes and the
/// sender is the one asking.
#[tokio::test]
async fn a_retracted_offer_is_gone_not_taken() {
    let relay = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let client = reqwest::Client::new();

    let posted = post_offer(
        &client,
        &relay,
        &recipient.public(),
        &sender,
        &Offer {
            name: "recalled.bin".into(),
            size: 1,
            chunks: 1,
            ticket: "arvcRECALL".into(),
            note: String::new(),
            sender_name: String::new(),
        },
        None,
    )
    .await
    .expect("post");

    // It reached the recipient's client, then the sender pulled it back anyway.
    let sub = InboxSubscription::new(relay.clone(), &recipient);
    assert_eq!(sub.poll_wait(0).await.expect("poll").len(), 1);
    retract_offer(
        &client,
        &relay,
        &recipient.public(),
        &posted.id,
        &posted.poster_token,
    )
    .await
    .expect("retract");

    let st = offer_status(
        &client,
        &relay,
        &recipient.public(),
        &posted.id,
        &posted.poster_token,
    )
    .await
    .expect("status");
    assert_eq!(st, OfferStatus::Gone);
}

#[tokio::test]
async fn wrong_recipient_sees_nothing_in_its_own_inbox() {
    let relay = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let eavesdropper = Identity::generate();
    let client = reqwest::Client::new();

    post_offer(
        &client,
        &relay,
        &recipient.public(),
        &sender,
        &Offer {
            name: "secret.bin".into(),
            size: 1,
            chunks: 1,
            ticket: "arvcX".into(),
            note: String::new(),
            sender_name: String::new(),
        },
        None,
    )
    .await
    .expect("post offer");

    // A different identity polls *its own* (different) slot: nothing there.
    let other = InboxSubscription::new(relay.clone(), &eavesdropper);
    assert!(other.poll_wait(0).await.expect("poll").is_empty());

    // The real recipient still gets it.
    let sub = InboxSubscription::new(relay, &recipient);
    assert_eq!(sub.poll_wait(0).await.expect("poll").len(), 1);
}

#[tokio::test]
async fn unauthenticated_read_and_delete_are_rejected() {
    let relay = spawn_relay().await;
    let recipient = Identity::generate();
    let slot = slot_for(&recipient.public().to_bytes());
    let client = reqwest::Client::new();

    // GET without a session token → 401 (a stranger can't enumerate presence).
    let get = client
        .get(format!("{relay}/v1/inbox/{slot}?wait=0"))
        .send()
        .await
        .expect("get");
    assert_eq!(get.status(), reqwest::StatusCode::UNAUTHORIZED);

    // DELETE without a session token → 401 (a stranger can't suppress offers).
    let del = client
        .delete(format!("{relay}/v1/inbox/{slot}/anything"))
        .send()
        .await
        .expect("delete");
    assert_eq!(del.status(), reqwest::StatusCode::UNAUTHORIZED);

    // A session for a *different* identity can't authenticate this slot: its
    // pubkey doesn't hash to `slot`, so /session is forbidden.
    let stranger = Identity::generate();
    let sess = client
        .post(format!("{relay}/v1/inbox/{slot}/session"))
        .body(stranger.public().to_bytes())
        .send()
        .await
        .expect("session");
    assert_eq!(sess.status(), reqwest::StatusCode::FORBIDDEN);
}

/// The rotation contract, end to end: a row deposited in the *previous* epoch's
/// slot — which is what every row posted before a boundary becomes — is still
/// found, and acking it deletes it where it actually lives.
///
/// A sync note rather than an offer, because a note is exactly the long-lived row
/// this has to hold for, and because `poll_wait` deliberately leaves notes alone
/// (so this exercises the read path without the offer-decode path acking it).
#[tokio::test]
async fn a_row_left_in_the_previous_epochs_slot_is_still_read_and_acked() {
    use arvolo_core::presence::{encode_sync_note, slot_for_at, INBOX_EPOCH_SECS};
    use arvolo_core::sync::SyncNote;

    let relay = spawn_relay().await;
    let me = Identity::generate();
    let client = reqwest::Client::new();

    let old_slot = slot_for_at(
        &me.public().to_bytes(),
        now_unix().saturating_sub(INBOX_EPOCH_SECS),
    );
    let body = encode_sync_note(&SyncNote::Snapshot {
        blob: vec![7u8; 32],
    })
    .expect("encode note");
    let id = client
        .post(format!("{relay}/v1/inbox/{old_slot}?ttl=3600"))
        .body(body)
        .send()
        .await
        .expect("post")
        .text()
        .await
        .expect("id")
        .trim()
        .to_string();
    assert!(!id.is_empty());

    let sub = InboxSubscription::new(relay.clone(), &me);
    let items = sub.raw_items(0).await.expect("read inbox");
    assert_eq!(
        items.len(),
        1,
        "a row one epoch old must still be inside the read window"
    );
    assert_eq!(items[0].id, id);

    // The ack has to go to the old slot. Sent to the current one the relay would
    // answer 204 and delete nothing, and the row would come back forever.
    sub.ack(&id).await.expect("ack");
    assert!(
        sub.raw_items(0).await.expect("re-read").is_empty(),
        "the ack landed in the wrong slot: the row is still there"
    );
}

/// The window has a far edge: the relay must not hand out a session for a slot no
/// live row can be in, or a leaked public id would buy an attacker every past slot.
#[tokio::test]
async fn a_session_is_granted_for_the_window_and_refused_outside_it() {
    use arvolo_core::presence::{slot_for_at, INBOX_EPOCH_SECS};

    let relay = spawn_relay().await;
    let me = Identity::generate();
    let client = reqwest::Client::new();
    let pk = me.public().to_bytes();
    let now = now_unix();

    for (label, when, want) in [
        ("current", now, reqwest::StatusCode::OK),
        (
            "previous",
            now.saturating_sub(INBOX_EPOCH_SECS),
            reqwest::StatusCode::OK,
        ),
        (
            "two epochs back",
            now.saturating_sub(2 * INBOX_EPOCH_SECS),
            reqwest::StatusCode::FORBIDDEN,
        ),
        (
            "next epoch",
            now + INBOX_EPOCH_SECS,
            reqwest::StatusCode::FORBIDDEN,
        ),
    ] {
        let slot = slot_for_at(&pk, when);
        let resp = client
            .post(format!("{relay}/v1/inbox/{slot}/session"))
            .body(pk.clone())
            .send()
            .await
            .expect("session");
        assert_eq!(resp.status(), want, "session for the {label} slot");
    }
}

/// Current unix seconds, as the slot derivation counts them.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
