//! `arvolo contacts pair` end to end: two people, one short code, and both come
//! away with the other saved **and verified**.
//!
//! The point being pinned is that the exchange is *mutual*. The rendezvous is
//! otherwise one-directional — a sender puts something in a slot and a receiver
//! takes it — so the side showing the code learning the other's id at all depends
//! on the reply channel added for this. A regression there would look like
//! success on the joiner's side and silence on the host's, which is exactly the
//! half-working state a test has to catch.
//!
//! Verification is asserted too, not just the save: the whole reason to pair
//! rather than paste an id is that the PAKE channel authenticates the key.

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

fn arvolo(cfg: &Path, relay: &str, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_arvolo"));
    c.args(args)
        .env("ARVOLO_CONFIG_DIR", cfg)
        // Its own identity: follows ARVOLO_CONFIG_DIR now, pinned anyway so the
        // two sides can never share a key and turn "pairing" into a mirror.
        .env("ARVOLO_IDENTITY", cfg.join("identity.key"))
        .env("ARVOLO_RELAY", relay)
        .env("ARVOLO_NO_WIZARD", "1")
        .kill_on_drop(true);
    c
}

async fn run(cfg: &Path, relay: &str, args: &[&str]) -> (bool, String, String) {
    let out = arvolo(cfg, relay, args).output().await.expect("run arvolo");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// First whitespace-delimited token of stdout (the id line of `arvolo me`).
fn first_word(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}

#[tokio::test]
async fn pairing_saves_and_verifies_both_sides() {
    let relay = spawn_relay().await;
    let cfg_a = TempDir::new().unwrap();
    let cfg_b = TempDir::new().unwrap();

    let (_, id_a, _) = run(cfg_a.path(), &relay, &["me"]).await;
    let (_, id_b, _) = run(cfg_b.path(), &relay, &["me"]).await;
    let (id_a, id_b) = (first_word(&id_a), first_word(&id_b));
    assert!(!id_a.is_empty() && !id_b.is_empty());
    assert_ne!(id_a, id_b, "the two sides must be different identities");

    // A shows a code and waits. The code goes to stdout; everything else to stderr.
    let mut host = arvolo(cfg_a.path(), &relay, &["contacts", "add", "bob"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the host side");

    let mut lines = BufReader::new(host.stdout.take().unwrap()).lines();
    let mut code = String::new();
    while let Ok(Ok(Some(line))) = timeout(Duration::from_secs(20), lines.next_line()).await {
        let t = line.trim();
        if !t.is_empty() {
            code = t.to_string();
            break;
        }
    }
    assert!(
        code.contains('-'),
        "the host should print a pairing code, got: {code:?}"
    );

    // B types it.
    let (ok, out, err) = run(cfg_b.path(), &relay, &["contacts", "add", "alice", &code]).await;
    assert!(ok, "the joining side failed: {err}");
    assert!(out.contains("verified"), "joiner should verify: {out}");

    let status = timeout(Duration::from_secs(30), host.wait())
        .await
        .expect("the host side should finish once paired")
        .expect("host exit");
    assert!(status.success(), "the host side failed");

    // The exchange really crossed: each has the *other's* id, and trusts the key
    // enough to have marked it verified.
    let (_, book_a, _) = run(
        cfg_a.path(),
        &relay,
        &["contacts", "list", "--json", "--no-presence"],
    )
    .await;
    let (_, book_b, _) = run(
        cfg_b.path(),
        &relay,
        &["contacts", "list", "--json", "--no-presence"],
    )
    .await;

    assert!(
        book_a.contains(&id_b),
        "A must have B's id after pairing.\nA's book: {book_a}"
    );
    assert!(
        book_b.contains(&id_a),
        "B must have A's id after pairing.\nB's book: {book_b}"
    );
    assert!(
        book_a.contains("\"verified\": true") && book_b.contains("\"verified\": true"),
        "pairing authenticates the key, so both sides must be verified"
    );
    // And neither side accidentally saved itself.
    assert!(!book_a.contains(&id_a), "A must not have saved its own id");
    assert!(!book_b.contains(&id_b), "B must not have saved its own id");
}

/// A wrong code must not save anything. The joiner cannot open the payload, so
/// there is no id to save — and it must not write its own reply either, or a
/// guesser would learn identities by typing codes at random.
#[tokio::test]
async fn a_wrong_code_saves_nobody() {
    let relay = spawn_relay().await;
    let cfg_a = TempDir::new().unwrap();
    let cfg_b = TempDir::new().unwrap();

    let mut host = arvolo(cfg_a.path(), &relay, &["contacts", "add", "bob"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the host side");

    let mut lines = BufReader::new(host.stdout.take().unwrap()).lines();
    let mut code = String::new();
    while let Ok(Ok(Some(line))) = timeout(Duration::from_secs(20), lines.next_line()).await {
        let t = line.trim();
        if !t.is_empty() {
            code = t.to_string();
            break;
        }
    }
    // Keep the nameplate, change the words: a plausible mistype, not a bad format.
    let wrong = {
        let (head, rest) = code.split_once('-').expect("code has a nameplate");
        let tail = rest
            .split_once('@')
            .map(|(_, r)| format!("@{r}"))
            .unwrap_or_default();
        format!("{head}-wrong-words{tail}")
    };

    let (ok, _, _) = run(cfg_b.path(), &relay, &["contacts", "add", "alice", &wrong]).await;
    assert!(!ok, "a wrong code must fail, not save a contact");

    let (_, book_b, _) = run(
        cfg_b.path(),
        &relay,
        &["contacts", "list", "--json", "--no-presence"],
    )
    .await;
    assert!(
        book_b.trim() == "[]" || !book_b.contains("\"id\""),
        "nothing should have been saved from a wrong code: {book_b}"
    );

    let _ = host.start_kill();
}
