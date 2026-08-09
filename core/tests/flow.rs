//! End-to-end coverage of the core transfer flow: prepare_send + recv_chunked
//! over a local (relay-disabled) path, plus cancellation.

// The `HEAVY` guard below is deliberately held across awaits, which is what
// `await_holding_lock` exists to catch — and what it warns about cannot happen
// here. The lint's hazard is a second task on the *same* runtime blocking on a
// lock the suspended task holds; nothing takes this lock inside a test's runtime,
// and `#[tokio::test]` gives each test a current-thread runtime of its own. What
// blocks is the *other test's thread*, waiting its turn, which is the entire point
// of the guard. An async mutex would make the wait cooperative and so let the
// tests interleave again — the opposite of what is wanted.
#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, Mutex};

use arvolo_core::flow::{self, RecvEvent, RecvOutcome, SendEvent};
use arvolo_core::transfer::RelayChoice;
use tokio_util::sync::CancellationToken;

/// Serializes the tests that run a real transfer.
///
/// Two reasons, and the second is the one that bites. Three of them tune
/// `ARVOLO_CONCURRENCY`, which is process-global: run side by side, one removes it
/// while another is mid-fetch, and each stops testing the window it says it does.
/// And every one of them stands up iroh endpoints and moves tens of megabytes over
/// them — ten at once on one machine oversubscribes CPU, disk and sockets so badly
/// that the binary stops making progress at all. Run serially the same ten finish
/// in about five minutes; run in parallel this file was left out of every test run
/// for hanging past twenty, which meant the chunked fetch path — the most delicate
/// thing here — was in practice covered by nothing.
///
/// A plain mutex rather than a test-framework attribute: no new dependency, and it
/// says why it exists right where someone adding the eleventh test will read it.
/// Take it first thing in any test that transfers; the pure ones (tar packing) do
/// not need it and stay parallel.
static HEAVY: Mutex<()> = Mutex::new(());

/// Take [`HEAVY`], surviving a panic in an earlier test: a poisoned lock would
/// turn one failure into nine, hiding whatever else was broken.
fn heavy() -> std::sync::MutexGuard<'static, ()> {
    HEAVY.lock().unwrap_or_else(|e| e.into_inner())
}

/// The sender must conclude a send is **delivered** once the receiver has acked
/// every chunk — that is the only fact that means "they have the whole file".
///
/// It used to conclude this *solely* from the receiver's control channel going
/// away (`receiver_gone` with an empty tail). That made a completed delivery look
/// unfinished whenever the disconnect wasn't observed, leaving the send Active
/// forever and driving the manager's delivery loop to retract and re-post its
/// offer. Here the receiver stays alive after finishing (we never cancel it and
/// never drop the session), so the disconnect signal never comes: `Delivered`
/// must still fire.
#[tokio::test]
async fn sender_reports_delivered_once_every_chunk_is_acked() {
    let _heavy = heavy();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let out = dir.path().join("out.bin");
    // Deliberately small (one chunk): the bug is in the control channel's teardown,
    // not in the data path, and it reproduced on a 23-byte file. Keeping the payload
    // tiny also keeps this test cheap next to the multi-chunk ones it runs beside.
    let data: Vec<u8> = (0..64 * 1024).map(|i| (i * 11 + 5) as u8).collect();
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(&src, "src.bin", false, None, None, RelayChoice::Disabled)
        .await
        .expect("prepare_send");
    let ticket = session.ticket.clone();
    assert_eq!(session.chunks, 1);

    let delivered = Arc::new(Mutex::new(false));
    let seen = delivered.clone();
    let send_cancel = CancellationToken::new();
    let serve = {
        let c = send_cancel.clone();
        tokio::spawn(async move {
            session
                .serve(c, move |ev| {
                    if matches!(ev, SendEvent::Delivered) {
                        *seen.lock().unwrap() = true;
                    }
                })
                .await
        })
    };

    // Fetch the whole file. `recv_chunked` returns once everything is saved.
    let saved = flow::recv_chunked(
        &ticket,
        Some(out.clone()),
        None,
        RelayChoice::Disabled,
        CancellationToken::new(),
        |_| {},
    )
    .await
    .expect("recv_chunked");
    assert_eq!(std::fs::read(saved.path()).unwrap(), data, "integrity");

    // The sender is still serving (we have not cancelled it). Within a few ticks
    // of the last ack it must report Delivered — without needing the receiver to
    // disconnect first.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !*delivered.lock().unwrap() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        *delivered.lock().unwrap(),
        "sender must report Delivered once every chunk is acked, not only when the \
         receiver's control channel drops"
    );

    send_cancel.cancel();
    let _ = serve.await;
}

