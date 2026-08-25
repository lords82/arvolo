//! Ten ways a transfer can change hands between the origin and the relay, and one
//! assertion under all of them: **the bytes come out identical**.
//!
//! The switch is not a mode the transfer is in, it is a decision taken per piece,
//! per attempt — so the cases worth pinning are the transitions, not the states:
//! starting on one side, losing one side, getting one back, believing something
//! false about one side. What must never vary is the file.
//!
//! Every fetch is verified against the ticket's hash whatever served it, so the
//! relay is only ever a *hint* about where to look. Two of these tests attack
//! exactly that: a relay serving the wrong bytes, and a relay claimed to hold what
//! it no longer has. Neither may corrupt the download or stall it.
//!
//! Own test binary: it pins `ARVOLO_CONCURRENCY` so a receiver can be interrupted
//! at a known point, and env is process-global.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arvolo_core::backfill::BlobNode;
use arvolo_core::chunked::{ChunkTicket, SeedRequest};
use arvolo_core::crypto::Identity;
use arvolo_core::flow::{self, ChunkSource, RecvEvent, ReusablePrep, SendSession};
use arvolo_core::hash::Hash;
use arvolo_core::swarm::{bitfield_new, bitfield_set};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

const CHUNK: usize = 16 * 1024 * 1024;

/// One piece in flight at a time, so "after the first chunk" is a point in the
/// download and not a race. Set, never removed: these tests share one process.
fn serial_fetch() {
    std::env::set_var("ARVOLO_CONCURRENCY", "1");
}

/// A running relay plus the path to its chunk store, so a test can tamper with
/// what it is holding.
struct Relay {
    url: String,
    store: PathBuf,
    _dir: tempfile::TempDir,
}

async fn spawn_relay() -> Relay {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().to_path_buf();
    let node = BlobNode::spawn(&store, RelayChoice::Disabled)
        .await
        .expect("blob node");
    let state = AppState::new(
        Arc::new(Mailbox::in_memory().expect("mailbox")),
        Arc::new(node),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    Relay {
        url: format!("http://{addr}"),
        store,
        _dir: dir,
    }
}

fn payload(chunks: usize, tail: usize) -> Vec<u8> {
    let n = (chunks - 1) * CHUNK + tail;
    (0..n as u64).map(|i| (i * 131 + 7) as u8).collect()
}

/// Put exactly `pieces` on the relay, by hand, while `session` is alive to be
/// pulled from. Precise control over *which* pieces the relay holds is what makes
/// a mixed-source download reproducible.
async fn seed_pieces(relay: &str, ticket: &ChunkTicket, pieces: &[usize]) {
    let req = SeedRequest {
        sender: ticket.providers[0].clone(),
        chunks: pieces.iter().map(|&i| ticket.chunks[i]).collect(),
        token: ticket
            .relay
            .as_ref()
            .expect("relay in ticket")
            .token
            .clone(),
        swarm_id: arvolo_core::swarm::swarm_id(&ticket.chunks, ticket.total_size),
    };
    let resp = reqwest::Client::new()
        .post(format!("{relay}/v1/seed"))
        .body(req.encode().unwrap())
        .send()
        .await
        .expect("seed");
    assert!(
        resp.status().is_success(),
        "seeding failed: {}",
        resp.status()
    );
}

/// A second sender for the same bytes: same content key, fresh node id.
///
/// Because a piece is sealed deterministically from the content key, two senders
/// holding that key produce byte-identical, interchangeable ciphertext — so this
/// one can hand the relay exactly the pieces the first one would have. It stands
/// in for a sender that came back at an address the relay can reach, which an
/// in-process test cannot otherwise arrange: these endpoints run with the iroh
/// relay disabled, which turns discovery off (`DiscoveryChoice::Off` —"dial only
/// what a ticket already says"), and every bind takes a fresh ephemeral port. In
/// production a returning sender is found by resolving its stable node id.
async fn same_content_sender(
    src: &Path,
    relay: &str,
    key: [u8; 32],
    total_size: u64,
    chunks: Vec<Hash>,
) -> SendSession {
    let prep =
        ReusablePrep::from_parts(key, [7u8; 32], total_size, chunks).expect("prep from parts");
    flow::prepare_send_reusing(
        src,
        "f.bin",
        false,
        None,
        Some(relay.to_string()),
        RelayChoice::Disabled,
        &mut Some(prep),
    )
    .await
    .expect("second sender")
}

struct Received {
    path: PathBuf,
    sources: Vec<ChunkSource>,
}

impl Received {
    fn via_relay(&self) -> usize {
        self.sources
            .iter()
            .filter(|s| matches!(s, ChunkSource::Relay))
            .count()
    }
    fn via_sender(&self) -> usize {
        self.sources
            .iter()
            .filter(|s| matches!(s, ChunkSource::Sender))
            .count()
    }
}

/// Run a receiver to completion (or fail the test), recording where each piece
/// came from. `on_first` fires once, after the first piece lands — the hook the
/// mid-flight tests use to pull a source out from under it.
async fn receive(
    ticket: &str,
    out: PathBuf,
    identity: Option<&Identity>,
    on_first: Option<tokio::sync::oneshot::Sender<()>>,
) -> Received {
    let sources = Arc::new(Mutex::new(Vec::new()));
    let s = sources.clone();
    let fired = Arc::new(Mutex::new(on_first));
    let outcome = tokio::time::timeout(
        Duration::from_secs(180),
        flow::recv_chunked(
            ticket,
            Some(out),
            identity,
            RelayChoice::Disabled,
            CancellationToken::new(),
            move |e| {
                if let RecvEvent::Chunk { source, .. } = e {
                    s.lock().unwrap().push(source);
                    if let Some(tx) = fired.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                }
            },
        ),
    )
    .await
    .expect("the download must not hang")
    .expect("receive");
    assert!(outcome.is_complete(), "the download must finish");
    Received {
        path: outcome.into_path(),
        sources: Arc::try_unwrap(sources).unwrap().into_inner().unwrap(),
    }
}

fn assert_identical(got: &Path, want: &[u8]) {
    let on_disk = std::fs::read(got).expect("read output");
    assert_eq!(on_disk.len(), want.len(), "wrong length");
    assert!(on_disk == want, "the file differs from what was sent");
}

/// Lay down `present` pieces at their true offsets plus a resume sidecar, so a
/// receiver starts already holding them.
fn partial_on_disk(out: &Path, data: &[u8], total: usize, present: &[usize]) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(out)
        .unwrap();
    f.set_len(data.len() as u64).unwrap();
    for &i in present {
        let start = i * CHUNK;
        let end = (start + CHUNK).min(data.len());
        f.seek(SeekFrom::Start(start as u64)).unwrap();
        f.write_all(&data[start..end]).unwrap();
    }
    drop(f);
    let mut bf = bitfield_new(total);
    for &i in present {
        bitfield_set(&mut bf, i);
    }
    std::fs::write(PathBuf::from(format!("{}.arvhave", out.display())), &bf).unwrap();
}

