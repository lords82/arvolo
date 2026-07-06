//! End-to-end proof of Fase 4: two devices that share one identity co-swarm a
//! **sealed `--to` transfer**. We force a peer-only path: device A downloads the
//! whole file and keeps seeding, the original sender is then shut down, and the
//! relay never backfilled — so device B's only possible source is peer A, and
//! `pieces_from_peers > 0` proves the pieces crossed device-to-device.
//!
//! Completion is detected from the output *file* (not by awaiting `recv_chunked`
//! to return), because an iroh endpoint's teardown can block after a transfer
//! finishes; the receiver tasks are aborted once their output is complete.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::flow::{self, RecvEvent};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

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

fn committed(events: &Mutex<Vec<RecvEvent>>) -> usize {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, RecvEvent::Chunk { .. }))
        .count()
}

/// Poll until `path` holds exactly `data`, or the deadline passes.
async fn wait_for_file(path: &std::path::Path, data: &[u8], within: Duration) -> bool {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_devices_sharing_identity_co_swarm_a_sealed_transfer() {
    // Completed peers keep seeding long enough for device B to pull from them.
    std::env::set_var("ARVOLO_SEED_AFTER", "120");

    let relay = spawn_relay().await;

    // One shared identity across two devices; a distinct sender.
    let account = Identity::generate();
    let dev_a = Identity::from_secret_bytes(&account.secret_bytes()).unwrap();
    let dev_b = Identity::from_secret_bytes(&account.secret_bytes()).unwrap();
    let sender = Identity::generate();

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    // ~40 MiB -> 3 chunks, so a peer-sourced transfer is clearly observable.
    let data: Vec<u8> = (0..40 * 1024 * 1024u64)
        .map(|i| (i * 131 + 7) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    // Sender seals the transfer to the shared identity, relay embedded (swarm on).
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
    assert_eq!(session.chunks, 3);
    assert!(
        session.has_relay,
        "ticket must carry the relay for swarming"
    );
    let ticket = session.ticket.clone();

    let send_cancel = CancellationToken::new();
    let serve = {
        let c = send_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };

    // Device A downloads fully, then keeps seeding (SEED_AFTER). Run detached and
    // detect completion from its output file.
    let out_a = dir.path().join("a.out");
    let events_a = Arc::new(Mutex::new(Vec::new()));
    let recv_a = {
        let (ticket, out_a, dev_a, ev) = (
            ticket.clone(),
            out_a.clone(),
            Identity::from_secret_bytes(&dev_a.secret_bytes()).unwrap(),
            events_a.clone(),
        );
        tokio::spawn(async move {
            let _ = flow::recv_chunked(
                &ticket,
                Some(out_a),
                Some(&dev_a),
                RelayChoice::Disabled,
                CancellationToken::new(),
                move |e| ev.lock().unwrap().push(e),
            )
            .await;
        })
    };
    assert!(
        wait_for_file(&out_a, &data, Duration::from_secs(60)).await,
        "device A never completed (committed {}/3)",
        committed(&events_a)
    );

    // Remove the ORIGINAL sender: now the only holder of the pieces is device A
    // (the relay never backfilled — A completed without ever dropping).
    send_cancel.cancel();
    let _ = serve.await;

    // Device B downloads: its only possible source is peer A over the swarm.
    let out_b = dir.path().join("b.out");
    let events_b = Arc::new(Mutex::new(Vec::new()));
    let recv_b = {
        let (ticket, out_b, ev) = (ticket.clone(), out_b.clone(), events_b.clone());
        tokio::spawn(async move {
            let _ = flow::recv_chunked(
                &ticket,
                Some(out_b),
                Some(&dev_b),
                RelayChoice::Disabled,
                CancellationToken::new(),
                move |e| ev.lock().unwrap().push(e),
            )
            .await;
        })
    };

    let ok = wait_for_file(&out_b, &data, Duration::from_secs(90)).await;

    recv_a.abort();
    recv_b.abort();
    std::env::remove_var("ARVOLO_SEED_AFTER");

    assert!(
        ok,
        "device B never completed from its peer (committed {}/3, from_peers={})",
        committed(&events_b),
        max_from_peers(&events_b)
    );
    assert!(
        max_from_peers(&events_b) > 0,
        "device B must have pulled at least one piece from device A over the swarm \
         (committed {}/3, from_peers=0)",
        committed(&events_b)
    );
}