/// Fase 4 enabling property: two devices that share one identity recover the same
/// content key from a `--to` sealed delivery, so they re-seal byte-identical
/// pieces and derive the same `swarm_id` — the basis for co-swarming a sealed
/// transfer. A stranger can't open the key, so can't join.
#[test]
fn shared_identity_devices_co_swarm_a_sealed_transfer() {
    let _heavy = heavy();
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
    let _heavy = heavy();
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
    assert_eq!(std::fs::read(saved.path()).unwrap(), data);

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
    let _heavy = heavy();
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
    let outcome = flow::recv_chunked(
        &ticket,
        Some(out.clone()),
        None,
        RelayChoice::Disabled,
        cancel,
        move |e| ev.lock().unwrap().push(e),
    )
    .await
    .expect("recv_chunked returns Ok on cancel");
    assert!(
        matches!(outcome, RecvOutcome::Cancelled(_)),
        "a cancelled recv reports the Cancelled outcome"
    );

    let events = events.lock().unwrap().clone();
    assert!(
        !events.iter().any(|e| matches!(e, RecvEvent::Saved { .. })),
        "cancelled recv must not emit Saved"
    );
    assert_ne!(
        std::fs::read(outcome.path()).unwrap().len(),
        data.len(),
        "cancelled recv must not have written the whole file"
    );

    send_cancel.cancel();
    let _ = serve.await;
}

