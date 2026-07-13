//! End-to-end IPC tests: a real daemon `server` bound to a temp-dir socket,
//! driven by the real `client`, over the actual `socket_path()`.
//!
//! Each test holds the process-global `testlock::ENV` guard for its whole body —
//! including across `.await`s — on purpose: it keeps `ARVOLO_CONFIG_DIR` (and thus
//! `socket_path()`) stable while the daemon and client run. Only one test holds it
//! at a time and nothing awaited re-acquires it, so there's no deadlock.
#![allow(clippy::await_holding_lock)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arvolo_core::crypto::Identity;
use arvolo_core::manager::TransferManager;
use tokio_util::sync::CancellationToken;

use super::client::DaemonClient;
use super::server::{self, Daemon};
use super::socket_path;

/// Stand up a daemon on a temp socket; returns the shutdown token and a guard
/// dir kept alive for the test's duration.
async fn spawn_test_daemon() -> (CancellationToken, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

    let manager = TransferManager::new(
        Identity::generate(),
        None, // no relay: RPC surface doesn't need one
        dir.path().join("downloads"),
    );
    let listener = tokio::net::UnixListener::bind(socket_path()).unwrap();
    let shutdown = CancellationToken::new();
    let daemon = Daemon {
        manager,
        relay: Some("https://relay.test".into()),
        download_dir: dir.path().join("downloads"),
        pending: Arc::new(Mutex::new(HashMap::new())),
    };
    let stop = shutdown.clone();
    tokio::spawn(async move {
        let _ = server::run(daemon, listener, stop).await;
    });
    (shutdown, dir)
}

#[tokio::test]
async fn rpc_roundtrips_over_the_socket() {
    let _guard = crate::testlock::ENV
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (shutdown, _dir) = spawn_test_daemon().await;

    let mut client = DaemonClient::connect().await.expect("connect");

    // Ping / Pong.
    client.ping().await.expect("ping");

    // Status carries our identity and the configured relay; nothing running yet.
    let st = client.status().await.expect("status");
    assert!(!st.public_id.is_empty());
    assert_eq!(st.relay.as_deref(), Some("https://relay.test"));
    assert_eq!(st.transfers, 0);
    assert_eq!(st.pending, 0);

    // Empty lists.
    assert!(client.list().await.expect("list").is_empty());
    assert!(client.list_pending().await.expect("pending").is_empty());

    // Rejecting an unknown offer is idempotent (Ok); accepting one errors.
    client
        .reject("nope".into())
        .await
        .expect("reject unknown ok");
    assert!(client.accept("nope".into(), None).await.is_err());

    // A second client can connect concurrently and subscribe (handshake returns
    // an event stream without error).
    let client2 = DaemonClient::connect().await.expect("connect 2");
    let _stream = client2.subscribe().await.expect("subscribe handshake");

    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}

#[tokio::test]
async fn list_contacts_over_the_socket() {
    let _guard = crate::testlock::ENV
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (shutdown, dir) = spawn_test_daemon().await;

    // Seed the address book the daemon reads (same ARVOLO_CONFIG_DIR).
    let _ = dir; // kept alive for the test
    let id = Identity::generate().public();
    let id_b32 = data_encoding::BASE32_NOPAD
        .encode(&id.to_bytes())
        .to_lowercase();
    crate::book::contact_add("alice", &id_b32).unwrap();
    crate::book::mark_verified("alice").unwrap();

    let mut client = DaemonClient::connect().await.expect("connect");
    let contacts = client.list_contacts().await.expect("list_contacts");
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].name, "alice");
    assert_eq!(contacts[0].id, id_b32);
    assert!(contacts[0].verified, "alice was marked verified");
    assert!(!contacts[0].fingerprint.is_empty());

    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}

#[tokio::test]
async fn second_bind_on_the_same_socket_fails() {
    let _guard = crate::testlock::ENV
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (shutdown, _dir) = spawn_test_daemon().await;

    // The single-instance guard relies on bind() refusing an in-use path.
    let err = tokio::net::UnixListener::bind(socket_path());
    assert!(err.is_err(), "binding an in-use socket path must fail");

    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}
