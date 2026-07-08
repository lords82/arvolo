//! End-to-end proof of the disjoint-piece swarm: two peers start with
//! **complementary** halves of a file (A the even chunks, B the odd ones), the
//! origin is gone, and they exchange the pieces each other is missing until both
//! are complete. This is impossible with the old prefix-only swarm — a device only
//! ever held a contiguous prefix, so two devices had nested (never disjoint) sets.
//!
//! Determinism: we hand-build each peer's partial output + resume sidecar, so the
//! initial piece split is fixed. Completion is detected from the output file (not by
//! awaiting `recv_chunked`, whose endpoint teardown can block); tasks are aborted
//! once their output is complete.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::flow::{self, RecvEvent};
use arvolo_core::swarm::{bitfield_new, bitfield_set};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

const CHUNK: usize = 16 * 1024 * 1024;

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

/// Lay down `present` chunks of `data` at their true offsets (rest left as a hole),
/// plus a resume sidecar marking exactly those pieces.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_peers_with_complementary_pieces_complete_each_other() {
    std::env::set_var("ARVOLO_SEED_AFTER", "120");
    let relay = spawn_relay().await;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    // 64 MiB -> exactly 4 chunks (0..=3), so an even/odd split is disjoint.
    let data: Vec<u8> = (0..64 * 1024 * 1024u64)
        .map(|i| (i * 179 + 13) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    // A shared (Plain) ticket; embed the relay to enable the swarm tracker.
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
    // The origin is not needed: shut it down so the only sources are the two peers.
    let send_cancel = CancellationToken::new();
    let serve = {
        let c = send_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };
    send_cancel.cancel();
    let _ = serve.await;

    // Complementary starting states: A has {0,2}, B has {1,3}.
    let out_a = dir.path().join("a.out");
    let out_b = dir.path().join("b.out");
    seed_partial(&out_a, &data, 4, &[0, 2]);
    seed_partial(&out_b, &data, 4, &[1, 3]);

    let spawn_peer = |out: PathBuf| {
        let ticket = ticket.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let handle = tokio::spawn(async move {
            let _ = flow::recv_chunked(
                &ticket,
                Some(out),
                None,
                RelayChoice::Disabled,
                CancellationToken::new(),
                move |e| ev.lock().unwrap().push(e),
            )
            .await;
        });
        (handle, events)
    };
    let (recv_a, events_a) = spawn_peer(out_a.clone());
    let (recv_b, events_b) = spawn_peer(out_b.clone());

    let ok_a = wait_for_file(&out_a, &data, Duration::from_secs(90)).await;
    let ok_b = wait_for_file(&out_b, &data, Duration::from_secs(90)).await;

    recv_a.abort();
    recv_b.abort();
    std::env::remove_var("ARVOLO_SEED_AFTER");

    assert!(
        ok_a,
        "device A did not complete from its peer (from_peers={})",
        max_from_peers(&events_a)
    );
    assert!(
        ok_b,
        "device B did not complete from its peer (from_peers={})",
        max_from_peers(&events_b)
    );
    // Each pulled its two missing pieces from the other — the disjoint exchange.
    assert!(
        max_from_peers(&events_a) > 0,
        "device A must have pulled its missing pieces from device B"
    );
    assert!(
        max_from_peers(&events_b) > 0,
        "device B must have pulled its missing pieces from device A"
    );
}

fn spawn_peer(
    ticket: String,
    out: PathBuf,
) -> (tokio::task::JoinHandle<()>, Arc<Mutex<Vec<RecvEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let handle = tokio::spawn(async move {
        let _ = flow::recv_chunked(
            &ticket,
            Some(out),
            None,
            RelayChoice::Disabled,
            CancellationToken::new(),
            move |e| ev.lock().unwrap().push(e),
        )
        .await;
    });
    (handle, events)
}

