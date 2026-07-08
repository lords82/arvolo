//! The daemon's download engine must honor the resume sidecar: after a
//! receiver-side crash leaves a partial output + `.arvhave` bitfield, restarting
//! the download (`TransferManager::start_download`, which is exactly what
//! `resume_incomplete` re-drives per persisted record) resumes from the sidecar
//! and fetches only the missing pieces, then completes byte-identically.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arvolo_core::crypto::Identity;
use arvolo_core::flow;
use arvolo_core::manager::TransferManager;
use arvolo_core::swarm::{bitfield_new, bitfield_set};
use arvolo_core::transfer::RelayChoice;
use tokio_util::sync::CancellationToken;

const CHUNK: usize = 16 * 1024 * 1024;

/// Lay down `present` chunks of `data` at their true offsets + a resume sidecar.
fn seed_partial(out: &Path, data: &[u8], total: usize, present: &[usize]) {
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

async fn wait_for_file(path: &Path, data: &[u8], within: Duration) -> bool {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn daemon_download_resumes_a_partial_from_the_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("m.bin");
    // 48 MiB -> 3 chunks.
    let data: Vec<u8> = (0..48 * 1024 * 1024u64)
        .map(|i| (i * 29 + 5) as u8)
        .collect();
    std::fs::write(&src, &data).unwrap();

    let session = flow::prepare_send(&src, "m.bin", false, None, None, RelayChoice::Disabled)
        .await
        .unwrap();
    let ticket = session.ticket.clone();
    let sc = CancellationToken::new();
    let serve = {
        let c = sc.clone();
        tokio::spawn(async move { session.serve(c, |_| {}).await })
    };

    // A crashed receiver left chunk 0 on disk (+ sidecar); {1,2} still missing.
    let out = dir.path().join("m.out");
    seed_partial(&out, &data, 3, &[0]);

    // The daemon engine re-drives the download to the same output path.
    let me = Identity::generate();
    let mgr = TransferManager::new(me, None, dir.path().to_path_buf());
    let _id = mgr.start_download(ticket, out.clone(), None, "m.bin".into(), data.len() as u64);

    assert!(
        wait_for_file(&out, &data, Duration::from_secs(60)).await,
        "manager-driven download resumed from the sidecar and completed"
    );
    assert!(
        !PathBuf::from(format!("{}.arvhave", out.display())).exists(),
        "resume sidecar is cleaned up once the download completes"
    );

    sc.cancel();
    let _ = serve.await;
}
