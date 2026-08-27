//! Fallback providers are a race with a head start, not a queue.
//!
//! Tried strictly in order, a chunk whose first provider was dead paid that
//! provider's full open bound before the second was even dialled — the worst case
//! grew linearly with the number of sources, all of it spent knowingly waiting on
//! timeouts. Hedged, a further provider joins the race after a few seconds of
//! stagger, and the first verified chunk wins.
//!
//! Own test binary: it overrides the process-global open bound, which cannot be
//! shared with tests that expect the real one.

use std::time::{Duration, Instant};

use arvolo_core::chunked::{ChunkReceiver, ChunkSender};
use arvolo_core::hash::Hash;
use arvolo_core::transfer::{bind_endpoint, RelayChoice};

#[tokio::test]
async fn a_dead_first_provider_does_not_queue_the_live_second() {
    // 8s to reach a provider: short enough to test against, long enough that the
    // hedge (5s) fires well before it.
    std::env::set_var("ARVOLO_CHUNK_OPEN_SECS", "8");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    let data: Vec<u8> = (0..32 * 1024).map(|i| (i * 11 + 3) as u8).collect();
    std::fs::write(&path, &data).unwrap();
    let sender = ChunkSender::serve(&path, RelayChoice::Disabled)
        .await
        .expect("sender");
    let hash = sender.chunks()[0];

    // A provider that is valid on paper and dead on the wire: a real endpoint id
    // (so nothing rejects it early) whose only address blackholes (TEST-NET-1,
    // packets vanish without answer). Connecting to it can only run out the open
    // bound — which is the point: dead here means slow-dead, the expensive kind.
    let ghost = bind_endpoint(RelayChoice::Disabled)
        .await
        .expect("ghost endpoint");
    let mut dead = iroh::EndpointAddr::new(ghost.id());
    dead.addrs
        .insert(iroh::TransportAddr::Ip("192.0.2.1:9".parse().unwrap()));
    ghost.close().await;

    let receiver = ChunkReceiver::open(RelayChoice::Disabled)
        .await
        .expect("receiver");
    let started = Instant::now();
    let ct = receiver
        .fetch_chunk(&[dead, sender.addr()], hash)
        .await
        .expect("the live provider should win the race");
    let waited = started.elapsed();

    assert_eq!(Hash::new(&ct), hash, "the winner's chunk is the right one");
    // Under the hedge the live provider starts at ~5s and answers at once. Queued
    // behind the dead one it could not start before that one's 8s bound ran out.
    assert!(
        waited < Duration::from_secs(8),
        "took {waited:?}: the live provider queued behind the dead one's timeout"
    );
    // And the stagger is real — a pure stampede would win in milliseconds and
    // dial every provider for every chunk.
    assert!(
        waited >= Duration::from_secs(4),
        "took only {waited:?}: every provider was dialled at once, no head start"
    );
    std::env::remove_var("ARVOLO_CHUNK_OPEN_SECS");
}
