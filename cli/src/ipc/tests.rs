//! End-to-end IPC tests: a real daemon `server` bound to a temp-dir socket,
//! driven by the real `client`, over the actual `socket_path()`.
//!
//! Each test holds the process-global `testlock::ENV` guard for its whole body —
//! including across `.await`s — on purpose: it keeps `ARVOLO_CONFIG_DIR` (and thus
//! `socket_path()`) stable while the daemon and client run. Only one test holds it
//! at a time and nothing awaited re-acquires it, so there's no deadlock.
#![cfg(unix)]
// These drive the transport itself — binding a socket, dialling it, half-closing
// it — rather than the protocol on top. That is unix vocabulary by construction:
// the Windows side is a named pipe with a different shape of listener, and the
// protocol tests it shares are the ones above the transport.
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
    let daemon = Daemon::new(
        manager,
        Some("https://relay.test".into()),
        dir.path().join("downloads"),
        Arc::new(Mutex::new(HashMap::new())),
    );
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

/// The whole address-book surface a GUI needs, driven over the socket: add,
/// verify (and see the fingerprint travel), trust — refused unverified, then
/// forced —, block, rename keeping the marks, remove. One flowing test rather
/// than seven, because the point is that the *same* contact survives each step
/// with the right marks, which per-step tests with fresh daemons cannot see.
#[tokio::test]
async fn contact_lifecycle_over_the_socket() {
    let _guard = crate::testlock::ENV
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (shutdown, _dir) = spawn_test_daemon().await;

    let id = Identity::generate().public();
    let id_b32 = data_encoding::BASE32_NOPAD
        .encode(&id.to_bytes())
        .to_lowercase();

    let mut client = DaemonClient::connect().await.expect("connect");

    client
        .add_contact("bob".into(), id_b32.clone())
        .await
        .expect("add");
    // Garbage id: refused, not saved.
    assert!(client
        .add_contact("mallory".into(), "not-a-key".into())
        .await
        .is_err());

    // Trust before verify is refused (MITM risk) — the CLI rule, on the wire.
    assert!(client.mark_trusted("bob".into(), false).await.is_err());
    // …but can be forced, exactly like `contacts trust --force`.
    client
        .mark_trusted("bob".into(), true)
        .await
        .expect("force trust");
    client.mark_untrusted("bob".into()).await.expect("untrust");

    client.mark_verified("bob".into()).await.expect("verify");
    client
        .mark_trusted("bob".into(), false)
        .await
        .expect("trust verified");

    client.block("bob".into()).await.expect("block");
    let c = &client.list_contacts().await.expect("list")[0];
    assert!(c.verified && c.trusted && c.blocked);
    assert!(!c.fingerprint.is_empty());
    client.unblock("bob".into()).await.expect("unblock");

    // Rename keeps the id-keyed marks — the reason rename exists at all.
    client
        .rename_contact("bob".into(), "roberto".into())
        .await
        .expect("rename");
    let contacts = client.list_contacts().await.expect("list");
    assert_eq!(contacts[0].name, "roberto");
    assert!(contacts[0].verified && contacts[0].trusted);

    client
        .remove_contact("roberto".into())
        .await
        .expect("remove");
    assert!(client.remove_contact("roberto".into()).await.is_err());
    assert!(client.list_contacts().await.expect("list").is_empty());

    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}

/// History and the advertised display name, over the socket. The daemon's own
/// status must carry the name a moment after it is set — the GUI shows it in the
/// header, and a stale answer there means the user "renamed themselves" into thin
/// air.
#[tokio::test]
async fn history_and_display_name_over_the_socket() {
    let _guard = crate::testlock::ENV
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (shutdown, _dir) = spawn_test_daemon().await;

    let mut client = DaemonClient::connect().await.expect("connect");

    assert!(client.list_history().await.expect("history").is_empty());
    crate::history::record("send", None, "done.bin", 10, 10, "completed").unwrap();
    let hist = client.list_history().await.expect("history");
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].name, "done.bin");
    assert_eq!(hist[0].status, "completed");
    assert_eq!(client.clear_history().await.expect("clear"), 1);
    assert!(client.list_history().await.expect("history").is_empty());

    client.set_my_name("Lorenzo".into()).await.expect("name");
    assert_eq!(
        client.status().await.expect("status").display_name,
        "Lorenzo"
    );
    client.set_my_name("".into()).await.expect("clear name");
    assert_eq!(client.status().await.expect("status").display_name, "");

    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}

/// `Recv` with something that is neither ticket, code nor offline ticket answers
/// with a helpful error instead of registering a doomed transfer row.
#[tokio::test]
async fn recv_rejects_gibberish_over_the_socket() {
    let _guard = crate::testlock::ENV
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (shutdown, _dir) = spawn_test_daemon().await;

    let mut client = DaemonClient::connect().await.expect("connect");
    let err = client
        .recv("definitely-not-a-ticket".into(), None, None)
        .await
        .expect_err("gibberish must be refused");
    assert!(err.to_string().contains("arvc"), "explains what it accepts");

    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}

