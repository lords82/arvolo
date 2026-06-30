//! Milestone A, step 1: the sender tracks which chunks the receiver acked.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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
    let on_relay = Arc::new(Mutex::new(HashSet::new()));
    let mut control = receiver
        .open_control(&sender.addr(), on_relay)
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

/// When the receiver drops after acking only some chunks, the sender reports the
/// remaining (undelivered) chunks — the tail to backfill.
#[tokio::test]
async fn sender_reports_undelivered_tail_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    let data = vec![7u8; 40 * 1024 * 1024]; // ~3 chunks
    std::fs::write(&path, &data).unwrap();

    let sender = ChunkSender::serve(&path, RelayChoice::Disabled)
        .await
        .expect("sender");
    let n = sender.chunks().len();
    assert!(n >= 3);

    let receiver = ChunkReceiver::open(RelayChoice::Disabled)
        .await
        .expect("receiver");
    let on_relay = Arc::new(Mutex::new(HashSet::new()));
    let mut control = receiver
        .open_control(&sender.addr(), on_relay)
        .await
        .expect("control");

    // Ack only chunk 0, then drop the control connection (receiver "goes down").
    control.ack(0).await.expect("ack 0");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    drop(control);

    let undelivered =
        tokio::time::timeout(std::time::Duration::from_secs(30), sender.receiver_gone())
            .await
            .expect("receiver_gone timed out");
    let expected: Vec<usize> = (1..n).collect();
    assert_eq!(
        undelivered, expected,
        "sender should report the undelivered tail"
    );

    receiver.close().await;
    sender.shutdown().await;
}

/// The chunks served (and thus anything a relay would hold) are ciphertext; only
/// a holder of the ticket's content key can recover the plaintext. Covers a
/// multi-chunk file with a short final chunk.
#[tokio::test]
async fn chunked_roundtrip_is_encrypted_and_decryptable() {
    use arvolo_core::crypto::{open_chunk, CHUNK_KEY_LEN};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    // 16 MiB + 123 bytes -> 2 chunks, the last one short.
    let data: Vec<u8> = (0..16 * 1024 * 1024 + 123)
        .map(|i| (i * 31 + 7) as u8)
        .collect();
    std::fs::write(&path, &data).unwrap();

    let sender = ChunkSender::serve(&path, RelayChoice::Disabled)
        .await
        .expect("sender");
    let n = sender.chunks().len();
    assert_eq!(n, 2);
    let key: [u8; CHUNK_KEY_LEN] = sender.key();

    let receiver = ChunkReceiver::open(RelayChoice::Disabled)
        .await
        .expect("receiver");

    let chunk_size = sender.chunk_size() as usize;
    let mut reassembled = Vec::new();
    for i in 0..n {
        let ct = receiver
            .fetch_chunk(&[sender.addr()], sender.chunks()[i])
            .await
            .expect("fetch chunk");
        // What's on the wire / would be on the relay is NOT the plaintext.
        let plain_slice = &data[i * chunk_size..((i + 1) * chunk_size).min(data.len())];
        assert_ne!(ct.as_slice(), plain_slice, "chunk {i} must be ciphertext");
        assert_eq!(ct.len(), plain_slice.len() + 16, "AEAD tag adds 16 bytes");

        let plain = open_chunk(&key, i as u32, n as u32, &ct).expect("decrypt chunk");
        reassembled.extend_from_slice(&plain);
    }
    assert_eq!(reassembled, data, "decrypted file must match the original");

    receiver.close().await;
    sender.shutdown().await;
}
