//! End-to-end presence/inbox over a real relay server: a sender deposits a sealed
//! offer in a recipient's inbox slot; the recipient long-polls, decrypts it,
//! authenticates the sender, and acks it. Reads/deletes are gated by a
//! proof-of-possession session — a stranger who knows the slot cannot drain it.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::presence::{
    offer_status, offer_status_waiting, post_offer, retract_offer, slot_for, InboxSubscription,
    Offer, OfferStatus,
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
        // No device id in these fixtures: the field is a same-identity
        // tiebreak, and every offer here comes from a different sender.
        origin: None,
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
            // No device id in these fixtures: the field is a same-identity
            // tiebreak, and every offer here comes from a different sender.
            origin: None,
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
            // No device id in these fixtures: the field is a same-identity
            // tiebreak, and every offer here comes from a different sender.
            origin: None,
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
            // No device id in these fixtures: the field is a same-identity
            // tiebreak, and every offer here comes from a different sender.
            origin: None,
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
            // No device id in these fixtures: the field is a same-identity
            // tiebreak, and every offer here comes from a different sender.
            origin: None,
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
    let slot = slot_for(&recipient.public());
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

    // A challenge, on the other hand, is free: the relay hands one to whoever asks,
    // because there is nothing secret in it and nothing to check the asker against —
    // it no longer learns who owns a slot. What it costs a stranger is nothing,
    // because the challenge is only useful to whoever can sign it.
    let sess = client
        .post(format!("{relay}/v1/inbox/{slot}/session"))
        .send()
        .await
        .expect("session");
    assert_eq!(sess.status(), reqwest::StatusCode::OK);
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

    let old_slot = slot_for_at(&me.public(), now_unix().saturating_sub(INBOX_EPOCH_SECS));
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

/// The proof, end to end and against the attacker who exists: a stranger who wants
/// somebody's inbox can ask the relay for a challenge — it will give them one — and
/// still cannot read a thing, because the token has to carry a signature that
/// verifies against the key the slot itself encodes.
///
/// This replaces a test of the old handshake, which asked whether the relay refused
/// a *public key* that did not match the slot. It had to ask that because the key
/// was handed over; the relay knowing every reader's identity was the price, and
/// removing it is what this change is for.
#[tokio::test]
async fn a_stranger_can_get_a_challenge_and_still_cannot_read_the_inbox() {
    let relay = spawn_relay().await;
    let owner = Identity::generate();
    let stranger = Identity::generate();
    let slot = slot_for(&owner.public());
    let client = reqwest::Client::new();

    // The challenge is public.
    let bytes = client
        .post(format!("{relay}/v1/inbox/{slot}/session"))
        .send()
        .await
        .expect("session")
        .bytes()
        .await
        .expect("challenge");
    let challenge: arvolo_core::presence::SessionChallenge =
        postcard::from_bytes(&bytes).expect("decode challenge");

    // Signing it as somebody else buys nothing.
    let epoch = now_unix() / arvolo_core::presence::INBOX_EPOCH_SECS;
    let msg = arvolo_core::presence::session_signing_input(&slot, &challenge.nonce, challenge.exp);
    let forged = arvolo_core::presence::SessionToken {
        nonce: challenge.nonce.clone(),
        exp: challenge.exp,
        mac: challenge.mac.clone(),
        sig: arvolo_core::crypto::blinded::sign_at(&stranger, epoch, &msg).to_vec(),
    };
    let bearer = |t: &arvolo_core::presence::SessionToken| {
        data_encoding::BASE32_NOPAD
            .encode(&postcard::to_allocvec(t).unwrap())
            .to_lowercase()
    };
    let resp = client
        .get(format!("{relay}/v1/inbox/{slot}?wait=0"))
        .bearer_auth(bearer(&forged))
        .send()
        .await
        .expect("get");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a signature from the wrong identity must not authenticate"
    );

    // The owner's signature over the same challenge does.
    let real = arvolo_core::presence::SessionToken {
        nonce: challenge.nonce,
        exp: challenge.exp,
        mac: challenge.mac,
        sig: arvolo_core::crypto::blinded::sign_at(&owner, epoch, &msg).to_vec(),
    };
    let resp = client
        .get(format!("{relay}/v1/inbox/{slot}?wait=0"))
        .bearer_auth(bearer(&real))
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
}

