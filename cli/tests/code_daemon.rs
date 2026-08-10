//! End-to-end proof that a short pairing code can live in the daemon: driven
//! through the real `arvolo` binary against a relay running in-process.
//!
//! Three things `arvolo code` could not do before, one test each:
//!   1. **return** — the command prints a code and exits, instead of blocking a
//!      terminal for as long as the code is wanted;
//!   2. **survive a restart** — the daemon is killed and started again, and the
//!      same code, already written down, still pairs (`--keep`);
//!   3. **resume a download** — an interrupted receive finishes from the ticket
//!      recorded beside the partial, without the code, which is consumed on use.
//!
//! Everything rides the HTTP relay on localhost, and the transfer itself is P2P
//! over iroh with no NAT relay, so the whole thing is deterministic.

// Unix only, and not by accident: `arvolo daemon` is itself `#[cfg(unix)]` —
// the control socket is a Unix socket — so on Windows there is no daemon for any
// of this to test. Compiled there anyway, the file failed on the unix-only file
// metadata it uses to measure a partial download, which read like a portability
// bug in the product rather than a test asking for something that does not exist.
#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use arvolo_core::backfill::BlobNode;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tempfile::TempDir;
use tokio::process::{Child, Command};

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

/// A `Command` for the built binary with an isolated config dir, identity and
/// relay. `ARVOLO_IDENTITY` is what keeps the two sides distinct — the identity
/// path does not follow `ARVOLO_CONFIG_DIR` (see `multidevice.rs`).
fn arvolo(cfg: &Path, relay: &str, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_arvolo"));
    c.args(args)
        .env("ARVOLO_CONFIG_DIR", cfg)
        .env("ARVOLO_IDENTITY", cfg.join("identity.key"))
        .env("ARVOLO_TEMP_DIR", cfg.join("tmp"))
        .env("ARVOLO_RELAY", relay)
        .env("ARVOLO_NO_WIZARD", "1")
        // Direct-only: no public NAT relay in a test.
        .env("ARVOLO_IROH_RELAY", "")
        .kill_on_drop(true);
    c
}

async fn run(cfg: &Path, relay: &str, args: &[&str]) -> (bool, String, String) {
    let out = arvolo(cfg, relay, args).output().await.expect("run arvolo");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Start a daemon and wait until it answers, so a following command doesn't race
/// the socket into existence.
async fn start_daemon(cfg: &Path, relay: &str) -> Child {
    let child = arvolo(cfg, relay, &["daemon"])
        .spawn()
        .expect("spawn daemon");
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if run(cfg, relay, &["status"]).await.0 {
            return child;
        }
    }
    panic!("daemon never came up");
}

/// The code out of `arvolo code`'s output: it prints the line a user copies.
fn scrape_code(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("arvolo recv "))
        .unwrap_or_else(|| panic!("no `arvolo recv <code>` line in:\n{stdout}"))
        .trim()
        .to_string()
}

fn write_payload(dir: &Path, name: &str, size: usize) -> std::path::PathBuf {
    let path = dir.join(name);
    // Compressible but not uniform, so a truncated partial is obviously wrong.
    let body: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &body).unwrap();
    path
}