#[tokio::test]
async fn archive_roundtrip_packs_and_extracts() {
    let _heavy = heavy();
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
    assert_eq!(saved.into_path(), outdir);
    // `outdir` is the folder the receiver names after the transfer, so a lone
    // folder's contents land straight in it — not at `outdir/folder/folder/…`.
    assert_eq!(std::fs::read(outdir.join("a.txt")).unwrap(), b"hello alpha");
    assert_eq!(
        std::fs::read(outdir.join("sub/b.bin")).unwrap(),
        vec![7u8; 1000]
    );
    assert!(
        !outdir.join("folder").exists(),
        "the folder must not be nested inside a folder of its own name"
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
    let _heavy = heavy();
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
    let _heavy = heavy();
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
    assert_eq!(std::fs::read(saved.path()).unwrap(), data);

    // Each chunk is committed exactly once. Commit order is NOT guaranteed (pieces
    // commit out of order the moment they verify), but the set must be complete and
    // the file byte-identical (asserted above).
    let events = events.lock().unwrap().clone();
    let mut idxs: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            RecvEvent::Chunk { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    idxs.sort_unstable();
    assert_eq!(idxs, vec![0, 1, 2], "every chunk committed exactly once");

    send_cancel.cancel();
    let _ = serve.await;
    std::env::remove_var("ARVOLO_CONCURRENCY");
}

// Hard test: a REAL partial (not hand-built). Cancel a download after its first
// committed chunk, leaving a sparse output + resume sidecar, then resume from a
// fresh server and confirm it completes byte-identically without re-fetching what
// it already had. Exercises out-of-order commit + sidecar crash-safety end-to-end.
#[tokio::test]
async fn resume_mid_transfer_from_real_partial() {
    let _heavy = heavy();
    use std::sync::atomic::{AtomicUsize, Ordering};
    std::env::set_var("ARVOLO_CONCURRENCY", "2");
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("m.bin");
    let out = dir.path().join("m.out");
    // 48 MiB -> 3 chunks.
    let data: Vec<u8> = (0..48 * 1024 * 1024u64)
        .map(|i| (i * 37 + 11) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    // First pass: cancel the moment the first chunk commits, leaving a partial.
    let session = flow::prepare_send(&src, "m.bin", false, None, None, RelayChoice::Disabled)
        .await
        .unwrap();
    let ticket = session.ticket.clone();
    let sc = CancellationToken::new();
    let serve = {
        let c = sc.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    let seen = Arc::new(AtomicUsize::new(0));
    let s2 = seen.clone();
    flow::recv_chunked(
        &ticket,
        Some(out.clone()),
        None,
        RelayChoice::Disabled,
        cancel,
        move |e| {
            if matches!(e, RecvEvent::Chunk { .. }) && s2.fetch_add(1, Ordering::SeqCst) == 0 {
                c2.cancel(); // stop right after the first committed chunk
            }
        },
    )
    .await
    .unwrap();
    sc.cancel();
    let _ = serve.await;

    let partial = seen.load(Ordering::SeqCst);
    assert!(
        (1..3).contains(&partial),
        "left a real partial: {partial}/3"
    );
    let sidecar = std::path::PathBuf::from(format!("{}.arvhave", out.display()));
    assert!(sidecar.exists(), "a partial leaves a resume sidecar");

    // Second pass: resume from the sidecar on a fresh server; must complete.
    let session2 = flow::prepare_send(&src, "m.bin", false, None, None, RelayChoice::Disabled)
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

    assert_eq!(
        std::fs::read(&out).unwrap(),
        data,
        "resumed output is byte-identical"
    );
    let events = events.lock().unwrap().clone();
    let resumed_from = events.iter().find_map(|e| match e {
        RecvEvent::Started { resuming_from, .. } => Some(*resuming_from),
        _ => None,
    });
    assert_eq!(
        resumed_from,
        Some(partial),
        "resumed exactly the pieces already on disk"
    );
    let refetched = events
        .iter()
        .filter(|e| matches!(e, RecvEvent::Chunk { .. }))
        .count();
    assert_eq!(
        refetched,
        3 - partial,
        "only the missing pieces were re-fetched"
    );
    assert!(!sidecar.exists(), "sidecar cleaned up after completion");

    sc2.cancel();
    let _ = serve2.await;
    std::env::remove_var("ARVOLO_CONCURRENCY");
}

#[tokio::test]
async fn resume_from_sidecar_fetches_only_missing() {
    let _heavy = heavy();
    use arvolo_core::swarm::{bitfield_new, bitfield_set};
    use std::path::PathBuf;
    std::env::set_var("ARVOLO_CONCURRENCY", "2");
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("r.bin");
    let out = dir.path().join("r.out");
    let data: Vec<u8> = (0..40 * 1024 * 1024u64)
        .map(|i| (i * 29 + 5) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    // Full receive once to lay down a correct output file.
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

    // Simulate a partial download: a resume sidecar that marks ONLY chunk 0 present.
    // Resume must trust the sidecar (not the file length) and re-fetch just 1 and 2.
    let sidecar = PathBuf::from(format!("{}.arvhave", out.display()));
    let mut bf = bitfield_new(3);
    bitfield_set(&mut bf, 0);
    std::fs::write(&sidecar, &bf).unwrap();

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

    // Byte-identical, resumed from the sidecar (1 piece present), and only the two
    // missing chunks fetched (order not guaranteed).
    assert_eq!(
        std::fs::read(&out).unwrap(),
        data,
        "resumed output is byte-identical"
    );
    let events = events.lock().unwrap().clone();
    assert!(events.iter().any(|e| matches!(
        e,
        RecvEvent::Started {
            resuming_from: 1,
            ..
        }
    )));
    let mut idxs: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            RecvEvent::Chunk { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    idxs.sort_unstable();
    assert_eq!(idxs, vec![1, 2], "only the missing chunks were fetched");
    // Sidecar is removed once the download completes.
    assert!(!sidecar.exists(), "resume sidecar cleaned up on completion");

    sc2.cancel();
    let _ = serve2.await;
    std::env::remove_var("ARVOLO_CONCURRENCY");
}
