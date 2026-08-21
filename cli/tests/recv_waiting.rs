//! `arvolo recv` with nothing to paste: the offers addressed to you, listed from
//! the relay and taken from the list — driven through the real binary against a
//! relay running in-process.
//!
//! The gap this covers is the one without a daemon. An `arvolo send <you> …` to an
//! offline recipient deposits the file and leaves a sealed offer in their inbox
//! slot; with nobody polling that slot, the offer used to sit there until its TTL
//! lapsed and the recipient never learned it existed. So the three things asserted
//! here are exactly the three that were missing:
//!   1. **see it** — a bare `recv` lists the waiting offer, who it's from, and that
//!      it is fetchable now rather than needing the sender online;
//!   2. **take it** — picking it downloads the file and acks the offer, so a
//!      second run reports an empty inbox instead of offering it twice;
//!   3. **understand the empty case** — nothing waiting says so, and says why a
//!      code or ticket can never appear there.
//!
//! Listing is non-destructive by protocol (the relay drops an offer only on the
//! recipient's DELETE), which is what lets 1 and 2 be separate runs of the command
//! over the same offer.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
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

/// A `Command` for the built binary with an isolated config dir, identity and
/// relay. `ARVOLO_IDENTITY` is what keeps the two sides distinct — the identity
/// path does not follow `ARVOLO_CONFIG_DIR`.
fn arvolo(cfg: &Path, relay: &str, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_arvolo"));
    c.args(args)
        .env("ARVOLO_CONFIG_DIR", cfg)
        .env("ARVOLO_IDENTITY", cfg.join("identity.key"))
        .env("ARVOLO_TEMP_DIR", cfg.join("tmp"))
        .env("ARVOLO_RELAY", relay)
        .env("ARVOLO_NO_WIZARD", "1")
        // Direct-only: no public NAT relay in a test.
        .env("ARVOLO_IROH_RELAY", "")
        .kill_on_drop(true);
    c
}

