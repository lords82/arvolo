//! Tests for the robust `send --to` delivery loop (`deliver_to`): when the relay
//! can't take a file, the send is *held* (`Waiting`) and keeps trying, rather than
//! failing. Drives the real `TransferManager` against a real (or deliberately
//! dead) relay.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::manager::{TransferManager, TransferStatus};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

async fn spawn_relay() -> (String, tempfile::TempDir) {
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
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), dir)
}

fn tmpfile(bytes: &[u8]) -> tempfile::NamedTempFile {
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), bytes).unwrap();
    f
}

/// Poll the manager until transfer `id` reaches a status matching `pred`.
async fn wait_status<F: Fn(&TransferStatus) -> bool>(
    m: &TransferManager,
    id: u64,
    pred: F,
) -> TransferStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(t) = m.list().into_iter().find(|t| t.id == id) {
            if pred(&t.status) {
                return t.status;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for status");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn held_when_relay_unavailable() {
    // A relay that isn't listening: the deposit fails (connection refused) and the
    // recipient can't be seen online — the send must be held, not failed.
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(
        sender,
        Some("http://127.0.0.1:1".into()),
        dl.path().to_path_buf(),
    );

    let src = tmpfile(b"hello world");
    let id = m
        .send_to(
            &recipient.public(),
            src.path().to_path_buf(),
            "f.bin".into(),
            false,
            String::new(),
        )
        .await
        .unwrap();

    let status = wait_status(&m, id, |s| matches!(s, TransferStatus::Waiting(_))).await;
    assert!(
        matches!(status, TransferStatus::Waiting(_)),
        "unreachable relay must hold the send, got {status:?}"
    );
    m.cancel(id);
}

#[tokio::test]
async fn held_when_file_too_large() {
    // Tiny per-file cap so any real deposit is refused (413). This is the only test
    // in this binary that reads the cap, so the process-global env is safe here.
    std::env::set_var("ARVOLO_MAX_BLOB_BYTES", "64");
    let (relay, _dir) = spawn_relay().await;

    let sender = Identity::generate();
    let recipient = Identity::generate(); // offline: never posts a presence beacon
    let dl = tempfile::tempdir().unwrap();
    let m = TransferManager::new(sender, Some(relay), dl.path().to_path_buf());

    let src = tmpfile(&vec![0u8; 4096]); // > 64 bytes → relay 413 → TooLarge
    let id = m
        .send_to(
            &recipient.public(),
            src.path().to_path_buf(),
            "big.bin".into(),
            false,
            String::new(),
        )
        .await
        .unwrap();

    let status = wait_status(&m, id, |s| matches!(s, TransferStatus::Waiting(_))).await;
    match &status {
        TransferStatus::Waiting(reason) => assert!(
            reason.contains("too large"),
            "a 413 should surface a 'too large' hold reason, got: {reason}"
        ),
        other => panic!("expected Waiting(too large), got {other:?}"),
    }
    m.cancel(id);
    std::env::remove_var("ARVOLO_MAX_BLOB_BYTES");
}

#[tokio::test]
async fn pause_then_resume() {
    let sender = Identity::generate();
    let recipient = Identity::generate();
    let dl = tempfile::tempdir().unwrap();
    // Dead relay: the send lands in Waiting, where we can pause it.
    let m = TransferManager::new(
        sender,
        Some("http://127.0.0.1:1".into()),
        dl.path().to_path_buf(),
    );
    let src = tmpfile(b"hi");
    let id = m
        .send_to(
            &recipient.public(),
            src.path().to_path_buf(),
            "f".into(),
            false,
            String::new(),
        )
        .await
        .unwrap();

    wait_status(&m, id, |s| matches!(s, TransferStatus::Waiting(_))).await;

    assert!(m.pause(id), "a held send should be pausable");
    let st = wait_status(&m, id, |s| matches!(s, TransferStatus::Paused(_))).await;
    assert!(matches!(st, TransferStatus::Paused(_)), "got {st:?}");

    assert!(m.resume(id), "a paused send should resume");
    let st = wait_status(&m, id, |s| {
        matches!(s, TransferStatus::Active | TransferStatus::Waiting(_))
    })
    .await;
    assert!(
        matches!(st, TransferStatus::Active | TransferStatus::Waiting(_)),
        "resumed send should be trying again, got {st:?}"
    );
    m.cancel(id);
}

#[tokio::test]
async fn paused_send_survives_restart() {
    let me_bytes = Identity::generate().secret_bytes();
    let recipient = Identity::generate();
    let dl = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let payload = work.path().join("f.bin");
    std::fs::write(&payload, b"hello").unwrap();
    let relay = "http://127.0.0.1:1".to_string();

    // First daemon: send to an offline recipient, wait until held, pause it.
    {
        let m = TransferManager::with_state_dir(
            Identity::from_secret_bytes(&me_bytes).unwrap(),
            Some(relay.clone()),
            dl.path().to_path_buf(),
            Some(state.path().to_path_buf()),
        );
        let id = m
            .send_to(
                &recipient.public(),
                payload.clone(),
                "f.bin".into(),
                false,
                String::new(),
            )
            .await
            .unwrap();
        wait_status(&m, id, |s| matches!(s, TransferStatus::Waiting(_))).await;
        assert!(m.pause(id));
        wait_status(&m, id, |s| matches!(s, TransferStatus::Paused(_))).await;
        // `m` drops here — the daemon "shut down" with the send paused on disk.
    }

    // Second daemon: same identity + state dir → the paused send is restored.
    let m2 = TransferManager::with_state_dir(
        Identity::from_secret_bytes(&me_bytes).unwrap(),
        Some(relay),
        dl.path().to_path_buf(),
        Some(state.path().to_path_buf()),
    );
    let restored = m2.resume_incomplete();
    assert!(
        restored >= 1,
        "the paused send should be restored, got {restored}"
    );
    assert!(
        m2.list()
            .into_iter()
            .any(|t| matches!(t.status, TransferStatus::Paused(_))),
        "the send must come back paused after a restart"
    );
}
