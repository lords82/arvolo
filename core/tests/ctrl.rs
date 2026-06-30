//! Milestone A, step 1: the sender tracks which chunks the receiver acked.

use arvolo_core::chunked::{ChunkReceiver, ChunkSender};
use arvolo_core::transfer::RelayChoice;

#[tokio::test]
async fn sender_tracks_delivered_chunks() {
    // ~24 MiB -> 2 chunks (16 + 8).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    let data: Vec<u8> = (0..24 * 1024 * 1024).map(|i| (i * 7 + 1) as u8).collect();
    std::fs::write(&path, &data).unwrap();

    let sender = ChunkSender::serve(&path, RelayChoice::Disabled)
        .await
        .expect("sender");
    let n = sender.chunks().len();
    eprintln!("STEP served, {n} chunks");
    assert_eq!(sender.delivered_count(), 0);

    let receiver = ChunkReceiver::open(RelayChoice::Disabled)
        .await
        .expect("receiver");
    eprintln!("STEP receiver open");
    let mut control = receiver
        .open_control(&sender.addr())
        .await
        .expect("control");
    eprintln!("STEP control open");

    for i in 0..n {
        let bytes = receiver
            .fetch_chunk(&[sender.addr()], sender.chunks()[i])
            .await
            .expect("fetch chunk");
        assert!(!bytes.is_empty());
        control.ack(i as u32).await.expect("ack");
        eprintln!("STEP chunk {i} fetched+acked");
    }
    control.finish().await.expect("finish");
    eprintln!("STEP control finished");

    for _ in 0..60 {
        if sender.delivered_count() == n {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    eprintln!("STEP delivered_count = {}", sender.delivered_count());
    assert_eq!(
        sender.delivered_count(),
        n,
        "sender should see all chunks delivered"
    );
    eprintln!("STEP DONE");
}
