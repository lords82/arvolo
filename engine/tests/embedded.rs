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

/// Every test here points `ARVOLO_CONFIG_DIR` at its own directory, and the
/// variable is one per process: run side by side, one test's engine reads
/// another's address book and history. Held for the whole of each test.
static ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A fresh installation in a temp dir, with an engine running inside this test.
///
/// The relay is a port nothing listens on: the inbox subscription retries in the
/// background and none of what is asserted here goes near it. What matters is
/// that `start` returns something that answers.
async fn engine(dir: &std::path::Path) -> (Running, CancellationToken) {
    engine_restoring(dir, 0).await
}

/// The same, for an embedder whose engine does not outlive its window — which is
/// what `restore_board` exists for.
async fn engine_restoring(
    dir: &std::path::Path,
    restore_board: usize,
) -> (Running, CancellationToken) {
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
        restore_board,
        front_ends: Arc::new(AtomicUsize::new(0)),
    })
    .await
    .expect("the engine starts");
    (running, shutdown)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_with_no_socket_answers_the_same_way() {
    let _env = ENV.lock().await;
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
    let _env = ENV.lock().await;
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

/// The phone's mailbox, after the phone has put the app away.
///
/// A desktop daemon holds its list for weeks, so nobody noticed that the list
/// *is* the memory: on iOS the engine dies with the window, and the screen that
/// had shown a morning of transfers came back empty every time — with the files
/// themselves sitting in Documents, saying otherwise.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_opens_its_mailbox_and_finds_what_arrived_before() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
    std::env::remove_var("ARVOLO_IDENTITY");

    // What the last run left behind. Written through the real writer, because the
    // shape on disk is the thing under test.
    arvolo_engine::history::record(
        "recv",
        None,
        "ciao.txt",
        9,
        9,
        "completed",
        Some("/tmp/Documents/ciao.txt".into()),
        None,
        String::new(),
    )
    .expect("a record is written");
    arvolo_engine::history::record(
        "send",
        None,
        "gone.bin",
        4,
        0,
        "cancelled",
        None,
        None,
        String::new(),
    )
    .expect("a record is written");

    let (running, shutdown) = engine_restoring(dir.path(), 300).await;
    let mut c = DaemonClient::embedded(Arc::new(running.daemon.clone()));
    let rows = c.list().await.expect("the list answers");

    let arrived = rows
        .iter()
        .find(|t| t.name == "ciao.txt")
        .expect("what arrived last time is still in the mailbox");
    assert_eq!(arrived.status, "completed");
    assert_eq!(
        arrived.path.as_deref(),
        Some("/tmp/Documents/ciao.txt"),
        "and it still knows where the file went, or the row cannot offer to open it"
    );
    assert!(
        rows.iter().any(|t| t.name == "gone.bin"),
        "a transfer that did not finish is still something that happened"
    );

    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}

/// `restore_board: 0` still means what it says — the engine puts nothing back
/// unless it is asked to. Both callers now ask (a daemon restarts rarely, but
/// when it does its board was just as empty), so this is the seam itself under
/// test rather than a platform's choice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_comes_back_unless_the_embedder_asks() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
    std::env::remove_var("ARVOLO_IDENTITY");
    arvolo_engine::history::record(
        "recv",
        None,
        "old.txt",
        1,
        1,
        "completed",
        None,
        None,
        String::new(),
    )
    .expect("a record is written");

    let (running, shutdown) = engine(dir.path()).await;
    let mut c = DaemonClient::embedded(Arc::new(running.daemon.clone()));
    assert!(
        c.list().await.expect("the list answers").is_empty(),
        "nothing is put back where the process that remembers is still running"
    );

    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}
