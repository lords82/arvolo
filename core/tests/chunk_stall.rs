//! Neither end of a chunk transfer may wait for ever.
//!
//! A QUIC connection whose peer answers at the transport level stays open no matter
//! how little it says, so "the connection is up" is not evidence that anything is
//! moving. Both sides had an await with no bound on the other side saying something,
//! and both turned a peer that went quiet into a task parked for good — the receiver
//! stalled at 97.4% of a 10.7 GiB file for hours, the sender counting a connected
//! downloader the whole time.
//!
//! Own test binary: it sets the process-global stall bound, which cannot be shared
//! with tests that expect the real one.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use arvolo_core::chunked::{ChunkSender, CHUNK_ALPN};
use arvolo_core::transfer::{bind_endpoint, RelayChoice};

/// The sender must hang up on a receiver that opens a stream and then asks for
/// nothing — otherwise that connection serves nobody, for ever, while still being
/// counted as an active download.
#[tokio::test]
async fn a_receiver_that_asks_nothing_does_not_park_the_sender() {
    std::env::set_var("ARVOLO_CHUNK_STALL_SECS", "1");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    std::fs::write(&path, vec![7u8; 64 * 1024]).unwrap();
    let sender = ChunkSender::serve(&path, RelayChoice::Disabled)
        .await
        .expect("sender");

    let ep = bind_endpoint(RelayChoice::Disabled)
        .await
        .expect("endpoint");
    let conn = ep
        .connect(sender.addr(), CHUNK_ALPN)
        .await
        .expect("connect to the sender");
    // Open the stream the sender is waiting on, and say nothing at all. Keep both
    // halves alive so nothing is closed from this side: the point is a peer that is
    // present and silent, not one that left.
    let held = conn.open_bi().await.expect("open a request stream");

    let started = Instant::now();
    let closed = tokio::time::timeout(Duration::from_secs(20), conn.closed()).await;
    let waited = started.elapsed();
    drop(held);

    assert!(
        closed.is_ok(),
        "the sender kept a silent stream open for {waited:?}: it is parked, and the \
         peer count with it"
    );
    std::env::remove_var("ARVOLO_CHUNK_STALL_SECS");
}

/// A connection that has served a request gets the *idle* bound, not the stall
/// bound: it has shown a purpose, and hanging up on it with the short clock would
/// charge every quiet-but-honest peer a reconnect it did nothing to deserve.
///
/// The request frame is encoded by hand, deliberately. The wire format under
/// `arvolo/chunk/1` — 4-byte LE length, then postcard (`Hash` as 32 raw bytes,
/// offset as a varint) — is a compatibility surface between versions: a test that
/// pins it byte-for-byte catches an accidental format change that would break
/// old↔new transfers, which a helper calling the very code under test never could.
#[tokio::test]
async fn a_connection_that_served_once_gets_the_idle_bound_not_the_stall_bound() {
    std::env::set_var("ARVOLO_CHUNK_IDLE_SECS", "3");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    std::fs::write(&path, vec![9u8; 8 * 1024]).unwrap();
    let sender = ChunkSender::serve(&path, RelayChoice::Disabled)
        .await
        .expect("sender");
    let hash = sender.chunks()[0];

    let ep = bind_endpoint(RelayChoice::Disabled)
        .await
        .expect("endpoint");
    let conn = ep
        .connect(sender.addr(), CHUNK_ALPN)
        .await
        .expect("connect to the sender");
    let (mut send, mut recv) = conn.open_bi().await.expect("open a request stream");
    // ChunkReq { hash, offset: 0 }, framed: length 33, 32 raw hash bytes, varint 0.
    let mut frame = 33u32.to_le_bytes().to_vec();
    frame.extend_from_slice(hash.as_bytes());
    frame.push(0x00);
    send.write_all(&frame).await.expect("write the request");
    send.finish().expect("finish the request");
    // Read the whole response: proves the request was parsed and served, which is
    // what flips the loop from the stall clock to the idle clock.
    let body = recv
        .read_to_end(128 * 1024)
        .await
        .expect("read the chunk response");
    assert!(body.len() > 8 * 1024, "the chunk body should have arrived");

    // Then say nothing more, holding the connection. The 3s override must be what
    // ends it: at least ~3s (nothing shorter fired), well under 20s (neither the
    // 30s stall default nor the 300s idle default was in charge).
    let served_at = Instant::now();
    let closed = tokio::time::timeout(Duration::from_secs(20), conn.closed()).await;
    let waited = served_at.elapsed();

    assert!(
        closed.is_ok(),
        "a served connection was kept open for {waited:?}: the idle bound never fired"
    );
    assert!(
        waited >= Duration::from_secs(2),
        "closed after only {waited:?}: something shorter than the idle bound fired"
    );
    std::env::remove_var("ARVOLO_CHUNK_IDLE_SECS");
}

/// And the ordinary exchange must be untouched by the bound: a receiver that does
/// ask gets its chunk. Guards against "fixing" the hang by hanging up on everybody.
#[tokio::test]
async fn a_receiver_that_asks_properly_still_gets_its_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    let data: Vec<u8> = (0..96 * 1024).map(|i| (i * 13 + 5) as u8).collect();
    std::fs::write(&path, &data).unwrap();
    let sender = ChunkSender::serve(&path, RelayChoice::Disabled)
        .await
        .expect("sender");
    let hash = sender.chunks()[0];

    let receiver = arvolo_core::chunked::ChunkReceiver::open(RelayChoice::Disabled)
        .await
        .expect("receiver");
    let out = dir.path().join("chunk.bin");
    let mut f = std::fs::File::create(&out).expect("stage file");
    let banned = Mutex::new(std::collections::HashSet::new());
    receiver
        .fetch_one(&sender.addr(), hash, &mut f, &banned)
        .await
        .expect("a receiver that asks gets served");
    drop(f);

    assert_eq!(
        arvolo_core::hash::Hash::new(std::fs::read(&out).unwrap()),
        hash,
        "the chunk that came back is the one that was asked for"
    );
}
