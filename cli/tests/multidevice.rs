//! End-to-end multi-device flow driven through the real `arvolo` binary against a
//! relay running in-process:
//!   1. `device pair` (device A) + `device join` (device B) → both end up sharing
//!      one identity, and B imports A's address book (G2: pairing);
//!   2. A adds a new contact, `sync now` on A then B → the contact appears on B
//!      (G3: the `sync now` orchestration and the mutable-cell round-trip).
//!
//! Pairing and sync ride the HTTP relay (rendezvous + inbox), so no iroh/NAT relay
//! is involved and the test is deterministic on localhost.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

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

/// A `Command` for the built `arvolo` binary with an isolated config dir + relay.
fn arvolo(cfg: &Path, relay: &str, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_arvolo"));
    c.args(args)
        .env("ARVOLO_CONFIG_DIR", cfg)
        .env("ARVOLO_RELAY", relay)
        .env("ARVOLO_NO_WIZARD", "1")
        .kill_on_drop(true);
    c
}

/// Run to completion; return (success, stdout).
async fn run(cfg: &Path, relay: &str, args: &[&str]) -> (bool, String) {
    let out = arvolo(cfg, relay, args).output().await.expect("run arvolo");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn fresh_id() -> String {
    data_encoding::BASE32_NOPAD
        .encode(&Identity::generate().public().to_bytes())
        .to_lowercase()
}

#[tokio::test]
async fn device_pair_join_and_sync_end_to_end() {
    let relay = spawn_relay().await;
    let cfg_a = TempDir::new().unwrap();
    let cfg_b = TempDir::new().unwrap();

    // Device A saves a contact so its address book is non-empty at pairing time.
    let alice = fresh_id();
    let (ok, _) = run(cfg_a.path(), &relay, &["contacts", "add", "alice", &alice]).await;
    assert!(ok, "contacts add on A");

    // Start `device pair` on A in the background and read the join code it prints.
    let mut pair = arvolo(cfg_a.path(), &relay, &["device", "pair"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn device pair");
    let mut lines = BufReader::new(pair.stdout.take().unwrap()).lines();
    let mut code = None;
    while let Ok(Ok(Some(line))) =
        tokio::time::timeout(Duration::from_secs(25), lines.next_line()).await
    {
        if let Some(rest) = line.split_once("device join ") {
            code = Some(rest.1.trim().to_string());
            break;
        }
    }
    let code = code.expect("device pair printed a join code");

    // Device B joins with the code (--yes overwrites B's fresh identity).
    let (ok, log) = run(cfg_b.path(), &relay, &["device", "join", &code, "--yes"]).await;
    assert!(ok, "device join failed: {log}");
    let _ = tokio::time::timeout(Duration::from_secs(25), pair.wait()).await;

    // After pairing the two devices share ONE identity...
    let (_, id_a) = run(cfg_a.path(), &relay, &["id"]).await;
    let (_, id_b) = run(cfg_b.path(), &relay, &["id"]).await;
    let id_a = id_a.split_whitespace().next().unwrap_or("");
    let id_b = id_b.split_whitespace().next().unwrap_or("");
    assert!(!id_a.is_empty());
    assert_eq!(id_a, id_b, "devices share one identity after pairing");

    // ...and B imported A's address book.
    let (_, list_b) = run(cfg_b.path(), &relay, &["contacts", "list"]).await;
    assert!(
        list_b.contains("alice"),
        "B imported the address book: {list_b}"
    );

    // Now exercise `sync now`: A adds a new contact and publishes; B pulls it.
    let bob = fresh_id();
    let (ok, _) = run(cfg_a.path(), &relay, &["contacts", "add", "bob", &bob]).await;
    assert!(ok, "contacts add bob on A");
    let (ok, _) = run(cfg_a.path(), &relay, &["sync", "now"]).await;
    assert!(ok, "sync now on A");
    let (ok, _) = run(cfg_b.path(), &relay, &["sync", "now"]).await;
    assert!(ok, "sync now on B");

    let (_, list_b) = run(cfg_b.path(), &relay, &["contacts", "list"]).await;
    assert!(
        list_b.contains("bob"),
        "sync now propagated the new contact to B: {list_b}"
    );
}
