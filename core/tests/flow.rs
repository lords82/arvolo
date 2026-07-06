//! End-to-end coverage of the core transfer flow: prepare_send + recv_chunked
//! over a local (relay-disabled) path, plus cancellation.

use std::sync::{Arc, Mutex};

use arvolo_core::flow::{self, RecvEvent};
use arvolo_core::transfer::RelayChoice;
use tokio_util::sync::CancellationToken;

/// Fase 4 enabling property: two devices that share one identity recover the same
/// content key from a `--to` sealed delivery, so they re-seal byte-identical
/// pieces and derive the same `swarm_id` — the basis for co-swarming a sealed
/// transfer. A stranger can't open the key, so can't join.
#[test]
fn shared_identity_devices_co_swarm_a_sealed_transfer() {
    use arvolo_core::crypto::{open, random_chunk_key, seal, seal_chunk, Identity};
    use arvolo_core::hash::Hash;
    use arvolo_core::swarm::swarm_id;

    let sender = Identity::generate();
    let account = Identity::generate(); // the shared identity across devices
    let dev_a = Identity::from_secret_bytes(&account.secret_bytes()).unwrap();
    let dev_b = Identity::from_secret_bytes(&account.secret_bytes()).unwrap();
    let stranger = Identity::generate();

    let content_key = random_chunk_key();
    let aad = b"test/content-key";
    let sealed = seal(&content_key, &account.public(), &sender, aad).unwrap();

    // Both devices unseal the identical content key.
    let ka: [u8; 32] = open(&sealed, &dev_a, &sender.public(), aad)
        .unwrap()
        .try_into()
        .unwrap();
    let kb: [u8; 32] = open(&sealed, &dev_b, &sender.public(), aad)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(ka, kb);
    assert_eq!(ka, content_key);

    // Deterministic re-seal → identical piece hashes → identical swarm id.
    let chunks: Vec<&[u8]> = vec![b"chunk-zero-data.", b"chunk-one-data!!"];
    let swarm_id_for = |k: &[u8; 32]| {
        let hashes: Vec<Hash> = chunks
            .iter()
            .enumerate()
            .map(|(i, p)| Hash::new(seal_chunk(k, i as u32, chunks.len() as u32, p).unwrap()))
            .collect();
        swarm_id(&hashes, 32)
    };
    assert_eq!(
        swarm_id_for(&ka),
        swarm_id_for(&kb),
        "same content key → identical pieces → same swarm"
    );

    // A stranger cannot open the sealed key, so cannot compute the pieces/join.
    assert!(open(&sealed, &stranger, &sender.public(), aad).is_err());
}

#[tokio::test]
async fn send_then_recv_roundtrip_emits_events() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let out = dir.path().join("out.bin");
    // ~24 MiB -> 2 chunks (16 + 8), last one short.
    let data: Vec<u8> = (0..24 * 1024 * 1024).map(|i| (i * 17 + 3) as u8).collect();
    std::fs::write(&src, &data).unwrap();

    // Serve (no relay) and grab the ticket.
    let session = flow::prepare_send(&src, "src.bin", false, None, None, RelayChoice::Disabled)
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
        None,
        RelayChoice::Disabled,
        CancellationToken::new(),
        move |e| ev.lock().unwrap().push(e),
    )
    .await
    .expect("recv_chunked");

    // Integrity.
    assert_eq!(std::fs::read(&saved).unwrap(), data);

    // Event shape: anonymous Sender first (plain ticket), then Started, one Chunk
    // per chunk, Saved last.
    let events = events.lock().unwrap().clone();
    assert!(
        matches!(events.first(), Some(RecvEvent::Sender { id: None })),
        "first event is an anonymous Sender for a plain ticket"
    );
    assert!(
        matches!(
            events.get(1),
            Some(RecvEvent::Started {
                total: 2,
                resuming_from: 0,
                ..
            })
        ),
        "Started follows the Sender event"
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

    let session = flow::prepare_send(&src, "src.bin", false, None, None, RelayChoice::Disabled)
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
        None,
        RelayChoice::Disabled,
        cancel,
        move |e| ev.lock().unwrap().push(e),
    )
    .await
    .expect("recv_chunked returns Ok on cancel");

    let events = events.lock().unwrap().clone();
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
    flow::pack_tar(std::slice::from_ref(&src), &tar_path).unwrap();
    let session = flow::prepare_send(&tar_path, "folder", true, None, None, RelayChoice::Disabled)
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
        None,
        RelayChoice::Disabled,
        CancellationToken::new(),
        |_| {},
    )
    .await
    .expect("recv_chunked");
    assert_eq!(saved, outdir);
    assert_eq!(
        std::fs::read(outdir.join("folder/a.txt")).unwrap(),
        b"hello alpha"
    );
    assert_eq!(
        std::fs::read(outdir.join("folder/sub/b.bin")).unwrap(),
        vec![7u8; 1000]
    );

    send_cancel.cancel();
    let _ = serve.await;
}

