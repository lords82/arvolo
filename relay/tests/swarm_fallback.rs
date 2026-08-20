//! End-to-end proof of the swarm's fallback guarantee — the scenario in the
//! question: A is the origin, B and C both download the shared ticket. B holds
//! some pieces and then **leaves**; C must still finish, pulling whatever B had
//! straight from A. The origin holds the whole file, so serving a piece to B never
//! removes it from A — C can always fall back to the origin (or the relay) for any
//! piece a departed peer used to serve.
//!
//! Determinism: A stays alive for the whole test as the always-available source, so
//! completion never depends on B being up at a particular instant. B is a complete
//! seeder that we cut off mid-swarm; the assertion is simply that C completes the
//! file byte-for-byte anyway. Completion is read from the output file (not by
//! awaiting `recv_chunked`, whose endpoint teardown can block), as in
//! `disjoint_swarm.rs`.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arvolo_core::backfill::BlobNode;
use arvolo_core::flow::{self, RecvEvent};
use arvolo_core::swarm::{bitfield_new, bitfield_set};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

const CHUNK: usize = 16 * 1024 * 1024;

/// Every test sets the SAME seeding env up front and never removes it: these
/// tests run in parallel in one process, so a `remove_var` in one strips the
/// variable from another mid-run and changes its seeding behaviour.
fn seed_env() {
    std::env::set_var("ARVOLO_SEED_AFTER", "120");
}

/// The origin's `serve()` ends with an iroh `Endpoint::close().await`, which
/// waits for peer connections to say goodbye — and this suite kills peers with
/// `abort()` mid-transfer, so a wedged close would otherwise hang the test
/// forever instead of failing it. Bound the wait; the assertions that follow
/// are the substance of the test either way.
async fn reap_origin(origin: tokio::task::JoinHandle<anyhow::Result<()>>) {
    if tokio::time::timeout(Duration::from_secs(30), origin)
        .await
        .is_err()
    {
        eprintln!("origin teardown did not finish within 30s; proceeding");
    }
}

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

fn max_from_peers(events: &Mutex<Vec<RecvEvent>>) -> u64 {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            RecvEvent::Swarm {
                pieces_from_peers, ..
            } => Some(*pieces_from_peers),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

async fn wait_for_file(path: &Path, data: &[u8], within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) == data.len() as u64
            && std::fs::read(path).map(|d| d == data).unwrap_or(false)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Lay down `present` chunks of `data` at their true offsets, plus a resume sidecar
/// marking exactly those pieces (so a peer starts already holding them).
fn seed_partial(out: &Path, data: &[u8], total: usize, present: &[usize]) {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(out)
        .unwrap();
    f.set_len(data.len() as u64).unwrap();
    for &i in present {
        let start = i * CHUNK;
        let end = (start + CHUNK).min(data.len());
        f.seek(SeekFrom::Start(start as u64)).unwrap();
        f.write_all(&data[start..end]).unwrap();
    }
    drop(f);
    let mut bf = bitfield_new(total);
    for &i in present {
        bitfield_set(&mut bf, i);
    }
    std::fs::write(PathBuf::from(format!("{}.arvhave", out.display())), &bf).unwrap();
}

/// Spawn a receiver for `ticket`, driven by `cancel` so the test can make it leave
/// the swarm cleanly (cancelling propagates to the swarm coordinator, which
/// deregisters from the tracker).
fn spawn_receiver(
    ticket: String,
    out: PathBuf,
    cancel: CancellationToken,
) -> (tokio::task::JoinHandle<()>, Arc<Mutex<Vec<RecvEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let handle = tokio::spawn(async move {
        let _ = flow::recv_chunked(
            &ticket,
            Some(out),
            None,
            RelayChoice::Disabled,
            cancel,
            move |e| ev.lock().unwrap().push(e),
        )
        .await;
    });
    (handle, events)
}

/// A peer (B) that holds part of the file leaves mid-swarm; the other receiver (C)
/// still completes, falling back to the origin (A) for the pieces B had.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_receiver_completes_from_the_origin_after_a_peer_leaves() {
    // Keep a completed peer seeding so B is a real provider before we cut it off.
    seed_env();
    let relay = spawn_relay().await;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    // 64 MiB -> exactly 4 chunks.
    let data: Vec<u8> = (0..64 * 1024 * 1024u64)
        .map(|i| (i * 179 + 13) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    // Shared (Plain) ticket with the relay embedded → swarm tracker on.
    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.clone()),
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send");
    assert_eq!(session.chunks, 4);
    assert!(session.has_relay);
    let ticket = session.ticket.clone();

    // A = the origin. It stays ALIVE for the whole test: the always-available source
    // that serving a piece to B never depletes.
    let origin_cancel = CancellationToken::new();
    let origin = {
        let c = origin_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };

    // B = a complete peer (holds every piece) that seeds, then leaves.
    let out_b = dir.path().join("b.out");
    seed_partial(&out_b, &data, 4, &[0, 1, 2, 3]);
    let b_cancel = CancellationToken::new();
    let (recv_b, _events_b) = spawn_receiver(ticket.clone(), out_b.clone(), b_cancel.clone());

    // C = a fresh receiver that starts empty and must finish the whole file.
    let out_c = dir.path().join("c.out");
    let c_cancel = CancellationToken::new();
    let (recv_c, events_c) = spawn_receiver(ticket.clone(), out_c.clone(), c_cancel.clone());

    // Let C discover the swarm and get going, then B leaves (cleanly deregisters).
    tokio::time::sleep(Duration::from_secs(3)).await;
    b_cancel.cancel();
    recv_b.abort();

    // C must complete regardless — the origin still holds every piece B had.
    let ok_c = wait_for_file(&out_c, &data, Duration::from_secs(120)).await;

    origin_cancel.cancel();
    reap_origin(origin).await;
    recv_c.abort();

    assert!(
        ok_c,
        "C must complete from the origin after peer B left (from_peers={})",
        max_from_peers(&events_c)
    );
    assert_eq!(std::fs::read(&out_c).unwrap(), data, "C's file is intact");
}

