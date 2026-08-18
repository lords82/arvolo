//! End-to-end advertised-display-name (petname) flow driven through the real
//! `arvolo` binary against a relay running in-process:
//!   1. the sender sets a display name (`arvolo me name`) and deposits an offer to
//!      the receiver (`send <who> --deposit` → mailbox + inbox offer carrying it);
//!   2. the receiver's `listen` shows the advertised name on the incoming offer —
//!      as a *new*, unverified petname claim, distinct from the local alias;
//!   3. the receiver approves it (`contacts accept-name`) → it pins, and
//!      `contacts list` shows the local alias as primary AND the pinned name.
//!
//! This exercises the whole chain the unit tests can't cover on their own: the
//! name traveling inside the sealed offer over a real relay, the manager emitting
//! it, the `listen` banner, durable pending persistence, and the approve step.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use arvolo_core::backfill::BlobNode;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

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
        // Isolate the identity per config dir: `identity_path()` keys off
        // ARVOLO_IDENTITY (else a shared HOME path), so without this both sides
        // would load the same key and sender == receiver.
        .env("ARVOLO_IDENTITY", cfg.join("identity.key"))
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

/// First whitespace-delimited token of a command's stdout (e.g. the id line).
fn first_word(s: String) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}

#[tokio::test]
async fn advertised_name_shown_and_approved_end_to_end() {
    let relay = spawn_relay().await;
    let cfg_send = TempDir::new().unwrap();
    let cfg_recv = TempDir::new().unwrap();

    // Two distinct identities.
    let send_id = first_word(run(cfg_send.path(), &relay, &["me"]).await.1);
    let recv_id = first_word(run(cfg_recv.path(), &relay, &["me"]).await.1);
    assert!(!send_id.is_empty() && !recv_id.is_empty());
    assert_ne!(send_id, recv_id);

    // Sender advertises a self-chosen display name.
    let (ok, _) = run(cfg_send.path(), &relay, &["me", "name", "Lorenzo"]).await;
    assert!(ok, "arvolo me name");

    // Receiver saves the sender under a LOCAL alias — it must stay primary; the
    // advertised name is shown alongside, never replacing it.
    let (ok, _) = run(
        cfg_recv.path(),
        &relay,
        &["contacts", "add", "boss", &send_id],
    )
    .await;
    assert!(ok, "contacts add");

    // A small file to send.
    let file = cfg_send.path().join("report.txt");
    std::fs::write(&file, b"hello").unwrap();

    // Receiver goes online. Auto-accept the saved contact so no interactive prompt
    // blocks; the advertised-name banner prints before the accept regardless.
    let mut listen = arvolo(
        cfg_recv.path(),
        &relay,
        &["listen", "--accept", "contacts"],
    )
    .stderr(Stdio::piped())
    .stdout(Stdio::null())
    .spawn()
    .expect("spawn listen");
    let mut lines = BufReader::new(listen.stderr.take().unwrap()).lines();

    // Let listen subscribe to its inbox, then deposit an offer (mailbox path exits
    // promptly and still posts an inbox offer carrying the name).
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (ok, out) = run(
        cfg_send.path(),
        &relay,
        &["send", file.to_str().unwrap(), "--to", &recv_id, "--mailbox"],
    )
    .await;
    assert!(ok, "send --deposit failed: {out}");

    // The receiver's listen shows the advertised name as a NEW, unverified claim.
    let mut saw_name = false;
    while let Ok(Ok(Some(line))) = timeout(Duration::from_secs(25), lines.next_line()).await {
        if line.contains("Lorenzo") {
            saw_name = true;
            break;
        }
    }
    let _ = listen.start_kill();
    assert!(
        saw_name,
        "listen should surface the sender's advertised name 'Lorenzo'"
    );

    // The pending name was persisted during listen → approving it now pins it.
    let (ok, out) = run(
        cfg_recv.path(),
        &relay,
        &["contacts", "rename", &send_id],
    )
    .await;
    assert!(ok, "accept-name: {out}");
    assert!(
        out.contains("Lorenzo"),
        "accept-name confirms the name: {out}"
    );

    // The listing keeps the local alias primary and shows the pinned advertised name.
    let (_, list) = run(cfg_recv.path(), &relay, &["contacts", "list"]).await;
    assert!(list.contains("boss"), "local alias stays primary: {list}");
    assert!(
        list.contains("Lorenzo"),
        "advertised name pinned and shown alongside: {list}"
    );
}
