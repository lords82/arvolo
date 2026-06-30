//! M0.3–0.5 gates: file transfer via iroh-blobs — direct, relay fallback, resume.

use arvolo_core::transfer::{export, fetch_into, fetch_to_path, Provider, RelayChoice};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use iroh_blobs::store::mem::MemStore;

fn sample(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 + 7) as u8).collect()
}

/// M0.3: send a file directly (no relay) and verify integrity.
#[tokio::test]
async fn send_and_receive_file_direct() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("input.bin");
    let dst = dir.path().join("output.bin");
    let data = sample(1024 * 1024);
    std::fs::write(&src, &data).unwrap();

    let provider = Provider::from_path_local(&src).await.expect("provider");
    let ticket = provider.ticket();
    let provider_hash = provider.hash();

    let got_hash = fetch_to_path(&ticket, &dst, RelayChoice::Disabled)
        .await
        .expect("fetch");

    assert_eq!(got_hash, provider_hash, "hash mismatch sender/receiver");
    assert_eq!(std::fs::read(&dst).unwrap(), data, "received bytes differ");
    provider.shutdown().await;
}

/// M0.4: the only addressing the receiver gets is a relay URL (no direct IP),
/// so the transfer must traverse the relay. Proves relay fallback works.
#[tokio::test]
async fn send_and_receive_via_relay() {
    let (relay_map, relay_url, _relay_guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");

    let mk = |sk: SecretKey| {
        let relay_map = relay_map.clone();
        async move {
            Endpoint::empty_builder(RelayMode::Custom(relay_map))
                .insecure_skip_relay_cert_verify(true)
                .secret_key(sk)
                .alpns(vec![iroh_blobs::ALPN.to_vec()])
                .bind()
                .await
                .expect("bind")
        }
    };

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("input.bin");
    let dst = dir.path().join("output.bin");
    let data = sample(512 * 1024);
    std::fs::write(&src, &data).unwrap();

    // Provider, reachable via relay only.
    let provider_ep = mk(SecretKey::from_bytes(&[7u8; 32])).await;
    provider_ep.online().await;
    let provider = Provider::serve_on(provider_ep.clone(), &src)
        .await
        .expect("provider");

    // Ticket carries ONLY the relay URL — no direct addresses.
    let relay_only = EndpointAddr::new(provider_ep.id()).with_relay_url(relay_url.clone());
    let ticket = provider.ticket_via(relay_only);

    // Receiver.
    let recv_ep = mk(SecretKey::from_bytes(&[9u8; 32])).await;
    recv_ep.online().await;
    let store = MemStore::new();
    let stats = fetch_into(&store, &recv_ep, &ticket)
        .await
        .expect("fetch via relay");
    assert!(
        stats.total_bytes_read() >= data.len() as u64,
        "no data transferred"
    );
    export(&store, ticket.hash(), &dst).await.expect("export");

    assert_eq!(
        std::fs::read(&dst).unwrap(),
        data,
        "received bytes differ (relay)"
    );
    provider.shutdown().await;
    recv_ep.close().await;
}

/// M0.5: resume. Fetch a blob into a store, then fetch it again into the SAME
/// store — the second fetch transfers no payload (the content-addressed store
/// continues from what it already has). This is the resume mechanism.
#[tokio::test]
async fn resume_does_not_refetch() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("input.bin");
    let dst = dir.path().join("output.bin");
    let data = sample(4 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();

    let provider = Provider::from_path_local(&src).await.expect("provider");
    let ticket = provider.ticket();

    let recv_ep = arvolo_core::transfer::bind_endpoint(RelayChoice::Disabled)
        .await
        .expect("recv ep");
    let store = MemStore::new();

    let first = fetch_into(&store, &recv_ep, &ticket)
        .await
        .expect("first fetch");
    assert!(
        first.payload_bytes_read >= data.len() as u64,
        "first fetch incomplete"
    );

    // Second fetch into the same store: nothing new should be downloaded.
    let second = fetch_into(&store, &recv_ep, &ticket)
        .await
        .expect("second fetch");
    assert_eq!(second.payload_bytes_read, 0, "resume re-downloaded payload");

    export(&store, ticket.hash(), &dst).await.expect("export");
    assert_eq!(
        std::fs::read(&dst).unwrap(),
        data,
        "received bytes differ (resume)"
    );

    recv_ep.close().await;
    provider.shutdown().await;
}
