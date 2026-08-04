//! End-to-end checks for the trust/verify gate on the `contacts` command:
//! trusting a contact (auto-download without a prompt) must sit on a key the
//! user has verified out-of-band, unless they explicitly override with --force.

use std::process::{Command, Output};

use arvolo_core::crypto::Identity;
use tempfile::TempDir;

/// Run `arvolo <args>` with an isolated config dir, identity and non-interactive stdin.
///
/// The identity needs its own variable: its path doesn't follow `ARVOLO_CONFIG_DIR`,
/// it falls back to `$HOME/.config/arvolo/identity.key` — so without this a test run
/// reads, and can create, the identity of whoever ran it.
fn arvolo(cfg: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arvolo"))
        .args(args)
        .env("ARVOLO_CONFIG_DIR", cfg.path())
        .env("ARVOLO_IDENTITY", cfg.path().join("identity.key"))
        // No relay: keeps `contacts list` from reaching out; irrelevant here.
        .env_remove("ARVOLO_RELAY")
        .output()
        .expect("run arvolo binary")
}

/// A fresh, valid base32 public id (what a contact's key looks like on the CLI).
fn fresh_id() -> String {
    let id = Identity::generate().public();
    data_encoding::BASE32_NOPAD
        .encode(&id.to_bytes())
        .to_lowercase()
}

#[test]
fn trust_requires_verification_unless_forced() {
    let cfg = TempDir::new().unwrap();
    let id = fresh_id();

    // Save the contact.
    let out = arvolo(&cfg, &["contacts", "add", "alice", &id]);
    assert!(out.status.success(), "add should succeed: {out:?}");

    // Trusting an UNVERIFIED contact is refused (non-zero exit, explains why).
    let out = arvolo(&cfg, &["contacts", "trust", "alice"]);
    assert!(
        !out.status.success(),
        "trust of an unverified contact must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("isn't verified"),
        "error should explain the missing verification, got: {stderr}"
    );

    // --force overrides the gate: it trusts but warns.
    let out = arvolo(&cfg, &["contacts", "trust", "alice", "--force"]);
    assert!(
        out.status.success(),
        "trust --force should succeed: {out:?}"
    );

    // Untrust, verify out-of-band (--yes skips the interactive prompt), then a
    // plain trust is accepted.
    assert!(arvolo(&cfg, &["contacts", "untrust", "alice"])
        .status
        .success());
    let out = arvolo(&cfg, &["contacts", "verify", "alice", "--yes"]);
    assert!(out.status.success(), "verify --yes should succeed: {out:?}");
    let out = arvolo(&cfg, &["contacts", "trust", "alice"]);
    assert!(
        out.status.success(),
        "trust of a verified contact should succeed: {out:?}"
    );
}

#[test]
fn verify_needs_yes_when_non_interactive() {
    let cfg = TempDir::new().unwrap();
    let id = fresh_id();
    assert!(arvolo(&cfg, &["contacts", "add", "bob", &id])
        .status
        .success());

    // Piped (non-TTY) stdin without --yes: verify refuses rather than marking.
    let out = arvolo(&cfg, &["contacts", "verify", "bob"]);
    assert!(
        !out.status.success(),
        "verify without --yes in a non-tty must not silently mark"
    );

    // The gate that depends on it still holds: trust is refused.
    let out = arvolo(&cfg, &["contacts", "trust", "bob"]);
    assert!(
        !out.status.success(),
        "bob is still unverified → trust refused"
    );
}

