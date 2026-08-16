//! The ordinary case the other swarm tests step around: **one big file, two peers
//! pulling it at the same time**, both incomplete, both still downloading from the
//! origin — do they also feed each other the pieces they have already got?
//!
//! `disjoint_swarm` hand-builds two complementary halves and removes the origin;
//! `coswarm` runs its peers in sequence, the first complete before the second
//! starts; `swarm_fallback` is about a peer *leaving*. All three arrange a state in
//! which the peer path is the only one left. None of them shows two live,
//! half-finished receivers trading with each other while the origin is right there
//! and perfectly able to serve them both — which is what actually happens when you
//! send a file to two people at once, and the case where a swarm either earns its
//! keep or does nothing at all.
//!
//! The origin deliberately stays up for the whole test. That is the realistic
//! shape, and it also means completion never depends on the two receivers holding
//! complementary pieces at some instant: whatever they fail to trade, they can
//! always fetch. So the correctness assertion (both files byte-identical to the
//! source) cannot deadlock, and the swarm assertion is layered on top of it rather
//! than load-bearing for it.
//!
//! What makes the peer exchange observable at all is the announce interval. A
//! member publishes its bitfield to the tracker on that timer, so it is also how
//! long it takes one peer to learn what the other holds — and at the 20s default,
//! two peers on loopback both finish a 64 MiB file while still believing the other
//! has nothing. `ARVOLO_SWARM_ANNOUNCE_SECS=1` shortens it; the swarm itself is
//! untouched.
//!
//! This is also the regression test for `schedule::prefer_offloadable`. Written
//! before it existed, it recorded the opposite: the two receivers finished without
//! ever trading a piece, because rarest-first counts the origin among a piece's
//! providers and so ranks what a peer holds *last*. That note now lives at the
//! assertion it turned into.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arvolo_core::backfill::BlobNode;
use arvolo_core::flow::{self, RecvEvent};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

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

/// Highest `pieces_from_peers` this receiver ever reported.
fn from_peers(events: &Mutex<Vec<RecvEvent>>) -> u64 {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            RecvEvent::Swarm {
                pieces_from_peers, ..
            } => Some(*pieces_from_peers),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// Highest peer count this receiver ever saw in the swarm.
fn peers_seen(events: &Mutex<Vec<RecvEvent>>) -> u64 {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            RecvEvent::Swarm { peers, .. } => Some(*peers as u64),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn committed(events: &Mutex<Vec<RecvEvent>>) -> usize {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, RecvEvent::Chunk { .. }))
        .count()
}

