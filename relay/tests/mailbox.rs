//! M2 gates: zero-knowledge store-and-forward — E2E roundtrip, TTL expiry,
//! burn-after-read.

use arvolo_core::crypto::{open, seal, Identity, Sealed};
use arvolo_relay::{Deposit, Mailbox, MailboxError};

fn deposit(encapped_key: Vec<u8>, ciphertext: Vec<u8>, ttl: u64, max: u32) -> Deposit {
    Deposit {
        encapped_key,
        ciphertext,
        ttl_secs: ttl,
        max_downloads: max,
        revoke_hash: Vec::new(),
    }
}

/// Sender encrypts (E2E), deposits ciphertext while recipient is offline; later
/// the recipient claims and decrypts. The relay only ever held ciphertext.
#[test]
fn offline_delivery_end_to_end() {
    let alice = Identity::generate(); // sender
    let bob = Identity::generate(); // recipient (offline at deposit time)
    let plaintext = b"quarterly-report.pdf contents".repeat(100);

    let sealed = seal(&plaintext, &bob.public(), &alice, b"report.pdf").unwrap();

    let mb = Mailbox::in_memory().unwrap();
    let claim = mb
        .deposit(
            deposit(
                sealed.encapped_key.clone(),
                sealed.ciphertext.clone(),
                3600,
                1,
            ),
            1_000,
        )
        .unwrap();

    // The relay stores only ciphertext, never the plaintext.
    assert_ne!(sealed.ciphertext, plaintext);

    // Recipient comes online later and claims.
    let claimed = mb.fetch(&claim, 1_010).unwrap();
    let recovered = open(
        &Sealed {
            encapped_key: claimed.encapped_key,
            ciphertext: claimed.ciphertext,
        },
        &bob,
        &alice.public(),
        b"report.pdf",
    )
    .unwrap();
    assert_eq!(recovered, plaintext);

    // Burn-after-read: the entry is deleted immediately, so a second claim is
    // a clean not-found and nothing lingers on the relay.
    assert_eq!(mb.fetch(&claim, 1_011), Err(MailboxError::NotFound));
    assert!(mb.is_empty());
}

/// After the TTL passes, the blob is gone (expires and is lost by design).
#[test]
fn ttl_expiry_and_reap() {
    let mb = Mailbox::in_memory().unwrap();
    let claim = mb
        .deposit(deposit(vec![1, 2, 3], vec![9; 64], 100, 5), 1_000)
        .unwrap();

    // Still valid before expiry.
    assert!(mb.fetch(&claim, 1_050).is_ok());

    // Expired after TTL: fetch fails and the entry is dropped.
    assert_eq!(mb.fetch(&claim, 1_200), Err(MailboxError::Expired));
    assert!(mb.is_empty());

    // Reaper also clears expired entries proactively.
    let c2 = mb
        .deposit(deposit(vec![1], vec![0; 10], 10, 5), 2_000)
        .unwrap();
    assert_eq!(mb.len(), 1);
    assert_eq!(mb.reap(2_005).unwrap(), 0); // not yet expired
    assert_eq!(mb.reap(2_050).unwrap(), 1); // now expired -> removed
    assert!(mb.fetch(&c2, 2_050).is_err());
}

/// An unknown claim is a clean not-found.
#[test]
fn unknown_claim_not_found() {
    let mb = Mailbox::in_memory().unwrap();
    assert_eq!(mb.fetch("nope", 1), Err(MailboxError::NotFound));
}

