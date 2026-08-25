//! What the relay's copy is actually worth, end to end.
//!
//! Two things are being proved here, and they used to be false together:
//!
//! 1. A receiver can finish from the relay **with the sender gone**. Before, the
//!    set of backfilled pieces reached it only over the control channel — from the
//!    sender — so a resume with the origin down had no provider for any piece and
//!    backed off against a relay that was holding the whole file. Discovery
//!    (`/v1/have`) is what closes that, and this test would fail without it.
//!
//! 2. A sender that has handed the remainder over is **entitled to stop**. Before,
//!    `serve` had no way out but a cancel, so a send whose receiver dropped went on
//!    serving an empty room for ever — and with the loop stuck inside it, neither
//!    the wake-on-accept nor the seven-day give-up could run.
//!
//! Own test binary: it pins `ARVOLO_CONCURRENCY` so a receiver can be stopped
//! part-way deterministically, and env is process-global.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::flow::{self, ChunkSource, RecvCancel, RecvEvent, SendEvent};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

/// One chunk at a time, so "stop after the first piece" leaves a known tail
/// instead of a race. Set, never removed: these tests share one process.
fn serial_fetch() {
    std::env::set_var("ARVOLO_CONCURRENCY", "1");
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

/// 40 MiB — three 16 MiB chunks, the last one short.
fn payload() -> Vec<u8> {
    (0..40 * 1024 * 1024u64)
        .map(|i| (i * 131 + 7) as u8)
        .collect()
}

/// The whole point of parking: the sender puts it there and leaves, and the
/// recipient still gets the file — finding the relay's copy by asking, because
/// there is nobody left to tell them about it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_receiver_finishes_from_the_relay_with_the_sender_gone() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data = payload();
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
    assert!(session.has_relay, "the ticket must carry the relay");
    let ticket = session.ticket.clone();

    let parked = session
        .park_on_relay(flow::PARK_TTL_SECS, &CancellationToken::new(), |_| {})
        .await
        .is_some();
    assert!(parked, "the relay should have taken the whole payload");

    // The sender goes away for good. From here the relay is the only source.
    session.shutdown().await;

    let out = dir.path().join("got.bin");
    let sources = Arc::new(Mutex::new(Vec::new()));
    let s = sources.clone();
    let outcome = tokio::time::timeout(
        Duration::from_secs(120),
        flow::recv_chunked(
            &ticket,
            Some(out.clone()),
            None,
            RelayChoice::Disabled,
            CancellationToken::new(),
            move |e| {
                if let RecvEvent::Chunk { source, .. } = e {
                    s.lock().unwrap().push(source);
                }
            },
        ),
    )
    .await
    .expect("receiving from the relay alone should not hang")
    .expect("receive");

    assert!(outcome.is_complete(), "the download must finish");
    assert_eq!(
        std::fs::read(outcome.path()).unwrap(),
        data,
        "byte-for-byte, from the relay"
    );
    let sources = sources.lock().unwrap();
    assert_eq!(sources.len(), 3);
    assert!(
        sources.iter().all(|s| matches!(s, ChunkSource::Relay)),
        "with the origin gone every piece has to come from the relay: {sources:?}"
    );
}

/// A receiver that starts and drops leaves a tail. Once that tail is on the relay
/// there is nothing left for a 1:1 sender to do, so `serve` returns by itself —
/// and says so, which is what keeps the caller from depositing the same file a
/// second time.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_drop_ends_the_send_once_the_tail_is_on_the_relay() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data = payload();
    std::fs::write(&src, &data).unwrap();

    let sender = Identity::generate();
    let recipient = Identity::generate();
    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        Some((&sender, &recipient.public())),
        Some(relay.clone()),
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send");
    let ticket = session.ticket.clone();

    let offloaded = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicUsize::new(0));
    let (off, drp) = (offloaded.clone(), dropped.clone());
    // Never cancelled: the point is that `serve` ends without being told to.
    let serve_cancel = CancellationToken::new();
    let serving = tokio::spawn(async move {
        session
            .serve(serve_cancel, move |ev| match ev {
                SendEvent::Offloaded => off.store(true, Ordering::Relaxed),
                SendEvent::ReceiverDropped { missing } => {
                    drp.store(missing, Ordering::Relaxed);
                }
                _ => {}
            })
            .await
    });

    // A receiver that takes one piece and then vanishes *without* saying it meant
    // to (`abort: false`) — a closed lid, not a refusal. A deliberate cancel would
    // suppress the backfill by design.
    let recv_cancel = CancellationToken::new();
    let stop = recv_cancel.clone();
    let out: PathBuf = dir.path().join("partial.bin");
    let receiving = tokio::spawn(async move {
        let _ = flow::recv_chunked(
            &ticket,
            Some(out),
            Some(&recipient),
            RelayChoice::Disabled,
            RecvCancel {
                token: recv_cancel,
                abort: Arc::new(AtomicBool::new(false)),
            },
            move |e| {
                if matches!(e, RecvEvent::Chunk { .. }) {
                    stop.cancel();
                }
            },
        )
        .await;
    });

    let ended = tokio::time::timeout(Duration::from_secs(120), serving)
        .await
        .expect("serve must end on its own once the relay holds the tail")
        .expect("serve task");
    assert!(ended.is_ok(), "serve returned an error: {ended:?}");
    assert!(
        offloaded.load(Ordering::Relaxed),
        "the send ended, but not because it had handed over — that distinction is \
         what stops the caller depositing the file a second time"
    );
    assert!(
        dropped.load(Ordering::Relaxed) > 0,
        "the test did not actually leave a tail"
    );
    let _ = tokio::time::timeout(Duration::from_secs(30), receiving).await;
}