/// Poll until `path` holds exactly `data`, or the deadline passes.
async fn wait_for_file(path: &std::path::Path, data: &[u8], within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) == data.len() as u64
            && std::fs::read(path).map(|d| d == data).unwrap_or(false)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_peers_downloading_at_once_also_feed_each_other() {
    // The receiver logs one line per committed piece naming where it came from
    // ("chunk 3 ← peer abcd…"). With `--nocapture` that is the whole story of who
    // fed whom, and without it this test can only ever say "no piece came from a
    // peer" without saying why.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("arvolo_core=info")
        .with_test_writer()
        .try_init();
    // See the module note: at the default interval neither peer would ever hear
    // what the other holds before both are done.
    std::env::set_var("ARVOLO_SWARM_ANNOUNCE_SECS", "1");
    // Whichever finishes first keeps serving, so the other can still pull from it.
    std::env::set_var("ARVOLO_SEED_AFTER", "120");
    // Two pieces in flight, and this setting decides whether there is a swarm to
    // observe at all.
    //
    // A receiver fetches `ARVOLO_CONCURRENCY` pieces at once (4 by default). Give a
    // four-piece file to a receiver with a four-piece window and it asks the origin
    // for the entire file in the first instant — before any announce, and leaving no
    // later moment at which it is missing something it has not already started
    // fetching. The first version of this test did exactly that and observed zero
    // trades, which said nothing about the swarm and everything about the setup: a
    // real transfer is dozens of pieces against a window of four. Halving the window
    // reproduces that ratio without moving a quarter-gigabyte around.
    //
    // Two rather than one, because a window of one can never queue on the origin and
    // so can never reach the case `prefer_offloadable` exists for.
    std::env::set_var("ARVOLO_CONCURRENCY", "2");

    let relay = spawn_relay().await;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.bin");
    // 128 MiB -> 8 pieces, which with a window of two is four rounds of fetching.
    //
    // The size is set by the number of *rounds*, not by the number of pieces. A peer
    // has nothing to offer until it has verified something, and it advertises that
    // only on its next announce — so the earliest a trade can happen is the third
    // round. At 64 MiB (four pieces, two rounds) both receivers had asked the origin
    // for the whole file before either had anything to give, and the test read zero
    // trades no matter what the scheduler did.
    let data: Vec<u8> = (0..128 * 1024 * 1024u64)
        .map(|i| (i * 197 + 11) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    // A shared `arvc…` ticket — no recipient — which is what "send this file to
    // whoever has the link" means, and the case where two strangers end up in the
    // same swarm.
    let session = flow::prepare_send(
        &src,
        "big.bin",
        false,
        None,
        Some(relay.clone()),
        RelayChoice::Disabled,
    )
    .await
    .expect("prepare_send");
    assert_eq!(session.chunks, 8);
    assert!(session.has_relay, "the ticket must carry the tracker relay");
    let ticket = session.ticket.clone();

    // The sender's own view is under test too — see the assertions at the end.
    let sender_events = Arc::new(Mutex::new(Vec::new()));
    let origin_cancel = CancellationToken::new();
    let origin = {
        let (c, ev) = (origin_cancel.clone(), sender_events.clone());
        tokio::spawn(async move { session.serve(c, move |e| ev.lock().unwrap().push(e)).await })
    };

    // Both receivers start together. Detached, with completion read off the output
    // file: awaiting `recv_chunked` can block in endpoint teardown after the bytes
    // are already on disk (same reason as the other swarm tests).
    let mut outs = Vec::new();
    let mut evs = Vec::new();
    let mut tasks = Vec::new();
    for who in ["x", "y"] {
        let out = dir.path().join(format!("{who}.out"));
        let ev = Arc::new(Mutex::new(Vec::new()));
        let task = {
            let (ticket, out, ev) = (ticket.clone(), out.clone(), ev.clone());
            tokio::spawn(async move {
                let _ = flow::recv_chunked(
                    &ticket,
                    Some(out),
                    None,
                    RelayChoice::Disabled,
                    CancellationToken::new(),
                    move |e| ev.lock().unwrap().push(e),
                )
                .await;
            })
        };
        outs.push(out);
        evs.push(ev);
        tasks.push(task);
    }

    // 1. Correctness first, and it is the part that must never be flaky: two
    //    receivers pulling the same file at the same time each end up with the
    //    whole of it, byte for byte. The origin is alive throughout, so nothing
    //    here depends on the swarm working.
    for (i, out) in outs.iter().enumerate() {
        assert!(
            wait_for_file(out, &data, Duration::from_secs(240)).await,
            "receiver {i} never completed (committed {}/8)",
            committed(&evs[i])
        );
    }

    // 2. They found each other. The tracker is the only way they could have: each
    //    announces itself to the relay and reads the other's entry back.
    let seen: Vec<u64> = evs.iter().map(|e| peers_seen(e)).collect();
    assert!(
        seen.iter().any(|&n| n > 0),
        "neither receiver ever saw a swarm peer: {seen:?}"
    );

    // 3. And they actually fed each other. This is the claim the test is named for.
    //
    //    It did not hold before `prefer_offloadable`. The picker is rarest-first, and
    //    because the origin holds every piece, a piece a peer already has counts one
    //    provider *more* than one it lacks — so a peer's pieces were picked last. The
    //    per-piece log of a run then read: both receivers took a distinct piece, then
    //    both went to the origin for the same third piece, then both for the same
    //    fourth, each sitting on a piece the other needed the whole time. A piece
    //    crossed between the two in one run out of four.
    //
    //    With the filter, a run of this test reads: 16 pieces delivered in all, 11
    //    from the origin and 5 between the peers, the first trade at the third round
    //    — as soon as a peer has anything to offer and the origin is busy. Roughly a
    //    third of the origin's upload gone, without giving up rarest-first: half the
    //    window still drains the pieces only the origin has, which is what keeps the
    //    swarm alive if it leaves.
    let traded: Vec<u64> = evs.iter().map(|e| from_peers(e)).collect();
    eprintln!("swarm: pieces taken from a peer, per receiver: {traded:?}");
    assert!(
        traded.iter().sum::<u64>() > 0,
        "no piece ever came from a peer: {traded:?} — both receivers took everything \
         from the origin, which is what this test exists to catch"
    );
    // Printed rather than asserted on, because `RecvEvent::Warning` is not only for
    // faults: a completed download announces "seeding to the swarm for 120s" through
    // the same variant. Matching failures out of it by message text would be a test
    // that breaks on rewording, so this reports and leaves the reading to whoever
    // runs it — a peer fetch that was chosen and then broke would show up here.
    let warnings: Vec<String> = evs
        .iter()
        .flat_map(|e| {
            e.lock()
                .unwrap()
                .iter()
                .filter_map(|x| match x {
                    RecvEvent::Warning { message } => Some(message.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    eprintln!("swarm: warnings along the way: {warnings:?}");

    // 4. And what the *sender* believes about all this. Both of these were wrong
    //    before the accounting was made per-receiver: the ack set was a union, so
    //    two receivers holding complementary halves proved a delivery neither had,
    //    and the byte counter was a sum across receivers, so it reached the payload
    //    size at about the halfway mark of the actual work.
    //
    //    Both files being complete is *not* enough to read this: the last ack still
    //    has to reach the origin, and the origin concludes delivery on a 500ms
    //    ticker. Poll for the count instead of racing it.
    let deadline = Instant::now() + Duration::from_secs(20);
    let delivered = loop {
        let n = sender_events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, flow::SendEvent::Delivered))
            .count();
        if n >= 2 || Instant::now() >= deadline {
            break n;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let over_total = {
        let evs = sender_events.lock().unwrap();
        evs.iter()
            .filter_map(|e| match e {
                flow::SendEvent::Progress { transferred, .. } => Some(*transferred),
                _ => None,
            })
            .any(|t| t > data.len() as u64)
    };
    assert!(!over_total, "reported progress ran past the payload size");
    assert_eq!(
        delivered, 2,
        "two receivers each took a whole copy, so two deliveries — one is the old \
         once-per-session latch, more than two is double counting"
    );

    origin_cancel.cancel();
    let _ = origin.await;
    for t in tasks {
        t.abort();
    }
}