/// Put a durable non-offer row — a contact-sync note — in `me`'s current slot, and
/// return its relay id. This is the row the long-poll tests are about: it is never
/// acked and never expires, so the slot it lives in is non-empty for good.
async fn post_sync_note(relay: &str, me: &Identity) -> String {
    use arvolo_core::presence::encode_sync_note;
    use arvolo_core::sync::SyncNote;

    let slot = slot_for(&me.public());
    let body = encode_sync_note(&SyncNote::Snapshot {
        blob: vec![3u8; 32],
    })
    .expect("encode note");
    reqwest::Client::new()
        .post(format!("{relay}/v1/inbox/{slot}?ttl=3600"))
        .body(body)
        .send()
        .await
        .expect("post note")
        .text()
        .await
        .expect("id")
        .trim()
        .to_string()
}

/// A slot is not empty just because it has *something* in it: the sync cell sits
/// there for good, and the relay used to end every hold on it. That turned a
/// 25-second long-poll into a request every two seconds, per client, forever — the
/// GET storm that pegged a relay at >700 req/s.
#[tokio::test]
async fn a_row_the_reader_already_has_does_not_end_the_hold() {
    let relay = spawn_relay().await;
    let me = Identity::generate();
    post_sync_note(&relay, &me).await;

    let sub = InboxSubscription::new(relay.clone(), &me);
    // The first read is what teaches the subscription the row exists; the relay is
    // told about it from the next poll on.
    assert_eq!(sub.raw_items(0).await.expect("first read").len(), 1);

    let started = std::time::Instant::now();
    let again = sub.raw_items(3).await.expect("long-poll");
    let held_for = started.elapsed();

    assert!(
        again.is_empty(),
        "a hold that times out says 'nothing you don't have', not the slot's contents"
    );
    assert!(
        held_for >= std::time::Duration::from_secs(2),
        "the poll came back after {held_for:?}: the hold ended on a row the reader \
         already had, which is the storm this guards against"
    );
}

/// The other half: holding must not cost latency. A deposit wakes the slot it landed
/// in directly, so a held poll returns on the deposit rather than on its own timer.
#[tokio::test]
async fn a_new_offer_wakes_a_held_poll_at_once() {
    let relay = spawn_relay().await;
    let me = Identity::generate();
    let sender = Identity::generate();
    let client = reqwest::Client::new();
    post_sync_note(&relay, &me).await;

    let sub = InboxSubscription::new(relay.clone(), &me);
    assert_eq!(sub.raw_items(0).await.expect("first read").len(), 1);

    let started = std::time::Instant::now();
    let (got, ()) = tokio::join!(sub.raw_items(20), async {
        // Long enough that the poll is definitely holding when this lands.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        post_offer(
            &client,
            &relay,
            &me.public(),
            &sender,
            &Offer {
                name: "woken.bin".into(),
                size: 1,
                chunks: 1,
                ticket: "arvcWOKEN".into(),
                note: String::new(),
                sender_name: String::new(),
                origin: None,
            },
            None,
        )
        .await
        .expect("post offer");
    });
    let waited = started.elapsed();

    let got = got.expect("long-poll");
    assert_eq!(
        got.len(),
        2,
        "a return hands back the whole slot — the new offer and the note it sat next to"
    );
    // Well inside the 20s the poll asked for, and inside the relay's own idle
    // re-check too: only the deposit's wake-up can have ended the hold this fast.
    assert!(
        waited < std::time::Duration::from_secs(6),
        "the offer took {waited:?} to surface: the deposit did not wake the held poll"
    );
}

/// Post a throwaway live offer and hand back the poster's handle.
async fn post_live_offer(
    client: &reqwest::Client,
    relay: &str,
    recipient: &Identity,
    sender: &Identity,
) -> arvolo_core::presence::PostedOffer {
    post_offer(
        client,
        relay,
        &recipient.public(),
        sender,
        &Offer {
            name: "held.bin".into(),
            size: 1,
            chunks: 1,
            ticket: "arvcHELD".into(),
            note: String::new(),
            sender_name: String::new(),
            origin: None,
        },
        None,
    )
    .await
    .expect("post")
}

