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