// ---------------------------------------------------------------- the ten cases

/// 1. Both sources up, each holding part. The pieces the relay has come from the
///    relay (it is preferred, to spare the origin); the rest come from the origin.
///    One download, two sources, one correct file.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn both_sources_up_splits_the_work() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data = payload(3, 8 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.url.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket = ChunkTicket::decode(&session.ticket).unwrap();
    seed_pieces(&relay.url, &ticket, &[0, 2]).await;

    let got = receive(&session.ticket, dir.path().join("got.bin"), None, None).await;
    assert_identical(&got.path, &data);
    assert_eq!(
        got.via_relay(),
        2,
        "the relay held two pieces: {:?}",
        got.sources
    );
    assert_eq!(got.via_sender(), 1, "the origin had to cover the third");
    session.shutdown().await;
}

/// 2. The origin vanishes mid-download. Everything it had not yet served is on the
///    relay, and the transfer carries on across the gap without restarting.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_origin_vanishing_midway_is_survivable() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data = payload(3, 8 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.url.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket = ChunkTicket::decode(&session.ticket).unwrap();
    // The relay holds the tail only, so the first piece must come from the origin.
    seed_pieces(&relay.url, &ticket, &[1, 2]).await;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let closing = tokio::spawn(async move {
        let _ = rx.await;
        session.shutdown().await; // the origin goes away, mid-download
    });

    let got = receive(
        &ticket.encode().unwrap(),
        dir.path().join("got.bin"),
        None,
        Some(tx),
    )
    .await;
    assert_identical(&got.path, &data);
    assert!(
        got.via_sender() >= 1 && got.via_relay() >= 1,
        "the download should have started on one side and finished on the other: {:?}",
        got.sources
    );
    let _ = closing.await;
}