#[tokio::test]
async fn code_is_hosted_by_the_daemon_and_the_command_returns() {
    let relay = spawn_relay().await;
    let (cfg_a, cfg_b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let file = write_payload(cfg_a.path(), "hello.bin", 64 * 1024);

    let _daemon = start_daemon(cfg_a.path(), &relay).await;

    // The whole point: this returns instead of blocking until someone shows up.
    let (ok, stdout, stderr) = tokio::time::timeout(
        Duration::from_secs(30),
        run(cfg_a.path(), &relay, &["code", file.to_str().unwrap()]),
    )
    .await
    .expect("`arvolo code` must not block when a daemon is running");
    assert!(ok, "code failed: {stderr}");
    let code = scrape_code(&stdout);
    assert_eq!(code.matches('-').count(), 2, "code grammar is unchanged");
    assert!(code.contains('@'), "a daemon-hosted code embeds its relay");

    // It is visible in the daemon's own list, so the code survives the terminal
    // that printed it.
    let (_, status, _) = run(cfg_a.path(), &relay, &["status"]).await;
    assert!(
        status.contains(&code),
        "the live code should show in `arvolo status`:\n{status}"
    );

    // And it works.
    let out_dir = cfg_b.path().join("in");
    std::fs::create_dir_all(&out_dir).unwrap();
    let (ok, _, stderr) = tokio::time::timeout(
        Duration::from_secs(60),
        run(
            cfg_b.path(),
            &relay,
            &["recv", &code, "--out", out_dir.to_str().unwrap()],
        ),
    )
    .await
    .expect("recv timed out");
    assert!(ok, "recv failed: {stderr}");
    assert_eq!(
        std::fs::read(out_dir.join("hello.bin")).unwrap(),
        std::fs::read(&file).unwrap(),
        "the receiver got the exact file"
    );
}

#[tokio::test]
async fn a_kept_code_survives_the_daemon_restarting() {
    let relay = spawn_relay().await;
    let (cfg_a, cfg_b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let file = write_payload(cfg_a.path(), "durable.bin", 32 * 1024);

    let mut daemon = start_daemon(cfg_a.path(), &relay).await;
    let (ok, stdout, stderr) = run(
        cfg_a.path(),
        &relay,
        &["code", "--keep", file.to_str().unwrap()],
    )
    .await;
    assert!(ok, "code --keep failed: {stderr}");
    let code = scrape_code(&stdout);

    // Kill the daemon outright — no graceful shutdown, no chance to hand anything
    // over. Under v1 the sender's PAKE state lived only in that process, so the
    // code died with it.
    daemon.kill().await.ok();
    daemon.wait().await.ok();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let _daemon = start_daemon(cfg_a.path(), &relay).await;
    // Give the restored code a moment to reattach to its rendezvous slot.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let (_, status, _) = run(cfg_a.path(), &relay, &["status"]).await;
    assert!(
        status.contains(&code),
        "the code should come back with the daemon:\n{status}"
    );

    let out_dir = cfg_b.path().join("in");
    std::fs::create_dir_all(&out_dir).unwrap();
    let (ok, _, stderr) = tokio::time::timeout(
        Duration::from_secs(60),
        run(
            cfg_b.path(),
            &relay,
            &["recv", &code, "--out", out_dir.to_str().unwrap()],
        ),
    )
    .await
    .expect("recv timed out");
    assert!(
        ok,
        "the same code, written down before the restart, must still pair: {stderr}"
    );
    assert_eq!(
        std::fs::read(out_dir.join("durable.bin")).unwrap(),
        std::fs::read(&file).unwrap()
    );
}

#[tokio::test]
async fn an_interrupted_download_resumes_without_the_code() {
    let relay = spawn_relay().await;
    let (cfg_a, cfg_b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    // Big enough that the receive is still going when we interrupt it.
    let file = write_payload(cfg_a.path(), "big.bin", 24 * 1024 * 1024);

    let _daemon = start_daemon(cfg_a.path(), &relay).await;
    let (ok, stdout, stderr) = run(cfg_a.path(), &relay, &["code", file.to_str().unwrap()]).await;
    assert!(ok, "code failed: {stderr}");
    let code = scrape_code(&stdout);

    let out_dir = cfg_b.path().join("in");
    std::fs::create_dir_all(&out_dir).unwrap();
    let partial = out_dir.join("big.bin");

    // Start the download and kill it partway.
    let mut recv = arvolo(
        cfg_b.path(),
        &relay,
        &["recv", &code, "--out", out_dir.to_str().unwrap()],
    )
    .spawn()
    .expect("spawn recv");
    // Wait until the ticket sidecar exists — that is the moment the receiver has
    // resolved the code and committed to a destination.
    let ticket_side = out_dir.join("big.bin.arvticket");
    for _ in 0..300 {
        if ticket_side.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        ticket_side.exists(),
        "the receiver should record its ticket beside the partial"
    );
    // Interrupt on a *condition*, not a stopwatch: wait until real bytes have
    // landed, then kill at once.
    //
    // A fixed sleep made this flaky under a loaded machine — with every other test
    // binary running its own daemons and relays, 1500 ms could pass before the
    // first byte was written. The old assertions then waved that through, because
    // a **zero-byte** partial satisfies both "it exists" and "it is smaller than
    // the whole file"; the run only fell over much later, at the byte comparison
    // after the resume, which is a long way from the thing that actually went
    // wrong. Requiring progress here means a resume is always given something to
    // resume *from*, and a genuine failure to start is reported where it happens.
    //
    // Progress is measured in *allocated blocks*, not in file length, because the
    // length of a partial download here is not a measure of anything. Pieces come
    // out of order, so the first one to land at a high offset extends the file to
    // (nearly) its final size while almost all of it is still holes — see
    // `sidecar.rs`, whose bitfield exists precisely because "there is no
    // length-based resume". Reading `len()` here made this test claim the download
    // had *completed* one poll after it started, and no payload size fixes that —
    // raising it to 64 MiB just produced "67108864 of 67108864 bytes" instead.
    //
    // Blocks only exist where real data was written, so this measures what has
    // genuinely arrived, and is what makes the kill land mid-transfer.
    const ENOUGH: u64 = 512 * 1024; // of 24 MiB — plainly started, nowhere near done
    let landed = |p: &Path| -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(p).map(|m| m.blocks() * 512).unwrap_or(0)
    };
    let mut got = 0;
    for _ in 0..6000 {
        got = landed(&partial);
        if got >= ENOUGH {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    recv.kill().await.ok();
    recv.wait().await.ok();

    assert!(
        got >= ENOUGH,
        "the download never got going ({got} bytes in 60s) — there is nothing to resume from"
    );
    let (stopped_at, whole) = (landed(&partial), std::fs::metadata(&file).unwrap().len());
    assert!(
        stopped_at < whole,
        "the receive finished ({stopped_at} of {whole} bytes actually written) before the kill \
         landed, so there is no interrupted download to resume"
    );

    // The code is spent — this is exactly the situation that used to be a dead
    // end, with a good partial on disk and no way back to the sender.
    // Resume from the partial instead.
    let (ok, _, stderr) = tokio::time::timeout(
        Duration::from_secs(120),
        run(cfg_b.path(), &relay, &["resume", partial.to_str().unwrap()]),
    )
    .await
    .expect("resume timed out");
    assert!(ok, "resume failed: {stderr}");
    assert_eq!(
        std::fs::read(&partial).unwrap(),
        std::fs::read(&file).unwrap(),
        "the resumed download finishes byte-for-byte"
    );
    assert!(
        !ticket_side.exists(),
        "the ticket sidecar is cleaned up once the download completes"
    );
}
