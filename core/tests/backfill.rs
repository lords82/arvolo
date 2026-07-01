//! Relay-side backfill over the custom chunk protocol: seed chunks into the
//! relay's file store, serve them, and release (delete) them.

use arvolo_core::backfill::BlobNode;
use arvolo_core::chunked::{ChunkReceiver, ChunkSender};
use arvolo_core::reexport::Hash;
use arvolo_core::transfer::RelayChoice;

/// Try to fetch a chunk from the relay node; `true` if it served it.
async fn served(receiver: &ChunkReceiver, node: &BlobNode, hash: Hash) -> bool {
    receiver.fetch_chunk(&[node.addr()], hash).await.is_ok()
}

// Integration test over local iroh endpoints (sender → relay → receiver). Kept
// #[ignore] by default because local iroh connectivity gets starved under the
// full suite's parallel load; reliable in isolation and validated live:
//   cargo test -p arvolo-core --test backfill -- --ignored
#[ignore = "iroh integration; run in isolation via --ignored"]
#[tokio::test]
async fn seed_serve_release_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    // ~24 MiB -> 2 chunks (16 + 8).
    let data: Vec<u8> = (0..24 * 1024 * 1024).map(|i| (i * 13 + 5) as u8).collect();
    std::fs::write(&path, &data).unwrap();

    let sender = ChunkSender::serve(&path, RelayChoice::Disabled)
        .await
        .expect("sender");
    let chunks = sender.chunks().to_vec();
    assert_eq!(chunks.len(), 2);

    let store_dir = dir.path().join("relaystore");
    let node = BlobNode::spawn(&store_dir, RelayChoice::Disabled)
        .await
        .expect("relay node");

    // Seed both chunks from the sender into the relay's file store.
    node.seed_chunks(sender.addr(), &chunks)
        .await
        .expect("seed chunks");

    let receiver = ChunkReceiver::open(RelayChoice::Disabled)
        .await
        .expect("receiver");

    // Both seeded chunks are complete and servable from the relay.
    for &h in &chunks {
        assert!(served(&receiver, &node, h).await, "seeded chunk must serve");
    }

    // Release chunk 0: its file is deleted immediately (deterministic).
    node.release_hex(&chunks[0].to_string())
        .await
        .expect("release");
    assert!(
        !served(&receiver, &node, chunks[0]).await,
        "released chunk must no longer serve"
    );
    // Release is idempotent.
    node.release_hex(&chunks[0].to_string())
        .await
        .expect("release idempotent");
    // The un-released chunk is still served.
    assert!(
        served(&receiver, &node, chunks[1]).await,
        "un-released chunk must remain servable"
    );

    receiver.close().await;
    sender.shutdown().await;
}