/// 3. A folder, not a file. The payload is staged as an archive and unpacked at
///    the far end, and that whole path has to survive coming off the relay with the
///    origin gone — a different set of bytes on the wire and a different one on
///    disk at the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_folder_arrives_whole_from_the_relay() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("stuff");
    std::fs::create_dir(&folder).unwrap();
    let big = payload(2, 2 * 1024 * 1024);
    std::fs::write(folder.join("big.bin"), &big).unwrap();
    std::fs::write(folder.join("note.txt"), b"a small one beside it").unwrap();
    // A folder travels as a tar, staged by the caller exactly as the CLI does it;
    // `archive` is what tells the far end to unpack rather than save.
    let tar = dir.path().join("stuff.tar");
    flow::pack_tar(std::slice::from_ref(&folder), &tar).unwrap();

    let session = flow::prepare_send(
        &tar,
        "stuff",
        true,
        None,
        Some(relay.url.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket_str = session.ticket.clone();
    assert!(
        session
            .park_on_relay(&CancellationToken::new(), |_| {})
            .await
    );
    session.shutdown().await;

    let got = receive(&ticket_str, dir.path().join("out"), None, None).await;
    assert!(
        got.via_relay() >= 1,
        "nobody else was serving: {:?}",
        got.sources
    );
    assert_identical(&got.path.join("big.bin"), &big);
    assert_eq!(
        std::fs::read(got.path.join("note.txt")).unwrap(),
        b"a small one beside it",
        "the small file beside it has to survive too"
    );
}

/// 4. A resume that changes sides: the partial was pulled P2P in a previous run,
///    the origin is gone, and the rest comes off the relay. What is already on disk
///    is kept, not re-fetched.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_resume_finishes_on_the_other_side() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data = payload(3, 8 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.url.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket_str = session.ticket.clone();
    let ticket = ChunkTicket::decode(&ticket_str).unwrap();
    seed_pieces(&relay.url, &ticket, &[1, 2]).await;
    session.shutdown().await;

    let out = dir.path().join("got.bin");
    partial_on_disk(&out, &data, 3, &[0]);

    let got = receive(&ticket_str, out, None, None).await;
    assert_identical(&got.path, &data);
    assert_eq!(
        got.sources.len(),
        2,
        "only the two missing pieces should have been fetched: {:?}",
        got.sources
    );
    assert_eq!(got.via_relay(), 2);
}

/// 5. The relay holds only pieces the receiver already has — a stale copy of an
///    old attempt. Useless, and it must not be *mistaken* for progress: everything
///    still missing has to come from the origin.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_relay_holding_only_what_we_have_is_harmless() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data = payload(3, 8 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.url.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket = ChunkTicket::decode(&session.ticket).unwrap();
    seed_pieces(&relay.url, &ticket, &[0]).await;

    let out = dir.path().join("got.bin");
    partial_on_disk(&out, &data, 3, &[0]);

    let got = receive(&session.ticket, out, None, None).await;
    assert_identical(&got.path, &data);
    assert_eq!(
        got.via_sender(),
        2,
        "both missing pieces come from the origin"
    );
    assert_eq!(got.via_relay(), 0);
    session.shutdown().await;
}

/// 6. The relay serves the wrong bytes. The hash in the ticket is the authority, so
///    the piece is rejected, the relay is dropped as a source, and the origin
///    supplies it. A relay that lies costs a round trip, never the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_relay_serving_rubbish_cannot_corrupt_the_file() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data = payload(2, 4 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.url.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket = ChunkTicket::decode(&session.ticket).unwrap();
    seed_pieces(&relay.url, &ticket, &[0, 1]).await;

    // Overwrite one stored chunk with garbage of the same name. The relay will
    // happily serve it; the receiver must not accept it.
    let poisoned: &Hash = &ticket.chunks[0];
    std::fs::write(relay.store.join(poisoned.to_string()), vec![0xAAu8; 4096]).unwrap();

    let got = receive(&session.ticket, dir.path().join("got.bin"), None, None).await;
    assert_identical(&got.path, &data);
    assert!(
        got.via_sender() >= 1,
        "the poisoned piece had to be re-fetched from the origin: {:?}",
        got.sources
    );
    session.shutdown().await;
}