/// A deposit the engine made is withdrawn *through* the engine, because only the
/// engine also retracts the offer it left in the recipient's inbox (and ends the
/// still-live row). Revoking the blob behind its back would be half a withdrawal.
///
/// The unreachable relay is the assertion. The direct path would dial it, fail, and
/// answer `Error` with the record still on disk; going through the manager touches
/// no relay here at all. So `Ok` + a gone record can only mean it dispatched right.
#[tokio::test]
async fn revoking_a_deposit_the_engine_made_goes_through_the_engine() {
    let _guard = crate::testlock::ENV
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

    let manager = TransferManager::new(Identity::generate(), None, dir.path().join("downloads"));
    // Any engine row will do: what's under test is the dispatch, not the transfer.
    let tid = manager.start_download(
        "arvm-not-a-real-ticket".into(),
        dir.path().join("out.bin"),
        None,
        "budget.xlsx".into(),
        10,
    );
    assert!(manager.get(tid).is_some());

    let rec = crate::deposits::save(
        crate::deposits::KIND_OFFLINE,
        // Reserved for documentation; a dial would stall, never succeed.
        "http://192.0.2.1:1",
        "claim-xyz",
        "revoke-me",
        "budget.xlsx",
        10,
        1,
        None,
        "arvmHANDOVER",
        None,
        crate::util::now_unix() + 3600,
        Some(tid),
        None,
    )
    .unwrap();
    assert!(!rec.expired(), "an expired record skips the relay anyway");

    let listener = tokio::net::UnixListener::bind(socket_path()).unwrap();
    let shutdown = CancellationToken::new();
    let daemon = Daemon::new(
        manager,
        Some("https://relay.test".into()),
        dir.path().join("downloads"),
        Arc::new(Mutex::new(HashMap::new())),
    );
    let stop = shutdown.clone();
    tokio::spawn(async move {
        let _ = server::run(daemon, listener, stop).await;
    });

    let mut client = DaemonClient::connect().await.expect("connect");
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.revoke_deposit(rec.id.clone()),
    )
    .await
    .expect("must not hang on the relay")
    .expect("must not error out through the direct path");

    assert!(
        crate::deposits::load(&rec.id).is_none(),
        "the record must be gone"
    );

    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}

/// `arvolo status clear` drops finished rows and **keeps the history**: they are
/// two different stores answering two different questions ("what's going on" vs
/// "what happened"), and the history is the only permanent one. Wiping both from one
/// verb would leave no record of a transfer the user only meant to tidy off a list.
#[tokio::test]
async fn clearing_finished_transfers_leaves_the_history_alone() {
    let _guard = crate::testlock::ENV
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (shutdown, _dir) = spawn_test_daemon().await;

    crate::history::record("send", None, "done.txt", 10, 10, "completed").unwrap();
    assert_eq!(crate::history::list().len(), 1);

    let mut client = DaemonClient::connect().await.expect("connect");
    // Nothing running, so nothing to drop — the point is what survives.
    assert_eq!(client.clear_finished().await.expect("clear"), 0);

    assert_eq!(
        crate::history::list().len(),
        1,
        "clear must not touch the history log"
    );

    shutdown.cancel();
    std::env::remove_var("ARVOLO_CONFIG_DIR");
}

/// A command this daemon has never heard of must come back as an **error on the
/// caller's own id**, not a placeholder.
///
/// This is the shape of every upgrade: `cargo install` replaces the binary, the
/// daemon keeps running the old code, and the next new-CLI command reaches a build
/// that can't parse it. The client blocks reading for its correlation id and skips
/// everything else, so a reply stamped `0` isn't an error — it's a hang, with no
/// hint that a restart is all it needed. (This is not theoretical: `clear_finished`
/// hung exactly this way against a daemon from ten minutes earlier.)
#[tokio::test]
async fn an_unknown_command_answers_the_caller_instead_of_hanging_it() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let _guard = crate::testlock::ENV
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (shutdown, _dir) = spawn_test_daemon().await;

    // Raw socket: the typed client can't send a variant that doesn't exist.
    let stream = tokio::net::UnixStream::connect(socket_path())
        .await
        .unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    write
        .write_all(b"{\"id\":7,\"cmd\":\"from_a_future_cli\"}\n")
        .await
        .unwrap();

    let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
        .await
        .expect("must answer, not hang")
        .unwrap()
        .expect("a reply line");
    let msg: super::protocol::ServerMessage = serde_json::from_str(&line).unwrap();
    match msg {
        super::protocol::ServerMessage::Reply { id, result } => {
            assert_eq!(
                id, 7,
                "the reply must carry the caller's id, not a placeholder"
            );
            assert!(matches!(result, super::protocol::Response::Error(_)));
        }
        other => panic!("expected a Reply, got {other:?}"),
    }

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