#[test]
fn list_filters_by_id_and_name() {
    let cfg = TempDir::new().unwrap();
    let alice = fresh_id();
    let bob = fresh_id();
    assert!(arvolo(&cfg, &["contacts", "add", "alice", &alice])
        .status
        .success());
    assert!(arvolo(&cfg, &["contacts", "add", "bob", &bob])
        .status
        .success());

    // Filter by full id → only alice's line.
    let out = arvolo(&cfg, &["contacts", "list", &alice]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("alice"),
        "id filter should show alice: {stdout}"
    );
    assert!(
        !stdout.contains("bob"),
        "id filter should exclude bob: {stdout}"
    );

    // Filter by a name substring works too.
    let out = arvolo(&cfg, &["contacts", "list", "bo"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("bob"),
        "name filter should show bob: {stdout}"
    );
    assert!(
        !stdout.contains("alice"),
        "name filter should exclude alice: {stdout}"
    );
}

/// `verify` takes a raw id, not only a saved name.
///
/// It used to print the fingerprint, ask the security question, and *then* fail
/// with "no such contact" — the confirmation had already been given, so the user
/// had answered for nothing. Both halves of the command now resolve the same way.
#[test]
fn verify_accepts_a_raw_id() {
    let cfg = TempDir::new().unwrap();
    let id = fresh_id();

    let out = arvolo(&cfg, &["contacts", "verify", &id, "--yes"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "verifying by raw id should succeed, got: {stderr}"
    );

    // And the mark really landed: trusting it now needs no --force.
    let out = arvolo(&cfg, &["contacts", "add", "alice", &id]);
    assert!(out.status.success());
    let out = arvolo(&cfg, &["contacts", "trust", "alice"]);
    assert!(
        out.status.success(),
        "the id verified above is alice's, so trust must be allowed: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A key change clears verified *and* trusted, so the warning has to say both —
/// a user whose contact was auto-downloading has just lost that too.
#[test]
fn key_change_warning_names_both_marks() {
    let cfg = TempDir::new().unwrap();
    let (first, second) = (fresh_id(), fresh_id());

    arvolo(&cfg, &["contacts", "add", "alice", &first]);
    arvolo(&cfg, &["contacts", "verify", "alice", "--yes"]);
    assert!(arvolo(&cfg, &["contacts", "trust", "alice"])
        .status
        .success());

    let out = arvolo(&cfg, &["contacts", "add", "alice", &second]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("CHANGED"), "should warn: {stderr}");
    assert!(
        stderr.contains("verified") && stderr.contains("trusted"),
        "the warning must name both cleared marks, got: {stderr}"
    );

    // And the demotion really happened — trust is refused again without --force.
    let out = arvolo(&cfg, &["contacts", "trust", "alice"]);
    assert!(
        !out.status.success(),
        "trust must be refused after the key changed"
    );
}

/// Clearing a mark that was never set must not claim to have cleared it.
#[test]
fn unverify_does_not_claim_a_change_it_did_not_make() {
    let cfg = TempDir::new().unwrap();
    let id = fresh_id();
    arvolo(&cfg, &["contacts", "add", "alice", &id]);

    let out = arvolo(&cfg, &["contacts", "unverify", "alice"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.is_empty() && stderr.contains("nothing to clear"),
        "an unset mark should be reported as such, got stdout={stdout} stderr={stderr}"
    );

    // Set it, and the clear is then reported as a real change on stdout.
    arvolo(&cfg, &["contacts", "verify", "alice", "--yes"]);
    let out = arvolo(&cfg, &["contacts", "unverify", "alice"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("Cleared"));
}

/// Sending to an unverified recipient warns, and the warning does not stop the
/// send: the run continues past it and fails (if at all) for its own reasons.
///
/// Driven with a relay address that goes nowhere — the warning is printed before
/// any network use, so it must appear regardless, and the eventual failure must be
/// about the relay rather than about verification.
#[test]
fn sending_to_an_unverified_contact_warns_without_blocking() {
    let cfg = TempDir::new().unwrap();
    let id = fresh_id();
    arvolo(&cfg, &["contacts", "add", "alice", &id]);

    let file = cfg.path().join("note.txt");
    std::fs::write(&file, b"ciao").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_arvolo"))
        .args(["send", "alice", file.to_str().unwrap(), "--deposit"])
        .env("ARVOLO_CONFIG_DIR", cfg.path())
        .env("ARVOLO_IDENTITY", cfg.path().join("identity.key"))
        .env("ARVOLO_RELAY", "http://127.0.0.1:1")
        .env("ARVOLO_NO_WIZARD", "1")
        .output()
        .expect("run arvolo binary");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("not verified") && stderr.contains("arvolo contacts verify alice"),
        "should warn and name the fix, got: {stderr}"
    );
    assert!(
        !stderr.contains("refus") && !stderr.contains("aborted"),
        "the warning must not stop the send, got: {stderr}"
    );

    // Verified now, and the warning is gone.
    arvolo(&cfg, &["contacts", "verify", "alice", "--yes"]);
    let out = Command::new(env!("CARGO_BIN_EXE_arvolo"))
        .args(["send", "alice", file.to_str().unwrap(), "--deposit"])
        .env("ARVOLO_CONFIG_DIR", cfg.path())
        .env("ARVOLO_IDENTITY", cfg.path().join("identity.key"))
        .env("ARVOLO_RELAY", "http://127.0.0.1:1")
        .env("ARVOLO_NO_WIZARD", "1")
        .output()
        .expect("run arvolo binary");
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("not verified"),
        "a verified contact must not be warned about"
    );
}