/// The offload the user asked for: with the origin (A) *and* a complete peer (B)
/// both available, C fetches pieces B already holds from B rather than A, sparing
/// the source. Proven end-to-end by `pieces_from_peers > 0` while A stays up the
/// whole time (so those pieces were a deliberate choice, not a fallback).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_receiver_offloads_the_origin_by_pulling_from_a_peer() {
    seed_env();
    let relay = spawn_relay().await;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    // 128 MiB -> exactly 8 chunks: enough that C is still downloading when it
    // discovers B, so the peer-preference has pieces left to act on (a 4-chunk file
    // on loopback can finish from A before B is ever discovered).
    let data: Vec<u8> = (0..128 * 1024 * 1024u64)
        .map(|i| (i * 179 + 13) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.clone()),
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send");
    assert_eq!(session.chunks, 8);
    let ticket = session.ticket.clone();

    // A = the origin, alive for the whole test (the source we want to offload).
    let origin_cancel = CancellationToken::new();
    let origin = {
        let c = origin_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };

    // B = a complete peer that stays up and seeds. Give it a head start so it is
    // registered on the tracker before C starts choosing providers.
    let out_b = dir.path().join("b.out");
    seed_partial(&out_b, &data, 8, &[0, 1, 2, 3, 4, 5, 6, 7]);
    let b_cancel = CancellationToken::new();
    let (recv_b, _events_b) = spawn_receiver(ticket.clone(), out_b.clone(), b_cancel.clone());
    tokio::time::sleep(Duration::from_secs(3)).await;

    // C = empty; it should prefer B for the pieces B has.
    let out_c = dir.path().join("c.out");
    let c_cancel = CancellationToken::new();
    let (recv_c, events_c) = spawn_receiver(ticket.clone(), out_c.clone(), c_cancel.clone());

    let ok_c = wait_for_file(&out_c, &data, Duration::from_secs(120)).await;

    origin_cancel.cancel();
    reap_origin(origin).await;
    b_cancel.cancel();
    recv_b.abort();
    recv_c.abort();

    assert!(ok_c, "C must complete");
    assert_eq!(std::fs::read(&out_c).unwrap(), data, "C's file is intact");
    assert!(
        max_from_peers(&events_c) > 0,
        "C must have offloaded the origin by pulling at least one piece from peer B"
    );
}

/// The baseline the fallback rests on: with the swarm on but **no peers at all**, a
/// receiver completes from the origin alone. This proves a peer is never a
/// precondition — C could download the whole file from A even if B never existed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_lone_receiver_completes_from_the_origin_with_the_swarm_on() {
    // Not a seeding test, but the env is process-wide and the others set it —
    // set it here too so this test behaves the same regardless of scheduling.
    seed_env();
    let relay = spawn_relay().await;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    // ~24 MiB -> 2 chunks (16 + 8), last one short.
    let data: Vec<u8> = (0..24 * 1024 * 1024u64)
        .map(|i| (i * 17 + 3) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.clone()),
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send");
    assert!(session.has_relay);
    let ticket = session.ticket.clone();

    let origin_cancel = CancellationToken::new();
    let origin = {
        let c = origin_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };

    let out_c = dir.path().join("c.out");
    let c_cancel = CancellationToken::new();
    let (recv_c, events_c) = spawn_receiver(ticket.clone(), out_c.clone(), c_cancel.clone());

    let ok = wait_for_file(&out_c, &data, Duration::from_secs(90)).await;

    origin_cancel.cancel();
    reap_origin(origin).await;
    recv_c.abort();

    assert!(
        ok,
        "a lone receiver must complete from the origin (from_peers={})",
        max_from_peers(&events_c)
    );
    assert_eq!(std::fs::read(&out_c).unwrap(), data);
}
