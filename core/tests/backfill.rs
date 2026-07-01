//! Relay-side backfill: seed chunks into a blob node, serve them, and release
//! (untag → GC deletes) them. Exercises the path that had the tag-vs-GC race.

use std::time::Duration;

use arvolo_core::backfill::BlobNode;
use arvolo_core::chunked::{ChunkReceiver, ChunkSender};
use arvolo_core::reexport::Hash;
use arvolo_core::transfer::RelayChoice;

/// Try to fetch a chunk from the blob node; `true` if it served it.
async fn served(receiver: &ChunkReceiver, node: &BlobNode, hash: Hash) -> bool {
    receiver.fetch_chunk(&[node.addr()], hash).await.is_ok()
}

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

    // Relay-side blob node. The GC interval is long enough not to run *during*
    // the (locally slow) multi-chunk seed, but short enough to assert prompt
    // collection after a release. (Production uses 15s and seeds are fast.)
    let store_dir = dir.path().join("relaystore");
    let node = BlobNode::spawn_with_gc(&store_dir, RelayChoice::Disabled, Duration::from_secs(10))
        .await
        .expect("blob node");

    // Seed both chunks from the sender into the relay's store.
    node.seed_chunks(sender.addr(), &chunks)
        .await
        .expect("seed chunks");

    let receiver = ChunkReceiver::open(RelayChoice::Disabled)
        .await
        .expect("receiver");

    // Both seeded chunks are complete and servable from the relay (this is what
    // the tag-before-fetch fix guarantees during a slow multi-chunk seed).
    for &h in &chunks {
        assert!(served(&receiver, &node, h).await, "seeded chunk must serve");
    }

    // Release chunk 0: its tag is removed and the periodic GC deletes the blob.
    node.release_hex(&chunks[0].to_string())
        .await
        .expect("release");

    // Within a couple of GC cycles the released chunk is gone…
    let mut gone = false;
    for _ in 0..30 {
        if !served(&receiver, &node, chunks[0]).await {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    assert!(gone, "released chunk should be collected by GC");

    // …while the un-released chunk is still served.
    assert!(
        served(&receiver, &node, chunks[1]).await,
        "un-released chunk must remain servable"
    );

    receiver.close().await;
    sender.shutdown().await;
}
