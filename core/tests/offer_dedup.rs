//! An offer about a file already coming down is not a question to ask again.
//!
//! A held send re-offers itself: after a pause, after a restart, after any attempt
//! that found nobody. The recipient used to get a fresh row awaiting approval every
//! time — one observed inbox held three offers for the same 10.7 GiB file while that
//! very file was downloading at 10.4 GiB.
//!
//! What identifies "the same send" is the ticket's content id, not its text: the
//! provider address changes with every socket bind and HPKE re-seals the key blob on
//! every attempt, so two tickets for one send agree on almost nothing else. These
//! tests pin that the match is made on the content id, and — the half that matters —
//! that a *different* send still asks.

use arvolo_core::chunked::ChunkTicket;
use arvolo_core::crypto::Identity;
use arvolo_core::flow;
use arvolo_core::manager::{TransferManager, TransferStatus};
use arvolo_core::transfer::RelayChoice;

fn payload(dir: &std::path::Path, name: &str, fill: u8) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, vec![fill; 64 * 1024]).unwrap();
    p
}

/// A ticket for the same send as `t`, as a later attempt would mint it: same digests
/// and same node id, everything else moved on.
fn as_a_later_attempt(t: &ChunkTicket) -> String {
    // A fresh socket: the same node, reachable somewhere else.
    let mut provider = t.providers[0].clone();
    provider.addrs.clear();
    ChunkTicket {
        total_size: t.total_size,
        chunk_size: t.chunk_size,
        chunks: t.chunks.clone(),
        providers: vec![provider],
        relay: None,
        key: t.key.clone(),
        name: t.name.clone(),
        archive: t.archive,
    }
    .encode()
    .unwrap()
}

#[tokio::test]
async fn a_later_offer_for_the_same_send_finds_the_download_already_running() {
    let dir = tempfile::tempdir().unwrap();
    let dl = tempfile::tempdir().unwrap();
    let sender = Identity::generate().public();

    // Prepared and then dropped: nobody is serving, so the download stays trying —
    // which is exactly the state a paused sender leaves its recipient in.
    let session = flow::prepare_send(
        payload(dir.path(), "one.bin", 1).as_path(),
        "one.bin",
        false,
        None,
        None,
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket = session.ticket.clone();
    let size = session.total_size;
    drop(session);

    let m = TransferManager::new(Identity::generate(), None, dl.path().to_path_buf());
    let id = m.start_download(
        ticket.clone(),
        dl.path().join("one.bin"),
        Some(sender.clone()),
        "one.bin".into(),
        size,
    );

    let later = as_a_later_attempt(&ChunkTicket::decode(&ticket).unwrap());
    assert_ne!(
        later, ticket,
        "a later attempt is a different ticket string"
    );
    assert_eq!(
        m.download_of_same_content(&later, Some(&sender)),
        Some(id),
        "the same send, offered again, must find the download it belongs to"
    );

    // A stranger naming content we hold should be impossible — it would mean holding
    // the sender's content key. If it ever happens it is to be seen, not swallowed.
    let stranger = Identity::generate().public();
    assert_eq!(m.download_of_same_content(&later, Some(&stranger)), None);

    m.cancel(id);
}

/// The control. Without it the test above passes on a function that says yes to
/// everything, and the cost of that would be a file silently never offered.
#[tokio::test]
async fn a_different_send_still_asks() {
    let dir = tempfile::tempdir().unwrap();
    let dl = tempfile::tempdir().unwrap();

    let mk = |name: &'static str, fill: u8| {
        let dir = dir.path().to_path_buf();
        async move {
            let s = flow::prepare_send(
                payload(&dir, name, fill).as_path(),
                name,
                false,
                None,
                None,
                RelayChoice::Disabled,
            )
            .await
            .unwrap();
            (s.ticket.clone(), s.total_size)
        }
    };
    let (one, size) = mk("one.bin", 1).await;
    let (two, _) = mk("two.bin", 2).await;

    let m = TransferManager::new(Identity::generate(), None, dl.path().to_path_buf());
    let id = m.start_download(one, dl.path().join("one.bin"), None, "one.bin".into(), size);

    assert_eq!(
        m.download_of_same_content(&two, None),
        None,
        "another file from the same sender is another decision"
    );
    m.cancel(id);
}

/// A download that has ended stops answering for its content. The user may have
/// deleted the file since, and "you already have this" is a different feature with a
/// different way of being wrong — so a re-offer after the end asks, as it always did.
#[tokio::test]
async fn a_download_that_ended_does_not_swallow_a_new_offer() {
    let dir = tempfile::tempdir().unwrap();
    let dl = tempfile::tempdir().unwrap();

    let session = flow::prepare_send(
        payload(dir.path(), "one.bin", 1).as_path(),
        "one.bin",
        false,
        None,
        None,
        RelayChoice::Disabled,
    )
    .await
    .unwrap();
    let ticket = session.ticket.clone();
    let size = session.total_size;
    drop(session);

    let m = TransferManager::new(Identity::generate(), None, dl.path().to_path_buf());
    let id = m.start_download(
        ticket.clone(),
        dl.path().join("one.bin"),
        None,
        "one.bin".into(),
        size,
    );
    assert_eq!(m.download_of_same_content(&ticket, None), Some(id));

    m.cancel(id);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let ended = m
            .get(id)
            .map(|t| {
                matches!(
                    t.status,
                    TransferStatus::Cancelled
                        | TransferStatus::Completed
                        | TransferStatus::Failed(_)
                )
            })
            .unwrap_or(false);
        if ended {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the cancel never settled"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        m.download_of_same_content(&ticket, None),
        None,
        "a transfer that is over is not a place to quietly send an offer"
    );
}
