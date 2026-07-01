//! End-to-end coverage of the core transfer flow: prepare_send + recv_chunked
//! over a local (relay-disabled) path, plus cancellation.

use std::sync::{Arc, Mutex};

use arvolo_core::flow::{self, RecvEvent};
use arvolo_core::transfer::RelayChoice;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn send_then_recv_roundtrip_emits_events() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let out = dir.path().join("out.bin");
    // ~24 MiB -> 2 chunks (16 + 8), last one short.
    let data: Vec<u8> = (0..24 * 1024 * 1024).map(|i| (i * 17 + 3) as u8).collect();
    std::fs::write(&src, &data).unwrap();

    // Serve (no relay) and grab the ticket.
    let session = flow::prepare_send(&src, "src.bin", false, None, RelayChoice::Disabled)
        .await
        .expect("prepare_send");
    let ticket = session.ticket.clone();
    assert_eq!(session.chunks, 2);
    assert!(!session.has_relay);

    let send_cancel = CancellationToken::new();
    let serve = {
        let c = send_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };

    // Receive, collecting progress events.
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let saved = flow::recv_chunked(
        &ticket,
        Some(out.clone()),
        RelayChoice::Disabled,
        CancellationToken::new(),
        move |e| ev.lock().unwrap().push(e),
    )
    .await
    .expect("recv_chunked");

    // Integrity.
    assert_eq!(std::fs::read(&saved).unwrap(), data);

    // Event shape: Started first, one Chunk per chunk, Saved last.
    let events = events.lock().unwrap();
    assert!(
        matches!(
            events.first(),
            Some(RecvEvent::Started { total: 2, resuming_from: 0, .. })
        ),
        "first event is Started"
    );
    let chunk_events = events
        .iter()
        .filter(|e| matches!(e, RecvEvent::Chunk { .. }))
        .count();
    assert_eq!(chunk_events, 2, "one Chunk event per chunk");
    assert!(
        matches!(events.last(), Some(RecvEvent::Saved { .. })),
        "last event is Saved"
    );

    send_cancel.cancel();
    let _ = serve.await;
}

#[tokio::test]
async fn recv_cancelled_returns_without_saving() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let out = dir.path().join("out.bin");
    let data: Vec<u8> = (0..24 * 1024 * 1024).map(|i| (i * 5 + 1) as u8).collect();
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(&src, "src.bin", false, None, RelayChoice::Disabled)
        .await
        .expect("prepare_send");
    let ticket = session.ticket.clone();
    let send_cancel = CancellationToken::new();
    let serve = {
        let c = send_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };

    // Cancel before any chunk is fetched: recv returns early, no Saved event,
    // and the output is not the complete file.
    let cancel = CancellationToken::new();
    cancel.cancel();
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let path = flow::recv_chunked(
        &ticket,
        Some(out.clone()),
        RelayChoice::Disabled,
        cancel,
        move |e| ev.lock().unwrap().push(e),
    )
    .await
    .expect("recv_chunked returns Ok on cancel");

    let events = events.lock().unwrap();
    assert!(
        !events.iter().any(|e| matches!(e, RecvEvent::Saved { .. })),
        "cancelled recv must not emit Saved"
    );
    assert_ne!(
        std::fs::read(&path).unwrap().len(),
        data.len(),
        "cancelled recv must not have written the whole file"
    );

    send_cancel.cancel();
    let _ = serve.await;
}

#[tokio::test]
async fn archive_roundtrip_packs_and_extracts() {
    let dir = tempfile::tempdir().unwrap();
    // A source folder with a nested file.
    let src = dir.path().join("folder");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("a.txt"), b"hello alpha").unwrap();
    std::fs::write(src.join("sub/b.bin"), vec![7u8; 1000]).unwrap();

    // Pack it, then serve the archive.
    let tar_path = dir.path().join("payload.tar");
    flow::pack_tar(&[src.clone()], &tar_path).unwrap();
    let session = flow::prepare_send(&tar_path, "folder", true, None, RelayChoice::Disabled)
        .await
        .expect("prepare_send");
    let ticket = session.ticket.clone();
    let send_cancel = CancellationToken::new();
    let serve = {
        let c = send_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };

    // Receive: the archive is unpacked into the output dir.
    let outdir = dir.path().join("received");
    let saved = flow::recv_chunked(
        &ticket,
        Some(outdir.clone()),
        RelayChoice::Disabled,
        CancellationToken::new(),
        |_| {},
    )
    .await
    .expect("recv_chunked");
    assert_eq!(saved, outdir);
    assert_eq!(std::fs::read(outdir.join("folder/a.txt")).unwrap(), b"hello alpha");
    assert_eq!(
        std::fs::read(outdir.join("folder/sub/b.bin")).unwrap(),
        vec![7u8; 1000]
    );

    send_cancel.cancel();
    let _ = serve.await;
}
