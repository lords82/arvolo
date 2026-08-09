//! End-to-end chunked-mailbox tests: a real relay server over a loopback socket,
//! driving the client's `deposit_offline` (chunked, streamed to disk) and
//! `fetch_offline` (streamed decrypt). Exercises multi-chunk framing, the password
//! layer, and HPKE auth — the whole offline path that never buffers a file in RAM.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::flow::{deposit_offline, fetch_offline};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

/// Start a relay on an ephemeral loopback port; return its base URL.
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

/// A deterministic pseudo-random payload of `size` bytes in a temp file.
fn payload(size: usize) -> tempfile::NamedTempFile {
    let f = tempfile::NamedTempFile::new().unwrap();
    let bytes: Vec<u8> = (0..size)
        .map(|i| (i.wrapping_mul(31) % 251) as u8)
        .collect();
    std::fs::write(f.path(), &bytes).unwrap();
    f
}

#[tokio::test]
async fn chunked_roundtrip_multichunk() {
    let (relay, _dir) = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();

    // > 16 MiB (one CHUNK_SIZE) with an odd tail → several chunks, last one short.
    let size = 18 * 1024 * 1024 + 12_345;
    let src = payload(size);

    let deposited = deposit_offline(
        src.path(),
        "payload.bin",
        &recipient.public(),
        &sender,
        &relay,
        3600,
        1,
        None,
    )
    .await
    .expect("deposit");
    let ticket = deposited.ticket.encode();

    let out = tempfile::NamedTempFile::new().unwrap();
    let (path, n) = fetch_offline(&ticket, Some(out.path().to_path_buf()), &recipient, None)
        .await
        .expect("fetch");
    assert_eq!(n, size);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        std::fs::read(src.path()).unwrap(),
        "decrypted output must match the input byte-for-byte"
    );
}

#[tokio::test]
async fn chunked_roundtrip_with_password() {
    let (relay, _dir) = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let src = payload(700_000);

    let deposited = deposit_offline(
        src.path(),
        "payload.bin",
        &recipient.public(),
        &sender,
        &relay,
        3600,
        1,
        Some("hunter2"),
    )
    .await
    .expect("deposit");
    let ticket = deposited.ticket.encode();

    // Wrong / missing password fails.
    assert!(
        fetch_offline(&ticket, None, &recipient, None)
            .await
            .is_err(),
        "a password-protected ticket must not open without the password"
    );

    let out = tempfile::NamedTempFile::new().unwrap();
    let (path, n) = fetch_offline(
        &ticket,
        Some(out.path().to_path_buf()),
        &recipient,
        Some("hunter2"),
    )
    .await
    .expect("fetch with password");
    assert_eq!(n, 700_000);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        std::fs::read(src.path()).unwrap()
    );
}

#[tokio::test]
async fn wrong_recipient_cannot_decrypt() {
    let (relay, _dir) = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let stranger = Identity::generate();
    let src = payload(1_000);

    let deposited = deposit_offline(
        src.path(),
        "payload.bin",
        &recipient.public(),
        &sender,
        &relay,
        3600,
        // Allow two fetches so the stranger's attempt isn't just a burn miss.
        2,
        None,
    )
    .await
    .expect("deposit");
    let ticket = deposited.ticket.encode();

    assert!(
        fetch_offline(&ticket, None, &stranger, None).await.is_err(),
        "only the intended recipient can recover the content key"
    );
}