/// Revocation: the sender's token (via its BLAKE3 hash) deletes the entry; a
/// wrong token is refused, and a non-revocable entry can't be revoked.
#[test]
fn revoke_with_token() {
    let token = "sender-secret-token";
    let revoke_hash = blake3::hash(token.as_bytes()).as_bytes().to_vec();
    let mb = Mailbox::in_memory().unwrap();

    let dep = Deposit {
        encapped_key: vec![1],
        ciphertext: b"revoke me".to_vec(),
        ttl_secs: 3600,
        max_downloads: 5,
        revoke_hash,
    };
    let claim = mb.deposit(dep, 1_000).unwrap();

    // Wrong token is refused and the entry stays fetchable.
    assert_eq!(mb.revoke(&claim, "wrong"), Err(MailboxError::Forbidden));
    assert!(mb.fetch(&claim, 1_010).is_ok());

    // Correct token deletes it; afterwards it's a clean not-found.
    assert!(mb.revoke(&claim, token).is_ok());
    assert_eq!(mb.fetch(&claim, 1_020), Err(MailboxError::NotFound));
    assert!(mb.is_empty());

    // A second revoke of the now-gone claim is a not-found.
    assert_eq!(mb.revoke(&claim, token), Err(MailboxError::NotFound));
}

/// A huge TTL is clamped so entries can't become effectively immortal (and the
/// `now + ttl` addition can't overflow into a negative expiry).
#[test]
fn ttl_is_clamped() {
    let mb = Mailbox::in_memory().unwrap();
    // Ask for a near-infinite TTL; the relay caps it at its max (default 30d).
    let claim = mb
        .deposit(deposit(vec![1], b"x".to_vec(), u64::MAX, 5), 1_000)
        .unwrap();
    let thirty_days = 30 * 24 * 3600;
    // Just past the clamp it must be gone, not immortal.
    assert_eq!(
        mb.fetch(&claim, 1_000 + thirty_days + 1),
        Err(MailboxError::Expired)
    );
}

/// A non-revocable entry (deposited without a revoke hash) can't be revoked.
#[test]
fn non_revocable_entry_is_forbidden() {
    let mb = Mailbox::in_memory().unwrap();
    let claim = mb
        .deposit(deposit(vec![1], b"no revoke".to_vec(), 3600, 1), 1_000)
        .unwrap();
    assert_eq!(mb.revoke(&claim, "anything"), Err(MailboxError::Forbidden));
    assert!(mb.fetch(&claim, 1_010).is_ok());
}

/// Deposited blobs survive a relay restart (SQLite + files on disk).
#[test]
fn persists_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("relay.db");
    let blobs = dir.path().join("blobs");

    let claim = {
        let mb = Mailbox::open(&db, &blobs).unwrap();
        mb.deposit(deposit(vec![5, 6], b"persist me".to_vec(), 3600, 3), 1_000)
            .unwrap()
    }; // mailbox dropped — simulates a process restart

    let mb = Mailbox::open(&db, &blobs).unwrap();
    let claimed = mb.fetch(&claim, 1_010).unwrap();
    assert_eq!(claimed.ciphertext, b"persist me");
    assert_eq!(claimed.encapped_key, vec![5, 6]);
}

/// Per-session relay-offload meter: cumulative within the TTL window, isolated by
/// swarm id, reads zero once the window lapses (so a genuinely new transfer of the
/// same file starts fresh), and the reaper drops the stale row.
#[test]
fn session_offload_meter_accumulates_and_expires() {
    let mb = Mailbox::in_memory().unwrap();
    let (a, b) = ("swarm-a", "swarm-b");

    // Unknown session starts at zero.
    assert_eq!(mb.session_bytes(a, 1_000), 0);

    // Bytes accumulate within the live window (expires_at = 2_000, in the future).
    mb.add_session_bytes(a, 100, 2_000).unwrap();
    mb.add_session_bytes(a, 50, 2_000).unwrap();
    assert_eq!(mb.session_bytes(a, 1_000), 150);

    // Sessions are isolated by swarm id.
    assert_eq!(mb.session_bytes(b, 1_000), 0);

    // Once the window has lapsed the tally reads as zero…
    assert_eq!(mb.session_bytes(a, 2_001), 0);

    // …and the reaper drops the stale row, so a re-send starts a fresh count.
    mb.reap_session_bytes(2_001);
    mb.add_session_bytes(a, 10, 5_000).unwrap();
    assert_eq!(mb.session_bytes(a, 3_000), 10);
}
