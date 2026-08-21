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