// Harder: THREE peers, each holding a distinct third of the file, complete the
// whole thing purely by swapping pieces among themselves (no origin).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn three_peers_each_with_one_third_complete_the_swarm() {
    std::env::set_var("ARVOLO_SEED_AFTER", "180");
    let relay = spawn_relay().await;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    // 48 MiB -> exactly 3 chunks, one per peer.
    let data: Vec<u8> = (0..48 * 1024 * 1024u64)
        .map(|i| (i * 211 + 17) as u8)
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
    assert_eq!(session.chunks, 3);
    let ticket = session.ticket.clone();
    let send_cancel = CancellationToken::new();
    let serve = {
        let c = send_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };
    send_cancel.cancel();
    let _ = serve.await;

    // Each peer starts with a different single chunk.
    let outs: Vec<PathBuf> = (0..3)
        .map(|p| dir.path().join(format!("p{p}.out")))
        .collect();
    for (p, out) in outs.iter().enumerate() {
        seed_partial(out, &data, 3, &[p]);
    }

    let peers: Vec<_> = outs
        .iter()
        .map(|out| spawn_peer(ticket.clone(), out.clone()))
        .collect();

    let mut all_ok = true;
    for out in &outs {
        all_ok &= wait_for_file(out, &data, Duration::from_secs(120)).await;
    }

    let mut from_peers = Vec::new();
    for (handle, events) in &peers {
        from_peers.push(max_from_peers(events));
        handle.abort();
    }
    std::env::remove_var("ARVOLO_SEED_AFTER");

    assert!(
        all_ok,
        "every peer completed the file (from_peers={from_peers:?})"
    );
    for (p, fp) in from_peers.iter().enumerate() {
        assert!(
            *fp > 0,
            "peer {p} must have pulled its two missing thirds from the others"
        );
    }
}

// The two features compose: a `--to` **sealed** transfer to a shared identity can
// be co-swarmed with DISJOINT pieces. Two devices sharing one identity unseal the
// same content key, so their pieces are interchangeable; starting with
// complementary halves and no origin, they complete each other.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn disjoint_exchange_works_on_a_sealed_to_transfer() {
    std::env::set_var("ARVOLO_SEED_AFTER", "120");
    let relay = spawn_relay().await;

    let account = Identity::generate();
    let dev_a = Identity::from_secret_bytes(&account.secret_bytes()).unwrap();
    let dev_b = Identity::from_secret_bytes(&account.secret_bytes()).unwrap();
    let sender = Identity::generate();

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data: Vec<u8> = (0..64 * 1024 * 1024u64)
        .map(|i| (i * 179 + 13) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    // Sealed to the shared identity, relay embedded (swarm on).
    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        Some((&sender, &account.public())),
        Some(relay.clone()),
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send");
    assert_eq!(session.chunks, 4);
    let ticket = session.ticket.clone();
    let send_cancel = CancellationToken::new();
    let serve = {
        let c = send_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };
    send_cancel.cancel();
    let _ = serve.await;

    let out_a = dir.path().join("a.out");
    let out_b = dir.path().join("b.out");
    seed_partial(&out_a, &data, 4, &[0, 2]);
    seed_partial(&out_b, &data, 4, &[1, 3]);

    let spawn = |out: PathBuf, id: Identity| {
        let ticket = ticket.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let handle = tokio::spawn(async move {
            let _ = flow::recv_chunked(
                &ticket,
                Some(out),
                Some(&id),
                RelayChoice::Disabled,
                CancellationToken::new(),
                move |e| ev.lock().unwrap().push(e),
            )
            .await;
        });
        (handle, events)
    };
    let (recv_a, events_a) = spawn(out_a.clone(), dev_a);
    let (recv_b, events_b) = spawn(out_b.clone(), dev_b);

    let ok_a = wait_for_file(&out_a, &data, Duration::from_secs(90)).await;
    let ok_b = wait_for_file(&out_b, &data, Duration::from_secs(90)).await;

    recv_a.abort();
    recv_b.abort();
    std::env::remove_var("ARVOLO_SEED_AFTER");

    assert!(
        ok_a,
        "sealed device A completed (from_peers={})",
        max_from_peers(&events_a)
    );
    assert!(
        ok_b,
        "sealed device B completed (from_peers={})",
        max_from_peers(&events_b)
    );
    assert!(max_from_peers(&events_a) > 0 && max_from_peers(&events_b) > 0);
}