#[test]
fn pack_tar_is_deterministic_and_content_sensitive() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("folder");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("a.txt"), b"hello alpha").unwrap();
    std::fs::write(src.join("z.txt"), b"zed").unwrap();
    std::fs::write(src.join("sub/b.bin"), vec![7u8; 1000]).unwrap();

    // Same inputs pack to byte-identical bytes — the property resume relies on.
    let t1 = dir.path().join("1.tar");
    let t2 = dir.path().join("2.tar");
    flow::pack_tar(std::slice::from_ref(&src), &t1).unwrap();
    flow::pack_tar(std::slice::from_ref(&src), &t2).unwrap();
    assert_eq!(
        std::fs::read(&t1).unwrap(),
        std::fs::read(&t2).unwrap(),
        "identical inputs must pack identically"
    );

    // A changed file changes the bytes (so resume detects it via hash mismatch).
    std::fs::write(src.join("a.txt"), b"hello ALPHA").unwrap();
    let t3 = dir.path().join("3.tar");
    flow::pack_tar(std::slice::from_ref(&src), &t3).unwrap();
    assert_ne!(
        std::fs::read(&t1).unwrap(),
        std::fs::read(&t3).unwrap(),
        "changed content must change the archive"
    );
}

#[tokio::test]
async fn sealed_to_recipient_only_intended_can_decrypt() {
    use arvolo_core::crypto::Identity;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("s.bin");
    let data = vec![9u8; 20 * 1024 * 1024]; // 2 chunks
    std::fs::write(&src, &data).unwrap();

    let sender_id = Identity::generate();
    let recipient = Identity::generate();
    let stranger = Identity::generate();

    // Seal the content key to `recipient`, authenticated as `sender_id`.
    let session = flow::prepare_send(
        &src,
        "s.bin",
        false,
        Some((&sender_id, &recipient.public())),
        None,
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send");
    let ticket = session.ticket.clone();
    let send_cancel = CancellationToken::new();
    let serve = {
        let c = send_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };

    // The intended recipient recovers the file, and learns the verified sender.
    let out = dir.path().join("out.bin");
    let seen_sender = Arc::new(Mutex::new(None));
    let ss = seen_sender.clone();
    flow::recv_chunked(
        &ticket,
        Some(out.clone()),
        Some(&recipient),
        RelayChoice::Disabled,
        CancellationToken::new(),
        move |e| {
            if let RecvEvent::Sender { id } = e {
                *ss.lock().unwrap() = Some(id);
            }
        },
    )
    .await
    .expect("recipient decrypts");
    assert_eq!(std::fs::read(&out).unwrap(), data);
    assert_eq!(
        seen_sender.lock().unwrap().clone(),
        Some(Some(sender_id.public().to_bytes())),
        "recv surfaces the HPKE-verified sender id"
    );

    // A stranger cannot open the sealed content key.
    let stranger_res = flow::recv_chunked(
        &ticket,
        Some(dir.path().join("no.bin")),
        Some(&stranger),
        RelayChoice::Disabled,
        CancellationToken::new(),
        |_| {},
    )
    .await;
    assert!(stranger_res.is_err(), "a non-recipient must not decrypt");

    send_cancel.cancel();
    let _ = serve.await;
}

#[tokio::test]
async fn parallel_fetch_preserves_integrity_and_order() {
    // 3 chunks with a window of 2 forces refill and out-of-order completion,
    // yet chunks must be committed to disk in order (byte-identical output).
    std::env::set_var("ARVOLO_CONCURRENCY", "2");
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.bin");
    let out = dir.path().join("big.out");
    // ~40 MiB -> 3 chunks (16 + 16 + 8).
    let data: Vec<u8> = (0..40 * 1024 * 1024u64)
        .map(|i| (i * 131 + 7) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(&src, "big.bin", false, None, None, RelayChoice::Disabled)
        .await
        .expect("prepare_send");
    assert_eq!(session.chunks, 3);
    let ticket = session.ticket.clone();
    let send_cancel = CancellationToken::new();
    let serve = {
        let c = send_cancel.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };

    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let saved = flow::recv_chunked(
        &ticket,
        Some(out.clone()),
        None,
        RelayChoice::Disabled,
        CancellationToken::new(),
        move |e| ev.lock().unwrap().push(e),
    )
    .await
    .expect("recv_chunked");
    assert_eq!(std::fs::read(&saved).unwrap(), data);

    // Chunk events are emitted in ascending (commit) order, one per chunk.
    let events = events.lock().unwrap().clone();
    let idxs: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            RecvEvent::Chunk { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(idxs, vec![0, 1, 2], "chunks committed in order");

    send_cancel.cancel();
    let _ = serve.await;
    std::env::remove_var("ARVOLO_CONCURRENCY");
}

#[tokio::test]
async fn resume_after_truncation_with_concurrency() {
    use std::io::Read;
    std::env::set_var("ARVOLO_CONCURRENCY", "2");
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("r.bin");
    let out = dir.path().join("r.out");
    let data: Vec<u8> = (0..40 * 1024 * 1024u64)
        .map(|i| (i * 29 + 5) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    // Full receive once.
    let session = flow::prepare_send(&src, "r.bin", false, None, None, RelayChoice::Disabled)
        .await
        .unwrap();
    let ticket = session.ticket.clone();
    let sc = CancellationToken::new();
    let serve = {
        let c = sc.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };
    flow::recv_chunked(
        &ticket,
        Some(out.clone()),
        None,
        RelayChoice::Disabled,
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), data);
    sc.cancel();
    let _ = serve.await;

    // Truncate to exactly one chunk (16 MiB), then resume from a fresh server.
    let f = std::fs::OpenOptions::new().write(true).open(&out).unwrap();
    f.set_len(16 * 1024 * 1024).unwrap();
    drop(f);

    let session2 = flow::prepare_send(&src, "r.bin", false, None, None, RelayChoice::Disabled)
        .await
        .unwrap();
    let ticket2 = session2.ticket.clone();
    let sc2 = CancellationToken::new();
    let serve2 = {
        let c = sc2.clone();
        tokio::spawn(async move { session2.serve(c, |_| {}).await })
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    flow::recv_chunked(
        &ticket2,
        Some(out.clone()),
        None,
        RelayChoice::Disabled,
        CancellationToken::new(),
        move |e| ev.lock().unwrap().push(e),
    )
    .await
    .unwrap();

    // Byte-identical, and only the missing chunks (1, 2) were fetched, in order.
    let mut got = Vec::new();
    std::fs::File::open(&out)
        .unwrap()
        .read_to_end(&mut got)
        .unwrap();
    assert_eq!(got, data, "resumed output is byte-identical");
    let events = events.lock().unwrap().clone();
    assert!(events.iter().any(|e| matches!(
        e,
        RecvEvent::Started {
            resuming_from: 1,
            ..
        }
    )));
    let idxs: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            RecvEvent::Chunk { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(idxs, vec![1, 2], "only missing chunks fetched, in order");

    sc2.cancel();
    let _ = serve2.await;
    std::env::remove_var("ARVOLO_CONCURRENCY");
}
