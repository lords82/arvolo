//! The mailbox deposit seals and uploads the file as a lazy stream — no temp copy
//! on local disk. This proves the rewrite is *correct*, not just that it writes
//! nothing: a multi-chunk file deposited this way must come back byte-identical
//! when the recipient fetches and decrypts it.
//!
//! The risk the rewrite introduced is in the chunk loop — the last chunk is a
//! partial read, and sealing now happens lazily as the socket pulls, so an
//! off-by-one in `want` or a dropped final chunk would corrupt or truncate the blob
//! silently. A file spanning several full chunks plus a partial one exercises all
//! of that against a real relay.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::flow::{deposit_offline, fetch_offline};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

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
async fn a_streamed_multichunk_deposit_round_trips_byte_for_byte() {
    let relay = spawn_relay().await;
    let sender = Identity::generate();
    let recipient = Identity::generate();

    // 20 MiB: with a 16 MiB chunk that's one full chunk + a 4 MiB partial — the
    // full/partial boundary the rewrite is most likely to get wrong. Content varies
    // per byte so a misplaced or repeated chunk can't accidentally still match.
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("payload.bin");
    let data: Vec<u8> = (0..20 * 1024 * 1024u64)
        .map(|i| (i * 31 + 7) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    let deposited = deposit_offline(&src, &recipient.public(), &sender, &relay, 3600, 1, None)
        .await
        .expect("deposit should stream to the relay and succeed");

    let out = src_dir.path().join("fetched.bin");
    let (path, n) = fetch_offline(
        &deposited.ticket.encode(),
        Some(out.clone()),
        &recipient,
        None,
    )
    .await
    .expect("recipient fetch + decrypt");

    assert_eq!(path, out);
    assert_eq!(n, data.len(), "decrypted length must match the original");
    assert_eq!(
        std::fs::read(&out).unwrap(),
        data,
        "the streamed, on-the-fly-sealed blob must decrypt to the original bytes"
    );
}
