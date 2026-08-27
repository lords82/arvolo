//! On-disk reconciliation: files that lost (or never got) their row are the one
//! kind of leftover the row-driven reaper cannot see, so they get their own pass.

use std::path::{Path, PathBuf};

use arvolo_relay::{Deposit, Mailbox};

fn deposit(ttl: u64) -> Deposit {
    Deposit {
        encapped_key: vec![1; 32],
        ciphertext: b"ciphertext".to_vec(),
        ttl_secs: ttl,
        max_downloads: 1,
        revoke_hash: Vec::new(),
    }
}

/// The blob directory of an in-memory mailbox (derived: it is a temp dir).
fn blob_dir(mb: &Mailbox) -> PathBuf {
    mb.blob_path("x").parent().unwrap().to_path_buf()
}

fn touch(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
}

/// A ciphertext file whose metadata row never landed (the deposit crashed between
/// the write and the insert) is invisible to `reap` — the sweep is what frees it.
#[test]
fn orphaned_blob_is_swept() {
    let mb = Mailbox::in_memory().unwrap();
    let dir = blob_dir(&mb);
    let orphan = dir.join("neverrecorded.bin");
    touch(&orphan, b"stranded ciphertext");

    // grace 0: every file is old enough to judge.
    assert_eq!(mb.sweep_orphan_blobs(0), 1);
    assert!(!orphan.exists());
}

/// A live deposit is not an orphan, however long it has been sitting there.
#[test]
fn live_blob_survives_the_sweep() {
    let mb = Mailbox::in_memory().unwrap();
    let claim = mb.deposit(deposit(3600), 1_000).unwrap();
    assert!(mb.blob_path(&claim).exists());

    assert_eq!(mb.sweep_orphan_blobs(0), 0);
    assert!(mb.blob_path(&claim).exists());
}

/// The grace period is the whole reason a deposit still streaming in is safe:
/// its row appears only when the body ends, so a young file is never judged.
#[test]
fn grace_period_protects_a_file_being_written() {
    let mb = Mailbox::in_memory().unwrap();
    let dir = blob_dir(&mb);
    let in_flight = dir.join("stillstreaming.bin");
    touch(&in_flight, b"first bytes of a deposit in progress");

    assert_eq!(mb.sweep_orphan_blobs(3600), 0);
    assert!(in_flight.exists());
}

/// Foreign files in the blob directory are none of the sweep's business.
#[test]
fn sweep_ignores_files_it_did_not_write() {
    let mb = Mailbox::in_memory().unwrap();
    let dir = blob_dir(&mb);
    let stranger = dir.join("README.txt");
    touch(&stranger, b"not a blob");

    assert_eq!(mb.sweep_orphan_blobs(0), 0);
    assert!(stranger.exists());
}

/// Same reconciliation for the backfill store: a chunk file with no `seeded` row
/// is exactly what the TTL backstop cannot reach, since it walks rows.
#[test]
fn orphaned_chunk_is_swept_and_seeded_one_is_kept() {
    let mb = Mailbox::in_memory().unwrap();
    let store = blob_dir(&mb).join("blobstore");
    std::fs::create_dir_all(&store).unwrap();

    let seeded_hash = "a".repeat(64);
    let orphan_hash = "b".repeat(64);
    let not_a_chunk = store.join("endpoint-state.json");
    touch(&store.join(&seeded_hash), b"seeded chunk");
    touch(&store.join(&orphan_hash), b"stranded chunk");
    touch(&not_a_chunk, b"{}");

    mb.record_seed("token", &seeded_hash, u64::MAX).unwrap();

    assert_eq!(mb.sweep_orphan_chunks(&store, 0), 1);
    assert!(store.join(&seeded_hash).exists());
    assert!(!store.join(&orphan_hash).exists());
    assert!(not_a_chunk.exists());
}