/// The sender's side of the same trick: a held `send --to` sleeps between delivery
/// attempts, and the recipient accepting is what should end that sleep. The ack must
/// come back to a waiting poster at once, not on its next look.
#[tokio::test]
async fn an_ack_wakes_a_held_status_poll_at_once() {
    let relay = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let client = reqwest::Client::new();
    let posted = post_live_offer(&client, &relay, &recipient, &sender).await;
    let who = recipient.public();

    // Their daemon lists the inbox first, as it would: that is `Arrived`, and it is
    // where a sender that has already seen the listing starts waiting from.
    let sub = InboxSubscription::new(relay.clone(), &recipient);
    assert_eq!(sub.poll_wait(0).await.expect("poll").len(), 1);

    let started = std::time::Instant::now();
    let (st, ()) = tokio::join!(
        offer_status_waiting(
            &client,
            &relay,
            &who,
            &posted.id,
            &posted.poster_token,
            OfferStatus::Arrived,
            20,
        ),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            sub.ack(&posted.id).await.expect("ack");
        }
    );
    let waited = started.elapsed();

    assert_eq!(st.expect("status"), OfferStatus::Taken);
    assert!(
        waited < std::time::Duration::from_secs(6),
        "the accept took {waited:?} to reach the sender: the ack did not wake the held poll"
    );
}

/// The recipient with no daemon never acks until the file is saved — much too late
/// to start the sender. What they do first is *list* their inbox, so that step has
/// to travel too.
#[tokio::test]
async fn a_listing_wakes_a_poster_waiting_from_pending() {
    let relay = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let client = reqwest::Client::new();
    let posted = post_live_offer(&client, &relay, &recipient, &sender).await;
    let who = recipient.public();

    let started = std::time::Instant::now();
    let (st, ()) = tokio::join!(
        offer_status_waiting(
            &client,
            &relay,
            &who,
            &posted.id,
            &posted.poster_token,
            OfferStatus::Pending,
            20,
        ),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let sub = InboxSubscription::new(relay.clone(), &recipient);
            assert_eq!(sub.poll_wait(0).await.expect("poll").len(), 1);
        }
    );
    let waited = started.elapsed();

    assert_eq!(st.expect("status"), OfferStatus::Arrived);
    assert!(
        waited < std::time::Duration::from_secs(6),
        "the listing took {waited:?} to reach the sender: it did not wake the held poll"
    );
}

/// And the half that keeps the relay standing: with nothing happening, the wait is
/// paid in full. A caller loops on this to pace itself, so an early return with no
/// news is not a wasted round trip — it is a request storm.
#[tokio::test]
async fn a_quiet_offer_holds_the_poll_for_the_whole_wait() {
    let relay = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let client = reqwest::Client::new();
    let posted = post_live_offer(&client, &relay, &recipient, &sender).await;

    let started = std::time::Instant::now();
    let st = offer_status_waiting(
        &client,
        &relay,
        &recipient.public(),
        &posted.id,
        &posted.poster_token,
        OfferStatus::Pending,
        3,
    )
    .await
    .expect("status");
    let waited = started.elapsed();

    assert_eq!(st, OfferStatus::Pending, "nobody touched it");
    assert!(
        waited >= std::time::Duration::from_secs(3),
        "returned after {waited:?} with no news: a loop on this would hammer the relay"
    );
}

/// The floor is the client's, not the relay's — it has to hold even when the answer
/// arrives instantly, which is exactly what a relay too old to know `wait` does.
/// Unreachable stands in for that here: it fails as fast as an instant answer.
#[tokio::test]
async fn no_news_from_an_unhelpful_relay_still_costs_the_full_wait() {
    let recipient = Identity::generate();
    let client = reqwest::Client::new();

    let started = std::time::Instant::now();
    let out = offer_status_waiting(
        &client,
        "http://127.0.0.1:1",
        &recipient.public(),
        "nosuchoffer",
        "token",
        OfferStatus::Pending,
        3,
    )
    .await;
    let waited = started.elapsed();

    assert!(out.is_err(), "a dead relay cannot answer");
    assert!(
        waited >= std::time::Duration::from_secs(3),
        "gave up after {waited:?}: the caller's pacing must not depend on the relay"
    );
}

/// Current unix seconds, as the slot derivation counts them.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
