//! One disk, one budget.
//!
//! The mailbox and the seed path used to be metered separately and in different
//! units — blobs in bytes, seeded chunks in *rows* — over the same volume. So
//! neither could see the other filling it, and the row cap (50k) stood for some
//! 800 GB at the nominal chunk size, which is not a disk guard on any ordinary
//! machine: what actually arrived first was ENOSPC. These two tests pin the two
//! directions of the fix, one each way round.
//!
//! Own test binary: the budget is a process-global env var.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::chunked::CHUNK_SIZE;
use arvolo_core::flow;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Deposit, Mailbox};
use tokio_util::sync::CancellationToken;

/// Room for one nominal chunk and a little over — small enough that a single
/// deposit and a single seeded chunk cannot both fit.
const BUDGET: u64 = 20 * 1024 * 1024;

/// Hands back a handle on the same mailbox the server is using, so a test can put
/// bytes in one path and ask the other what it thinks is left.
async fn spawn_relay() -> (String, Arc<Mailbox>) {
    let dir = tempfile::tempdir().unwrap();
    let node = BlobNode::spawn(dir.path(), RelayChoice::Disabled)
        .await
        .expect("blob node");
    let mailbox = Arc::new(Mailbox::in_memory().expect("mailbox"));
    let state = AppState::new(mailbox.clone(), Arc::new(node));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _dir = dir;
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), mailbox)
}

fn now() -> u64 {
    arvolo_relay::now_unix()
}

/// Mailbox bytes count against seeding. Before, a relay whose disk was entirely
/// full of deposits would still cheerfully agree to pull chunks onto it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_mailbox_leaves_no_room_to_seed() {
    std::env::set_var("ARVOLO_MAX_TOTAL_BLOB_BYTES", BUDGET.to_string());
    let (relay, mailbox) = spawn_relay().await;

    // 15 MiB of mailbox blob: on its own, well inside the budget.
    mailbox
        .deposit(
            Deposit {
                ciphertext: vec![7u8; 15 * 1024 * 1024],
                encapped_key: Vec::new(),
                ttl_secs: 3600,
                max_downloads: 1,
                revoke_hash: Vec::new(),
            },
            now(),
        )
        .expect("deposit");
    assert!(mailbox.disk_bytes() >= 15 * 1024 * 1024);

    // A one-chunk payload now cannot be parked: 15 MiB + 16 MiB is over budget.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    std::fs::write(&src, vec![3u8; 4 * 1024 * 1024]).unwrap();
    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.clone()),
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send");
    assert_eq!(session.chunks, 1);

    let parked = session
        .park_on_relay(&CancellationToken::new(), |_| {})
        .await;
    assert!(
        !parked,
        "the disk is spoken for by the mailbox — seeding onto it must be refused"
    );
    assert_eq!(
        mailbox.seeded_bytes(),
        0,
        "and nothing should have been pulled"
    );
    session.shutdown().await;
}

/// And the other way: seeded chunks count against a deposit. This is the direction
/// that matters most now, because parking makes seeding the ordinary path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seeded_chunks_leave_no_room_to_deposit() {
    std::env::set_var("ARVOLO_MAX_TOTAL_BLOB_BYTES", BUDGET.to_string());
    let (relay, mailbox) = spawn_relay().await;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    std::fs::write(&src, vec![9u8; 4 * 1024 * 1024]).unwrap();
    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.clone()),
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send");
    assert!(
        session
            .park_on_relay(&CancellationToken::new(), |_| {})
            .await,
        "one chunk fits in the budget"
    );
    assert_eq!(
        mailbox.seeded_bytes(),
        CHUNK_SIZE as u64,
        "the seeded chunk has to show up in the disk figure — this was the blind spot"
    );

    // The control: the very same deposit, against a relay with the same budget and
    // nothing seeded. Without this the refusal below proves only that something
    // said no — and a deposit has several unrelated ways of saying it.
    let (empty_relay, _mb) = spawn_relay().await;
    assert!(
        deposit_8mib(&empty_relay).await.is_success(),
        "8 MiB fits in a 20 MiB budget when the relay is holding nothing"
    );

    // On the relay that now holds one nominal 16 MiB chunk, the same 8 MiB no
    // longer fits in what is left. Refused as too large *for the remaining budget*
    // rather than 507, because the streaming deposit bounds itself as it writes —
    // the point is that the seeded chunk is what shrank the room.
    let status = deposit_8mib(&relay).await;
    assert_eq!(
        status,
        reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        "a deposit must see the chunks the relay is already holding"
    );

    session.shutdown().await;
}

async fn deposit_8mib(relay: &str) -> reqwest::StatusCode {
    reqwest::Client::new()
        .post(format!("{relay}/v1/deposit?ttl=3600&max=1"))
        // Empty = a link deposit (no HPKE recipient); the header itself is required.
        .header("x-arvolo-encapped-key", "")
        .body(vec![1u8; 8 * 1024 * 1024])
        .send()
        .await
        .expect("deposit")
        .status()
}
