//! A relay's answer on how long it will hold a parked send.
//!
//! Own test binary, and it has to be: `ARVOLO_MAX_TTL` is process-global, so a
//! clamp set here would silently shorten every other test's parked send — which
//! is exactly how this started life, as a sibling of `parked.rs` quietly making
//! its neighbour assert the wrong number.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::flow;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

async fn spawn_relay() -> (String, Arc<Mailbox>) {
    let dir = tempfile::tempdir().unwrap();
    let node = BlobNode::spawn(dir.path(), RelayChoice::Disabled)
        .await
        .expect("blob node");
    let mailbox = Arc::new(Mailbox::in_memory().expect("mailbox"));
    let state = AppState::new(mailbox.clone(), Arc::new(node));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _dir = dir;
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), mailbox)
}

async fn parked_session(relay: &str, dir: &std::path::Path) -> flow::SendSession {
    let src = dir.join("f.bin");
    std::fs::write(&src, vec![5u8; 2 * 1024 * 1024]).unwrap();
    flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.to_string()),
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send")
}

/// The TTL is a wish, not an instruction — exactly as a deposit's is. A relay that
/// will not hold anything for more than an hour must not be talked into a week,
/// and the caller has to be *told*, or it promises a deadline nobody agreed to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relay_clamps_the_lifetime_and_says_what_it_gave() {
    std::env::set_var("ARVOLO_MAX_TTL", "3600");
    let (relay, _mailbox) = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let session = parked_session(&relay, dir.path()).await;

    let granted = session
        .park_on_relay(flow::PARK_TTL_SECS, &CancellationToken::new(), |_| {})
        .await
        .expect("parking still succeeds — it is the lifetime that is cut, not the file");
    assert_eq!(
        granted, 3600,
        "the sender must learn the shorter life, not the one it asked for"
    );
    session.shutdown().await;
    std::env::remove_var("ARVOLO_MAX_TTL");
}
