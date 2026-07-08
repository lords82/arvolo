//! End-to-end multi-device sync-note transport over a real relay: two devices
//! that share one identity exchange an encrypted address-book snapshot through
//! their common inbox slot, and the offer path leaves the note untouched.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::crypto::Identity;
use arvolo_core::presence::{decode_sync_note, encode_sync_note, InboxSubscription};
use arvolo_core::sync::{
    decrypt_snapshot, encrypt_snapshot, snapshot_key, ContactEntry, Lamport, SyncNote, SyncSnapshot,
};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

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

fn sample_snapshot() -> SyncSnapshot {
    SyncSnapshot {
        lamport: 3,
        device: [1u8; 16],
        contacts: vec![ContactEntry {
            name: "alice".into(),
            pubkey: "aaaa".into(),
            clock: Lamport {
                counter: 1,
                device: [1u8; 16],
            },
            deleted: false,
        }],
        verified: vec![],
        trusted: vec![],
        seen: vec![],
        names: vec![],
    }
}

#[tokio::test]
async fn sync_note_travels_between_devices_sharing_identity() {
    let relay = spawn_relay().await;

    // Two devices, one shared identity (same secret bytes).
    let identity = Identity::generate();
    let dev_a = Identity::from_secret_bytes(&identity.secret_bytes()).unwrap();
    let dev_b = Identity::from_secret_bytes(&identity.secret_bytes()).unwrap();
    let key = snapshot_key(&identity.secret_bytes());

    // Device A publishes an encrypted snapshot to the shared inbox slot.
    let snap = sample_snapshot();
    let blob = encrypt_snapshot(&key, &snap).unwrap();
    let body = encode_sync_note(&SyncNote::Snapshot { blob }).unwrap();
    let sub_a = InboxSubscription::new(relay.clone(), &dev_a);
    let posted_id = sub_a.post_raw(body, Some(3600)).await.expect("post note");
    assert!(!posted_id.is_empty());

    // Device B reads the raw cell, finds the note, decrypts the snapshot.
    let sub_b = InboxSubscription::new(relay.clone(), &dev_b);
    let items = sub_b.raw_items(0).await.expect("read inbox");
    assert_eq!(items.len(), 1, "device B sees the one sync note");
    let note = decode_sync_note(&items[0].blob).expect("it is a sync note");
    let SyncNote::Snapshot { blob } = note else {
        panic!("expected an inline snapshot note")
    };
    let got = decrypt_snapshot(&key, &blob).expect("device B decrypts with shared key");
    assert_eq!(got, snap, "snapshot survives the round-trip");

    // The offer poll must NOT consume the sync note (readers never ack it).
    let offers = sub_b.poll_wait(0).await.expect("offer poll");
    assert!(offers.is_empty(), "a sync note is not surfaced as an offer");
    let still = sub_b.raw_items(0).await.expect("re-read inbox");
    assert_eq!(still.len(), 1, "offer poll left the sync note in place");

    // A stranger (different identity) cannot decrypt it.
    let stranger_key = snapshot_key(&Identity::generate().secret_bytes());
    assert!(decrypt_snapshot(&stranger_key, &blob).is_err());

    // The owner can clear it (writer-side cleanup).
    sub_b.ack(&posted_id).await.expect("ack");
    let after = sub_b.raw_items(0).await.expect("read after ack");
    assert!(after.is_empty(), "note removed after owner ack");
}

// The mutable-cell invariant `sync now` relies on: a writer, after merging the
// notes currently in the cell, publishes a fresh full snapshot AND deletes the
// notes it merged — so the slot converges to a single current snapshot, and a
// later reader (another device) sees exactly that latest snapshot.
#[tokio::test]
async fn sync_cell_writer_supersedes_and_cleans_up() {
    let relay = spawn_relay().await;

    let identity = Identity::generate();
    let dev_a = Identity::from_secret_bytes(&identity.secret_bytes()).unwrap();
    let dev_b = Identity::from_secret_bytes(&identity.secret_bytes()).unwrap();
    let dev_c = Identity::from_secret_bytes(&identity.secret_bytes()).unwrap();
    let key = snapshot_key(&identity.secret_bytes());

    // Device A publishes snapshot v1.
    let v1 = sample_snapshot();
    let sub_a = InboxSubscription::new(relay.clone(), &dev_a);
    let id1 = sub_a
        .post_raw(
            encode_sync_note(&SyncNote::Snapshot {
                blob: encrypt_snapshot(&key, &v1).unwrap(),
            })
            .unwrap(),
            Some(3600),
        )
        .await
        .unwrap();

    // Device B does a writer round: read the cell, merge (here: build v2), publish
    // the fresh snapshot, then delete the note(s) it merged.
    let sub_b = InboxSubscription::new(relay.clone(), &dev_b);
    let items = sub_b.raw_items(0).await.unwrap();
    assert_eq!(items.len(), 1, "B sees A's note");
    // (B would decrypt+merge here; the merge itself is covered by book.rs tests.)
    let mut v2 = sample_snapshot();
    v2.lamport = 9;
    v2.contacts.push(ContactEntry {
        name: "bob".into(),
        pubkey: "bbbb".into(),
        clock: Lamport {
            counter: 8,
            device: [2u8; 16],
        },
        deleted: false,
    });
    sub_b
        .post_raw(
            encode_sync_note(&SyncNote::Snapshot {
                blob: encrypt_snapshot(&key, &v2).unwrap(),
            })
            .unwrap(),
            Some(3600),
        )
        .await
        .unwrap();
    for it in &items {
        sub_b.ack(&it.id).await.unwrap(); // delete the merged note(s)
    }
    assert_ne!(id1, "", "posted id is non-empty");

    // The cell now holds exactly one note — the current snapshot.
    let after = sub_b.raw_items(0).await.unwrap();
    assert_eq!(after.len(), 1, "cell converged to a single current note");

    // A third device pulling the cell decrypts exactly the latest snapshot.
    let sub_c = InboxSubscription::new(relay.clone(), &dev_c);
    let seen = sub_c.raw_items(0).await.unwrap();
    assert_eq!(seen.len(), 1);
    let note = decode_sync_note(&seen[0].blob).expect("sync note");
    let SyncNote::Snapshot { blob } = note else {
        panic!("expected inline snapshot")
    };
    let got = decrypt_snapshot(&key, &blob).unwrap();
    assert_eq!(
        got, v2,
        "third device sees the latest (superseding) snapshot"
    );
    assert!(
        got.contacts.iter().any(|c| c.name == "bob"),
        "the superseding snapshot carries the merged addition"
    );
}
