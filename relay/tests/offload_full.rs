//! What a **full** relay does to an offload — and, more to the point, what it must
//! not do to the send.
//!
//! A relay with no room is a condition somebody else can produce: it is shared, and
//! filling it is what using it looks like. So a full relay must degrade to "we
//! could not park it", leaving the sender serving and the mailbox still available.
//! If it could instead put a send into the state that says "already parked, the
//! mailbox is off", then filling the relay would be a way to strand other people's
//! transfers.
//!
//! Own test binary: the seed row cap is a process-global env var.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::flow::{self, SendEvent};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_relay_refuses_the_park_without_stranding_the_send() {
    // No room for a single seeded chunk: every seed request is refused outright.
    std::env::set_var("ARVOLO_MAX_SEEDED_ROWS", "0");
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data: Vec<u8> = (0..20 * 1024 * 1024u64)
        .map(|i| (i * 131 + 7) as u8)
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

    let said_so = Arc::new(AtomicBool::new(false));
    let claimed_done = Arc::new(AtomicBool::new(false));
    let (s, d) = (said_so.clone(), claimed_done.clone());
    let parked = session
        .park_on_relay(&CancellationToken::new(), move |ev| match ev {
            SendEvent::BackfillFailed { reason } => {
                assert!(
                    reason.contains("no room"),
                    "a full relay should be reported as full, not as a bare rejection: {reason}"
                );
                s.store(true, Ordering::Relaxed);
            }
            // The event the caller turns into "mailbox off, already parked". A relay
            // that could not take the file must never produce it.
            SendEvent::Offloaded => d.store(true, Ordering::Relaxed),
            _ => {}
        })
        .await;

    assert!(!parked, "nothing was stored, so nothing is parked");
    assert!(
        said_so.load(Ordering::Relaxed),
        "the refusal has to reach the user — this was the silent case"
    );
    assert!(
        !claimed_done.load(Ordering::Relaxed),
        "a full relay must not be able to switch off somebody else's fallback"
    );

    session.shutdown().await;
}
