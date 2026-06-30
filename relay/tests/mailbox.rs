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
