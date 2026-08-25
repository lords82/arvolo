//! Parking twice must not cost twice.
//!
//! The relay skips chunk files it already holds, so a repeated seed never
//! re-fetched anything — but the handler charges the transfer's offload meter for
//! every hash in the request *before* it gets that far. So a receiver that
//! connected and dropped repeatedly could burn a transfer's whole relay budget
//! without the relay storing one new byte, and the 402 that followed cut the
//! transfer off from a relay already holding everything it needed. Asking the
//! relay what it has, and seeding only the difference, is what removes that.
//!
//! Own test binary: the offload cap is a process-global env var.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::chunked::{ChunkTicket, SeedRequest, CHUNK_SIZE};
use arvolo_core::flow;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

/// Exactly enough budget for one full pass over a 3-chunk payload, so a second
/// pass that re-charged the meter would be refused and one that does not, is not.
const CHUNKS: u64 = 3;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn parking_again_does_not_spend_the_offload_budget_again() {
    std::env::set_var(
        "ARVOLO_MAX_SESSION_RELAY_BYTES",
        (CHUNKS * CHUNK_SIZE as u64).to_string(),
    );
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data: Vec<u8> = (0..40 * 1024 * 1024u64)
        .map(|i| (i * 131 + 7) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

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
    assert_eq!(session.chunks as u64, CHUNKS);
    let ticket = ChunkTicket::decode(&session.ticket).expect("decode ticket");

    let cancel = CancellationToken::new();
    assert!(
        session
            .park_on_relay(flow::PARK_TTL_SECS, &cancel, |_| {})
            .await
            .is_some(),
        "first park should fit exactly inside the budget"
    );

    // The meter is now at the cap: seeding the same pieces again the old way — one
    // request naming every hash — is refused, which is precisely what repeated
    // drops used to do to a transfer.
    let req = SeedRequest {
        sender: ticket.providers[0].clone(),
        chunks: ticket.chunks.clone(),
        token: ticket.relay.as_ref().unwrap().token.clone(),
        swarm_id: arvolo_core::swarm::swarm_id(&ticket.chunks, ticket.total_size),
    };
    let resp = reqwest::Client::new()
        .post(format!("{relay}/v1/seed"))
        .body(req.encode().unwrap())
        .send()
        .await
        .expect("seed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::PAYMENT_REQUIRED,
        "the budget should be spent — otherwise this test proves nothing below"
    );

    // Parking again asks first, finds the relay already holding all three, and so
    // spends nothing. Before the subtraction this returned false: the seed request
    // took the same 402 as the raw one above, from a relay holding the whole file.
    assert!(
        session
            .park_on_relay(flow::PARK_TTL_SECS, &cancel, |_| {})
            .await
            .is_some(),
        "a second park must be free — the relay already has every piece"
    );

    session.shutdown().await;
}