/// Run to completion with stdin closed — the "just show me the list" case, and
/// what any script gets.
async fn run(cfg: &Path, relay: &str, args: &[&str]) -> (bool, String, String) {
    let out = arvolo(cfg, relay, args).output().await.expect("run arvolo");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Same, but answering the picker on stdin.
async fn run_answering(cfg: &Path, relay: &str, args: &[&str], answer: &str) -> (bool, String) {
    let mut child = arvolo(cfg, relay, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn arvolo");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(answer.as_bytes())
        .await
        .expect("write stdin");
    let out = child.wait_with_output().await.expect("wait arvolo");
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// First whitespace-delimited token of a command's stdout (the id line of `me`).
fn first_word(s: String) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}

#[tokio::test]
async fn lists_what_is_waiting_and_downloads_the_chosen_one() {
    let relay = spawn_relay().await;
    let cfg_send = TempDir::new().unwrap();
    let cfg_recv = TempDir::new().unwrap();

    let send_id = first_word(run(cfg_send.path(), &relay, &["me"]).await.1);
    let recv_id = first_word(run(cfg_recv.path(), &relay, &["me"]).await.1);
    assert!(!send_id.is_empty() && !recv_id.is_empty());
    assert_ne!(send_id, recv_id);

    let file = cfg_send.path().join("report.txt");
    std::fs::write(&file, b"hello from the mailbox").unwrap();

    // The recipient is offline (no daemon, no listen): this deposits the file and
    // leaves a sealed offer in their inbox slot.
    let (ok, out, err) = run(
        cfg_send.path(),
        &relay,
        &[
            "send",
            file.to_str().unwrap(),
            "--to",
            &recv_id,
            "--mailbox",
            "--note",
            "the one we talked about",
        ],
    )
    .await;
    assert!(ok, "send --deposit failed: {out}{err}");

    // 1. See it. Stdin is closed, so the command lists and stops.
    let (ok, listed, err) = run(cfg_recv.path(), &relay, &["recv"]).await;
    assert!(ok, "bare recv should succeed: {listed}{err}");
    assert!(
        listed.contains("report.txt"),
        "the waiting file should be listed: {listed}"
    );
    assert!(
        listed.contains(&send_id),
        "an unsaved sender is shown by id, not by a name they chose: {listed}"
    );
    assert!(
        listed.contains("NEW sender"),
        "a first-time sender should be flagged as new: {listed}"
    );
    assert!(
        listed.contains("the one we talked about"),
        "the note rides inside the sealed offer and belongs in the row: {listed}"
    );
    assert!(
        listed.contains("fetchable now"),
        "a mailbox deposit can be taken without the sender online: {listed}"
    );

    // Listing must not consume it: the same offer is still there.
    let (_, again, _) = run(cfg_recv.path(), &relay, &["recv"]).await;
    assert!(
        again.contains("report.txt"),
        "reading the inbox must not burn the offer: {again}"
    );

    // 2. Take it.
    let out_dir = cfg_recv.path().join("downloads");
    std::fs::create_dir_all(&out_dir).unwrap();
    let (ok, log) = run_answering(
        cfg_recv.path(),
        &relay,
        &["recv", "--out", out_dir.to_str().unwrap()],
        "1\n",
    )
    .await;
    assert!(ok, "taking offer 1 should succeed: {log}");
    let saved = out_dir.join("report.txt");
    assert!(saved.exists(), "the file should be saved: {log}");
    assert_eq!(std::fs::read(&saved).unwrap(), b"hello from the mailbox");

    // 3. And it was acked, so it isn't offered a second time.
    let (ok, after, _) = run(cfg_recv.path(), &relay, &["recv"]).await;
    assert!(ok);
    assert!(
        after.contains("Nothing waiting") && !after.contains("report.txt"),
        "a taken offer should be gone from the relay: {after}"
    );
}

/// The empty case has to explain itself: someone holding a pairing code and
/// waiting for it to appear in this list is waiting for something that cannot
/// happen, and silence would read as a bug rather than as the design.
#[tokio::test]
async fn nothing_waiting_says_why_a_code_can_never_be_listed() {
    let relay = spawn_relay().await;
    let cfg = TempDir::new().unwrap();

    let (ok, out, err) = run(cfg.path(), &relay, &["recv"]).await;
    assert!(ok, "an empty inbox is not an error: {out}{err}");
    assert!(
        out.contains("Nothing waiting"),
        "should say the inbox is empty: {out}"
    );
    assert!(
        out.contains("arvolo recv <code|arvc…|arvm…|link>"),
        "and point at the way a code or ticket is actually used: {out}"
    );
}

/// An unwanted offer can be declined, and declining is what removes it.
///
/// Without this the only way out of an arrival you don't want was to block the
/// person — a much larger statement than "not this file" — or to look at it again
/// on every listing until its TTL lapsed a week later.
#[tokio::test]
async fn an_offer_can_be_declined_and_then_it_is_gone() {
    let relay = spawn_relay().await;
    let cfg_send = TempDir::new().unwrap();
    let cfg_recv = TempDir::new().unwrap();

    let recv_id = first_word(run(cfg_recv.path(), &relay, &["me"]).await.1);
    let file = cfg_send.path().join("unwanted.bin");
    std::fs::write(&file, b"no thanks").unwrap();
    let (ok, out, err) = run(
        cfg_send.path(),
        &relay,
        &[
            "send",
            file.to_str().unwrap(),
            "--to",
            &recv_id,
            "--mailbox",
        ],
    )
    .await;
    assert!(ok, "send --deposit failed: {out}{err}");

    let (ok, log) = run_answering(cfg_recv.path(), &relay, &["recv"], "d1\n").await;
    assert!(ok, "declining should succeed: {log}");
    assert!(log.contains("Declined"), "it should say so: {log}");

    // Gone for good: not offered again, and nothing was downloaded.
    let (ok, after, _) = run(cfg_recv.path(), &relay, &["recv"]).await;
    assert!(ok);
    assert!(
        after.contains("Nothing waiting") && !after.contains("unwanted.bin"),
        "a declined offer must not come back: {after}"
    );
}

/// `status` without a daemon has to show the same waiting offers.
///
/// It claims to list "everything you can still act on", and this is the case where
/// that claim used to fail: with no daemon nobody polls the inbox, so an offer sat
/// on the relay invisible to every command until its TTL lapsed. Listing it must
/// also stay non-destructive — `status` is a view, so the offer is still there for
/// `recv` afterwards.
#[tokio::test]
async fn status_without_a_daemon_shows_what_is_waiting() {
    let relay = spawn_relay().await;
    let cfg_send = TempDir::new().unwrap();
    let cfg_recv = TempDir::new().unwrap();

    let recv_id = first_word(run(cfg_recv.path(), &relay, &["me"]).await.1);
    let file = cfg_send.path().join("invoice.pdf");
    std::fs::write(&file, b"payable on receipt").unwrap();
    let (ok, out, err) = run(
        cfg_send.path(),
        &relay,
        &[
            "send",
            file.to_str().unwrap(),
            "--to",
            &recv_id,
            "--mailbox",
        ],
    )
    .await;
    assert!(ok, "send --deposit failed: {out}{err}");

    let (ok, status, err) = run(cfg_recv.path(), &relay, &["status"]).await;
    assert!(ok, "status should succeed: {status}{err}");
    assert!(
        status.contains("invoice.pdf"),
        "status must show the waiting offer: {status}"
    );
    assert!(
        status.contains("arvolo recv"),
        "and point at the command that takes it: {status}"
    );

    // A view, not an action: the offer survives being looked at.
    let (_, listed, _) = run(cfg_recv.path(), &relay, &["recv"]).await;
    assert!(
        listed.contains("invoice.pdf"),
        "status must not consume the offer: {listed}"
    );
}

/// An empty inbox adds no noise: the section is silent rather than printing a
/// cheerful zero on every run.
#[tokio::test]
async fn status_says_nothing_when_nothing_is_waiting() {
    let relay = spawn_relay().await;
    let cfg = TempDir::new().unwrap();
    // Give the config dir an identity, so the inbox really is read and found empty
    // rather than skipped for want of one.
    run(cfg.path(), &relay, &["me"]).await;

    let (ok, status, err) = run(cfg.path(), &relay, &["status"]).await;
    assert!(ok, "status should succeed: {status}{err}");
    // The section's own heading, not the phrase — the "daemon: not running" line
    // says where offers come from and would match a looser check.
    assert!(
        !status.contains("waiting for you on "),
        "an empty inbox should print no section at all: {status}"
    );
    assert!(
        !status.contains("couldn't ask"),
        "the relay is up, so there's nothing to apologise for: {status}"
    );
}

/// A blocked sender's offer never reaches the list — blocking that only made you
/// look at it more slowly would not be blocking.
#[tokio::test]
async fn a_blocked_sender_is_not_listed() {
    let relay = spawn_relay().await;
    let cfg_send = TempDir::new().unwrap();
    let cfg_recv = TempDir::new().unwrap();

    let send_id = first_word(run(cfg_send.path(), &relay, &["me"]).await.1);
    let recv_id = first_word(run(cfg_recv.path(), &relay, &["me"]).await.1);

    let file = cfg_send.path().join("spam.txt");
    std::fs::write(&file, b"unwanted").unwrap();
    let (ok, out, err) = run(
        cfg_send.path(),
        &relay,
        &[
            "send",
            file.to_str().unwrap(),
            "--to",
            &recv_id,
            "--mailbox",
        ],
    )
    .await;
    assert!(ok, "send --deposit failed: {out}{err}");

    let (ok, out, err) = run(cfg_recv.path(), &relay, &["contacts", "block", &send_id]).await;
    assert!(ok, "block failed: {out}{err}");

    let (ok, listed, _) = run(cfg_recv.path(), &relay, &["recv"]).await;
    assert!(ok);
    assert!(
        !listed.contains("spam.txt"),
        "a blocked sender's offer must not be listed: {listed}"
    );
}
