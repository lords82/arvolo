//! The same window, without a daemon.
//!
//! On a desktop the GUI dials a socket and a second process answers. On a phone
//! there is no second process — the engine runs inside the app — and the promise
//! this crate exists to keep is that *nothing else changes*: the same typed
//! client, the same requests, the same answers, the same event stream.
//!
//! These tests hold the engine in-process and drive it through
//! `DaemonClient::embedded`, which is exactly what a mobile shell will do.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

use arvolo_engine::announce::Silent;
use arvolo_engine::start::{start, Options, Running};
use arvolo_ipc::client::DaemonClient;
use tokio_util::sync::CancellationToken;

/// A fresh installation in a temp dir, with an engine running inside this test.
///
/// The relay is a port nothing listens on: the inbox subscription retries in the
/// background and none of what is asserted here goes near it. What matters is
/// that `start` returns something that answers.
async fn engine(dir: &std::path::Path) -> (Running, CancellationToken) {
    std::env::set_var("ARVOLO_CONFIG_DIR", dir);
    std::env::remove_var("ARVOLO_IDENTITY");
    let shutdown = CancellationToken::new();
    let running = start(Options {
        download_dir: Some(dir.join("downloads")),
        relay: "http://127.0.0.1:1".into(),
        auto_sync: false,
        announce: Arc::new(Silent),
        shutdown: shutdown.clone(),
        publisher: None,
        front_ends: Arc::new(AtomicUsize::new(0)),
    })
    .await
    .expect("the engine starts");
    (running, shutdown)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_with_no_socket_answers_the_same_way() {
    let dir = tempfile::tempdir().unwrap();
    let (running, shutdown) = engine(dir.path()).await;

    let mut c = DaemonClient::embedded(Arc::new(running.daemon.clone()));
    assert!(!c.is_remote(), "nothing is being dialled");

    c.ping().await.expect("ping");

    let status = c.status().await.expect("status");
    assert_eq!(
        status.public_id, running.my_id,
        "the window is told about the identity the engine actually opened"
    );
    assert_eq!(
        status.download_dir,
        dir.path().join("downloads").display().to_string()
    );

    // Not just reads: a request that changes state, then one that sees it — the
    // same pair of calls a contacts screen makes.
    let stranger =
        arvolo_engine::identity::encode_id(&arvolo_core::crypto::Identity::generate().public());
    c.add_contact("giulia".into(), stranger)
        .await
        .expect("add a contact");
    let contacts = c.list_contacts().await.expect("list contacts");
    assert!(
        contacts.iter().any(|k| k.name == "giulia"),
        "what one request wrote, the next one reads"
    );

    running.stop();
    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_arrive_without_a_connection_to_carry_them() {
    let dir = tempfile::tempdir().unwrap();
    let (running, shutdown) = engine(dir.path()).await;

    let mut events = DaemonClient::embedded(Arc::new(running.daemon.clone()))
        .subscribe()
        .await
        .expect("subscribe");

    // What the address-book watcher raises when something edits the book behind
    // the engine's back — the event a window redraws its people grid on.
    running.daemon.manager.notify_contacts_changed();

    let ev = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .expect("an event arrives")
        .expect("the stream is alive");
    assert!(ev.is_some(), "the stream did not end");

    running.stop();
    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}
