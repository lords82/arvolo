//! Preparing a send is the expensive half — it reads and encrypts the whole
//! payload to compute the chunk digests — and a delivery loop retries. Before
//! [`flow::prepare_send_reusing`] every retry paid for that pass again, so a large
//! file spent most of its life re-encrypting itself between attempts, and minted a
//! fresh ticket each time: an offer already sitting in the recipient's inbox went
//! stale on every try.
//!
//! Both consequences are visible from the outside in one fact — the ticket — so
//! that is what these assert. Not byte for byte: the provider *address* changes
//! because each attempt binds a fresh socket. What has to be reproduced is what a
//! ticket already in someone's hands resolves and verifies through — the node id
//! and the chunk digests — and those come from the seed and the key the
//! preparation carries.

use arvolo_core::chunked::ChunkTicket;
use arvolo_core::flow;
use arvolo_core::transfer::RelayChoice;

fn payload(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("payload.bin");
    // Compressible but not uniform, so a wrong key would be obvious.
    let body: Vec<u8> = (0..(3 * 1024 * 1024)).map(|i| (i % 251) as u8).collect();
    std::fs::write(&p, &body).unwrap();
    p
}

#[tokio::test]
async fn a_reused_preparation_reproduces_the_same_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let file = payload(dir.path());

    let mut prep = None;
    let first = flow::prepare_send_reusing(
        file.as_path(),
        "payload.bin",
        false,
        None,
        None,
        RelayChoice::Disabled,
        &mut prep,
    )
    .await
    .expect("first preparation");
    assert!(
        prep.is_some(),
        "the preparation is kept for the next attempt"
    );

    let second = flow::prepare_send_reusing(
        file.as_path(),
        "payload.bin",
        false,
        None,
        None,
        RelayChoice::Disabled,
        &mut prep,
    )
    .await
    .expect("second attempt");

    let a = ChunkTicket::decode(&first.ticket).unwrap();
    let b = ChunkTicket::decode(&second.ticket).unwrap();
    assert_eq!(
        a.chunks, b.chunks,
        "the digests must be reproduced, or a ticket already handed out verifies \
         nothing that this attempt serves"
    );
    assert_eq!(
        a.providers[0].id, b.providers[0].id,
        "and the same node id, which is what discovery re-resolves an old ticket \
         to after the address changed"
    );
    assert_eq!(first.chunks, second.chunks);
    assert_eq!(first.total_size, second.total_size);
}

/// The same claim through the slot the daemon actually uses. The slot exists so the
/// preparation can be owned by something that outlives the delivery task — a pause
/// destroys the task, and re-preparing is what made the recipient's transfer die and
/// their next offer a new one to approve by hand.
#[tokio::test]
async fn a_preparation_in_a_slot_survives_the_attempt_that_made_it() {
    let dir = tempfile::tempdir().unwrap();
    let file = payload(dir.path());

    let slot = flow::PrepSlot::default();
    let first = flow::prepare_send_in_slot(
        file.as_path(),
        "payload.bin",
        false,
        None,
        None,
        RelayChoice::Disabled,
        &slot,
    )
    .await
    .expect("first attempt");
    assert!(
        slot.lock().unwrap().is_some(),
        "the slot keeps the preparation for whoever comes next"
    );

    // A second attempt, as a resume would make it: same slot, nothing else carried.
    let second = flow::prepare_send_in_slot(
        file.as_path(),
        "payload.bin",
        false,
        None,
        None,
        RelayChoice::Disabled,
        &slot,
    )
    .await
    .expect("second attempt");

    let (a, b) = (
        ChunkTicket::decode(&first.ticket).unwrap(),
        ChunkTicket::decode(&second.ticket).unwrap(),
    );
    assert_eq!(a.chunks, b.chunks);
    assert_eq!(a.providers[0].id, b.providers[0].id);
    assert_eq!(
        a.content_id(),
        b.content_id(),
        "and so the recipient sees one send, not two"
    );
}

/// What a persisted preparation has to be able to do: come back as parts and be the
/// same send again. Everything step 2 writes to disk is these four values.
#[tokio::test]
async fn a_preparation_taken_apart_and_rebuilt_is_the_same_send() {
    let dir = tempfile::tempdir().unwrap();
    let file = payload(dir.path());

    let slot = flow::PrepSlot::default();
    let first = flow::prepare_send_in_slot(
        file.as_path(),
        "payload.bin",
        false,
        None,
        None,
        RelayChoice::Disabled,
        &slot,
    )
    .await
    .expect("first attempt");

    let rebuilt = {
        let held = slot.lock().unwrap();
        let p = held.as_ref().expect("prepared");
        flow::ReusablePrep::from_parts(p.key(), p.node_seed(), p.total_size(), p.chunks().to_vec())
            .expect("rebuild from parts")
    };

    // A fresh slot seeded with the rebuilt preparation: this is a restarted daemon.
    let slot = flow::PrepSlot::new(std::sync::Mutex::new(Some(rebuilt)));
    let second = flow::prepare_send_in_slot(
        file.as_path(),
        "payload.bin",
        false,
        None,
        None,
        RelayChoice::Disabled,
        &slot,
    )
    .await
    .expect("after the restart");

    let (a, b) = (
        ChunkTicket::decode(&first.ticket).unwrap(),
        ChunkTicket::decode(&second.ticket).unwrap(),
    );
    assert_eq!(
        a.chunks, b.chunks,
        "no pass was paid for, and none was needed"
    );
    assert_eq!(
        a.providers[0].id, b.providers[0].id,
        "the seed is what an old ticket re-resolves through"
    );
}

/// The digest count has to describe the size, or the parts are not a preparation.
/// Cheap half of the guarantee the hashing pass gives for free.
#[test]
fn parts_that_do_not_describe_the_payload_are_refused() {
    let chunks = vec![arvolo_core::hash::Hash::new(b"one")];
    // One 16 MiB chunk's worth of digests, claiming three chunks' worth of bytes.
    assert!(
        flow::ReusablePrep::from_parts([7u8; 32], [9u8; 32], 40 * 1024 * 1024, chunks).is_err()
    );
}

/// The control: without the carried preparation, two attempts are two different
/// sends. This is what the old loop did on every retry.
#[tokio::test]
async fn a_fresh_preparation_is_a_different_send() {
    let dir = tempfile::tempdir().unwrap();
    let file = payload(dir.path());

    let a = flow::prepare_send(
        file.as_path(),
        "payload.bin",
        false,
        None,
        None,
        RelayChoice::Disabled,
    )
    .await
    .expect("first");
    let b = flow::prepare_send(
        file.as_path(),
        "payload.bin",
        false,
        None,
        None,
        RelayChoice::Disabled,
    )
    .await
    .expect("second");

    let (a, b) = (
        ChunkTicket::decode(&a.ticket).unwrap(),
        ChunkTicket::decode(&b.ticket).unwrap(),
    );
    assert_ne!(
        a.chunks, b.chunks,
        "a fresh content key re-encrypts the same bytes into different digests"
    );
    assert_ne!(
        a.providers[0].id, b.providers[0].id,
        "and a fresh seed is a different node"
    );
}
