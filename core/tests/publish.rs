//! A download writes where the engine tells it to. On a phone that is the app's
//! own storage — the only place a receive can pre-allocate a file, write pieces at
//! their offsets and keep a resume sidecar beside the partial — and the file has
//! to move somewhere a person can reach it once it is whole.
//!
//! `SavePublisher` is the whole of the engine's involvement in that move. These
//! tests pin the three things the rest of the system is entitled to assume: that
//! it is called once with the finished file, that its answer is what the row then
//! points at, and that a manager without one behaves exactly as it always has.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arvolo_core::crypto::Identity;
use arvolo_core::flow;
use arvolo_core::manager::{SavePublisher, SavedLocation, Transfer, TransferManager, TransferStatus};
use arvolo_core::transfer::RelayChoice;
use tokio_util::sync::CancellationToken;

/// Stands in for the platform: moves the finished file out of the app's storage
/// and answers with a handle that is **not** a path, which is the shape Android's
/// MediaStore has and the case the engine is least likely to get right by accident.
struct Public {
    dir: PathBuf,
    calls: Mutex<Vec<(PathBuf, String)>>,
    refuse: bool,
}

impl Public {
    fn new(dir: &Path, refuse: bool) -> Arc<Self> {
        Arc::new(Self {
            dir: dir.to_path_buf(),
            calls: Mutex::new(Vec::new()),
            refuse,
        })
    }
}

impl SavePublisher for Public {
    fn publish(&self, staged: &Path, name: &str) -> anyhow::Result<SavedLocation> {
        self.calls
            .lock()
            .unwrap()
            .push((staged.to_path_buf(), name.to_string()));
        if self.refuse {
            anyhow::bail!("no room left on the public volume");
        }
        std::fs::rename(staged, self.dir.join(name))?;
        Ok(SavedLocation::Uri(format!(
            "content://media/external/downloads/{name}"
        )))
    }
}

/// Serve a payload over the local P2P path and hand back the ticket, the bytes,
/// and the token that stops serving.
async fn served(dir: &Path) -> (String, Vec<u8>, CancellationToken, tokio::task::JoinHandle<()>) {
    let src = dir.join("holiday.bin");
    let data: Vec<u8> = (0..(1024 * 1024u64)).map(|i| (i * 31 + 7) as u8).collect();
    std::fs::write(&src, &data).unwrap();
    let session = flow::prepare_send(&src, "holiday.bin", false, None, None, RelayChoice::Disabled)
        .await
        .unwrap();
    let ticket = session.ticket.clone();
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    let serve = tokio::spawn(async move {
        let _ = session.serve(c, |_| {}).await;
    });
    (ticket, data, cancel, serve)
}

async fn wait_terminal(mgr: &TransferManager, id: u64) -> Transfer {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let row = mgr
            .list()
            .into_iter()
            .find(|t| t.id == id)
            .expect("the row is in the list");
        match &row.status {
            TransferStatus::Completed => return row,
            TransferStatus::Failed(e) => panic!("the download failed: {e}"),
            _ => {}
        }
        assert!(
            Instant::now() < deadline,
            "the download never reached a terminal state"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_published_download_points_at_where_the_platform_put_it() {
    let app = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let (ticket, data, cancel, serve) = served(app.path()).await;

    let publisher = Public::new(downloads.path(), false);
    let mgr = TransferManager::new(Identity::generate(), None, app.path().to_path_buf());
    mgr.set_publisher(publisher.clone());

    let staged = app.path().join("holiday.out");
    let id = mgr.start_download(
        ticket,
        staged.clone(),
        None,
        "holiday.bin".into(),
        data.len() as u64,
    );
    let row = wait_terminal(&mgr, id).await;

    let calls = publisher.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "the publisher is called once, not per piece");
    assert_eq!(calls[0].0, staged, "it is handed the finished file");
    assert_eq!(calls[0].1, "holiday.out", "under the name it landed with");

    assert_eq!(
        std::fs::read(downloads.path().join("holiday.out")).unwrap(),
        data,
        "the file is out of the app's storage, whole"
    );
    assert!(!staged.exists(), "and no longer in staging");

    assert_eq!(
        row.saved_uri.as_deref(),
        Some("content://media/external/downloads/holiday.out"),
        "the row carries the platform's handle"
    );
    assert!(
        row.path.is_none(),
        "and not a path, which would name a file that is no longer there"
    );

    cancel.cancel();
    let _ = serve.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_download_survives_a_publisher_that_refuses() {
    let app = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let (ticket, data, cancel, serve) = served(app.path()).await;

    let publisher = Public::new(downloads.path(), true);
    let mgr = TransferManager::new(Identity::generate(), None, app.path().to_path_buf());
    mgr.set_publisher(publisher.clone());

    let staged = app.path().join("holiday.out");
    let id = mgr.start_download(
        ticket,
        staged.clone(),
        None,
        "holiday.bin".into(),
        data.len() as u64,
    );
    let row = wait_terminal(&mgr, id).await;

    assert_eq!(publisher.calls.lock().unwrap().len(), 1, "it was asked");
    assert_eq!(
        std::fs::read(&staged).unwrap(),
        data,
        "an hour of downloading is not thrown away because the move failed"
    );
    assert_eq!(
        row.path.as_deref(),
        Some(staged.as_path()),
        "the row still says where the file actually is"
    );
    assert!(row.saved_uri.is_none(), "nothing was published");

    cancel.cancel();
    let _ = serve.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_publisher_nothing_moves() {
    let app = tempfile::tempdir().unwrap();
    let (ticket, data, cancel, serve) = served(app.path()).await;

    let mgr = TransferManager::new(Identity::generate(), None, app.path().to_path_buf());
    let out = app.path().join("holiday.out");
    let id = mgr.start_download(
        ticket,
        out.clone(),
        None,
        "holiday.bin".into(),
        data.len() as u64,
    );
    let row = wait_terminal(&mgr, id).await;

    assert_eq!(std::fs::read(&out).unwrap(), data);
    assert_eq!(
        row.path.as_deref(),
        Some(out.as_path()),
        "the desktop is exactly what it was"
    );
    assert!(row.saved_uri.is_none());

    cancel.cancel();
    let _ = serve.await;
}
