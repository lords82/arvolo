//! Taking back a mailbox send has to take back *both* things it left on the relay:
//! the sealed blob **and** the offer sitting in the recipient's inbox.
//!
//! Revoking only the blob is the failure this pins. It looks like it worked — the
//! file 404s, the local record is gone, the sender is told it's deleted — while the
//! recipient still has an arrival pointing at it: their daemon retries a claim that
//! will never resolve, and a person sees a file that never lands. Nothing on the
//! sender's side would ever show that, which is why it needs a real recipient's
//! inbox, on a real relay, to catch.
//!
//! Driven through the actual binary against an in-process relay: the one-shot
//! `send <who> --deposit` path has no daemon behind it, so the tokens that retract
//! the offer only survive if the *record* keeps them — which is the whole fix.

use std::path::Path;
use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::presence::InboxSubscription;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
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

/// Run the real binary with an isolated config dir **and identity**.
///
/// `ARVOLO_IDENTITY` is not optional here: without it the identity path falls back
/// to `$HOME/.config/arvolo/identity.key`, so the test would read — and write — the
/// identity of whoever runs it.
async fn run(cfg: &Path, relay: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_arvolo"))
        .args(args)
        .env("ARVOLO_CONFIG_DIR", cfg)
        .env("ARVOLO_IDENTITY", cfg.join("identity.key"))
        .env("ARVOLO_RELAY", relay)
        .env("ARVOLO_NO_WIZARD", "1")
        .kill_on_drop(true)
        .output()
        .await
        .expect("run arvolo");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[tokio::test]
async fn cancelling_a_mailbox_send_also_retracts_the_recipients_offer() {
    let relay = spawn_relay().await;
    let alice = tempfile::tempdir().unwrap();

    // Bob never runs: he is the offline recipient, and his inbox is the evidence.
    let bob = Identity::generate();
    let bob_b32 = data_encoding::BASE32_NOPAD
        .encode(&bob.public().to_bytes())
        .to_lowercase();

    let file = alice.path().join("budget.txt");
    std::fs::write(&file, b"documento riservato").unwrap();

    let (ok, _) = run(alice.path(), &relay, &["contacts", "add", "bob", &bob_b32]).await;
    assert!(ok, "contacts add");

    // `--deposit` forces the mailbox path even though presence is unknown: one blob
    // on the relay, one offer in bob's inbox, no daemon anywhere.
    let (ok, out) = run(
        alice.path(),
        &relay,
        &[
            "send",
            "bob",
            file.to_str().unwrap(),
            "--deposit",
            "--use-http",
        ],
    )
    .await;
    assert!(ok, "send bob --deposit failed: {out}");

    let inbox = InboxSubscription::new(relay.clone(), &bob);
    let offers = inbox.poll().await.expect("poll bob's inbox");
    assert_eq!(offers.len(), 1, "bob should have been offered the file");

    // The deposit id is the handle the user is given, so take it the way they do.
    let id = out
        .split_whitespace()
        .find(|w| w.len() == 8 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| panic!("no deposit id in output: {out}"))
        .to_string();

    let (ok, out) = run(alice.path(), &relay, &["cancel", &id]).await;
    assert!(ok, "cancel {id} failed: {out}");

    // The blob is gone — the half that always worked.
    let claim = offers[0].offer.ticket.clone();
    assert!(!claim.is_empty(), "the offer carries the arvm ticket");

    // …and so is the offer. This is the half that used to linger until its TTL,
    // pointing bob at a file that no longer exists.
    let after = inbox.poll().await.expect("poll bob's inbox again");
    assert!(
        after.is_empty(),
        "the offer must be retracted with the blob, else bob keeps chasing a 404: {after:?}"
    );
}