/// 7. The relay is *said* to hold a piece it no longer has — the ordinary stale
///    case, because the sender's record of what it seeded never un-marks anything.
///    A wrong hint must cost a failed fetch and nothing more.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_stale_claim_about_the_relay_does_not_stall_us() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data = payload(2, 4 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.url.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket = ChunkTicket::decode(&session.ticket).unwrap();
    // The sender parks everything, so it will advertise the relay as holding it…
    assert!(
        session
            .park_on_relay(&CancellationToken::new(), |_| {})
            .await
    );
    // …and then the relay loses one of them.
    std::fs::remove_file(relay.store.join(ticket.chunks[1].to_string())).unwrap();

    let got = receive(&session.ticket, dir.path().join("got.bin"), None, None).await;
    assert_identical(&got.path, &data);
    assert!(
        got.via_sender() >= 1,
        "the piece the relay lost must have come from the origin: {:?}",
        got.sources
    );
    session.shutdown().await;
}

/// 8. A sealed `--to` transfer across the switch. The content key travels in the
///    ticket, not with the sender, so an origin that is gone must not take the
///    ability to open the file with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_sealed_transfer_still_opens_when_only_the_relay_is_left() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data = payload(2, 4 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();

    let sender = Identity::generate();
    let recipient = Identity::generate();
    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        Some((&sender, &recipient.public())),
        Some(relay.url.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket_str = session.ticket.clone();
    assert!(
        session
            .park_on_relay(&CancellationToken::new(), |_| {})
            .await
    );
    session.shutdown().await;

    let got = receive(
        &ticket_str,
        dir.path().join("got.bin"),
        Some(&recipient),
        None,
    )
    .await;
    assert_identical(&got.path, &data);
    assert_eq!(got.via_relay(), 2, "nobody else was left to serve it");
}

/// 9. A short final chunk, taken off the relay. The last piece is the one whose
///    length is not the chunk size, and the one a length bug shows up in.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_short_last_piece_survives_the_relay() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    // Deliberately awkward: one byte over two whole chunks.
    let data = payload(3, 1);
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.url.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket_str = session.ticket.clone();
    assert_eq!(ChunkTicket::decode(&ticket_str).unwrap().chunks.len(), 3);
    assert!(
        session
            .park_on_relay(&CancellationToken::new(), |_| {})
            .await
    );
    session.shutdown().await;

    let got = receive(&ticket_str, dir.path().join("got.bin"), None, None).await;
    assert_identical(&got.path, &data);
    assert_eq!(got.via_relay(), 3);
}

/// 10. The relay gains the rest **after** the download has started, and the origin
///     leaves. Nothing was pre-positioned: the receiver had already looked once and
///     found the relay empty, so finishing at all means it looked again. This is the
///     shape of "they parked it and closed the laptop while you were downloading".
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_relay_stocked_mid_download_rescues_it() {
    serial_fetch();
    let relay = spawn_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    let data = payload(3, 8 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();

    let first = flow::prepare_send(
        &src,
        "f.bin",
        false,
        None,
        Some(relay.url.clone()),
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket_str = first.ticket.clone();
    let ticket = ChunkTicket::decode(&ticket_str).unwrap();
    let key = first.content_key();
    let (t_size, t_chunks) = (ticket.total_size, ticket.chunks.clone());

    let (tx, rx) = tokio::sync::oneshot::channel();
    let src2 = src.clone();
    let url2 = relay.url.clone();
    let stocked = Arc::new(AtomicBool::new(false));
    let flag = stocked.clone();
    let rescue = tokio::spawn(async move {
        let _ = rx.await;
        // The origin goes, and with it the only source the receiver knows about.
        first.shutdown().await;
        // A second holder of the same content hands the rest to the relay. The
        // receiver is never told its address and never talks to it.
        let other = same_content_sender(&src2, &url2, key, t_size, t_chunks).await;
        // Everything, not a guessed subset: which piece the receiver happened to
        // take first is chosen by rarest-first with a random tie-break, and at the
        // start of this test every piece is tied at one provider. Seeding [1,2] on
        // the assumption it began at 0 left whichever piece it really began with
        // sourceless one run in three.
        assert!(
            other.park_on_relay(&CancellationToken::new(), |_| {}).await,
            "the stand-in should have been able to park the payload"
        );
        flag.store(true, Ordering::Relaxed);
        other
    });

    let got = receive(&ticket_str, dir.path().join("got.bin"), None, Some(tx)).await;
    assert_identical(&got.path, &data);
    assert!(stocked.load(Ordering::Relaxed), "the rescue never happened");
    assert_eq!(
        got.via_sender(),
        1,
        "only the first piece predates the origin leaving: {:?}",
        got.sources
    );
    assert_eq!(
        got.via_relay(),
        2,
        "the rest had to be found by asking again"
    );
    rescue.await.unwrap().shutdown().await;
}
