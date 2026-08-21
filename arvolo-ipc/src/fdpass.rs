//! A one-shot side channel that hands **open file descriptors** from the CLI to
//! the daemon.
//!
//! Why it exists: macOS keeps Downloads/Desktop/Documents behind per-app consent
//! (TCC), and the consent *prompt* is only ever shown on behalf of a process the
//! user can see — the terminal, an app window. A background daemon gets a silent
//! `Operation not permitted` instead, so a `send` queued by path used to sit at
//! "active, 0 B" forever. The CLI, however, runs in the user's terminal and CAN
//! open the file (macOS prompts for the terminal, once). So the CLI opens the
//! source and passes the daemon the open descriptor, not the path: a descriptor
//! is a capability — permission was settled at open time, and `SCM_RIGHTS` is
//! the unix mechanism purpose-built to hand one across processes.
//!
//! The same code works on Linux (SCM_RIGHTS is portable unix), where it also
//! covers a daemon hardened with e.g. systemd `ProtectHome=`. Windows has no
//! per-app folder consent — same-user processes see the same files — so this
//! module is unix-only and Windows keeps plain paths.
//!
//! Shape: the sender binds a private socket next to `daemon.sock` (0600, random
//! suffix), tells the daemon its path and a random token inside the ordinary
//! JSON request, and serves exactly one connection: token line in, one byte +
//! the descriptors out. Everything times out; an old daemon that ignores the
//! new fields simply never connects and the offer thread folds up quietly —
//! path behavior, as before.

use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use sendfd::{RecvWithFd, SendWithFd};

/// SCM_RIGHTS payloads are bounded by the kernel (253 on Linux); stay far below.
pub const MAX_FDS: usize = 64;

/// How long the offer stays open for the daemon to collect, and how long either
/// side waits on the socket. The daemon fetches immediately after reading the
/// request, so seconds are plenty; the timeout only bites with an old daemon
/// that will never come.
const WINDOW: Duration = Duration::from_secs(15);

/// The sender half: serve `files` once, to whoever presents `token`.
///
/// Returns the socket path and token to embed in the request. The thread lives
/// at most [`WINDOW`] and removes the socket file on its way out; dropping the
/// handle does not block on it.
pub fn offer_files(files: Vec<std::fs::File>) -> Result<FdOffer> {
    anyhow::ensure!(!files.is_empty() && files.len() <= MAX_FDS, "1..={MAX_FDS} files");
    let dir = crate::socket_path()
        .parent()
        .context("socket dir")?
        .to_path_buf();
    let token = random_token();
    let sock = dir.join(format!("fdpass-{}-{}.sock", std::process::id(), &token[..8]));
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock)
        .with_context(|| format!("bind {}", sock.display()))?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).ok();
    // Poll-accept so the thread can give up: a blocking accept with no peer
    // would pin the thread past any usefulness.
    listener.set_nonblocking(true)?;

    let sock_cleanup = sock.clone();
    let expect = token.clone();
    let thread = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + WINDOW;
        let conn = loop {
            match listener.accept() {
                Ok((s, _)) => break Some(s),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break None,
            }
        };
        if let Some(mut s) = conn {
            let _ = serve_one(&mut s, &expect, &files);
        }
        let _ = std::fs::remove_file(&sock_cleanup);
    });

    Ok(FdOffer {
        socket: sock,
        token,
        _thread: thread,
    })
}

/// A pending descriptor hand-off; keep it alive until the request round-trips.
pub struct FdOffer {
    pub socket: PathBuf,
    pub token: String,
    _thread: std::thread::JoinHandle<()>,
}

fn serve_one(s: &mut UnixStream, expect: &str, files: &[std::fs::File]) -> Result<()> {
    s.set_read_timeout(Some(WINDOW))?;
    s.set_write_timeout(Some(WINDOW))?;
    let mut buf = [0u8; 128];
    let mut got = Vec::new();
    loop {
        let n = s.read(&mut buf)?;
        anyhow::ensure!(n > 0, "peer hung up before the token");
        got.extend_from_slice(&buf[..n]);
        if got.last() == Some(&b'\n') {
            break;
        }
        anyhow::ensure!(got.len() < 128, "token line too long");
    }
    let presented = std::str::from_utf8(&got)?.trim();
    // Not a real secret (the socket is already 0600 in the user's own runtime
    // dir); the token just pairs THIS offer with THIS request.
    anyhow::ensure!(presented == expect, "wrong token");
    let fds: Vec<RawFd> = files.iter().map(|f| f.as_raw_fd()).collect();
    s.send_with_fd(&[fds.len() as u8], &fds)
        .context("send descriptors")?;
    Ok(())
}

/// The daemon half: collect `expect` descriptors from `socket`, as [`std::fs::File`]s.
///
/// Blocking — call it from `spawn_blocking`.
pub fn take_files(socket: &Path, token: &str, expect: usize) -> Result<Vec<std::fs::File>> {
    anyhow::ensure!((1..=MAX_FDS).contains(&expect), "1..={MAX_FDS} files");
    let mut s = UnixStream::connect(socket)
        .with_context(|| format!("connect {}", socket.display()))?;
    s.set_read_timeout(Some(WINDOW))?;
    s.set_write_timeout(Some(WINDOW))?;
    s.write_all(format!("{token}\n").as_bytes())?;
    let mut byte = [0u8; 1];
    let mut raw = vec![-1 as RawFd; MAX_FDS];
    let (n, nfds) = s.recv_with_fd(&mut byte, &mut raw).context("receive descriptors")?;
    anyhow::ensure!(n == 1, "peer sent no descriptor frame");
    anyhow::ensure!(
        nfds == expect && byte[0] as usize == expect,
        "expected {expect} descriptors, got {nfds}"
    );
    // From here the fds are ours to own — wrap immediately so nothing leaks.
    Ok(raw[..nfds]
        .iter()
        .map(|&fd| unsafe { std::fs::File::from_raw_fd(fd) })
        .collect())
}

/// The path a process can re-open one of its own descriptors through: opening
/// `/dev/fd/N` duplicates descriptor N (macOS) or re-opens its file (Linux) —
/// either way, no path permission is consulted, which is the whole point. Valid
/// only while the original descriptor stays open: keep the [`std::fs::File`]
/// alive as long as anything might use this path.
pub fn fd_path(file: &std::fs::File) -> PathBuf {
    PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd()))
}

fn random_token() -> String {
    // No rand dep here; pairing (not secrecy) is the job — see `serve_one`.
    use std::hash::{Hash, Hasher};
    static SALT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::process::id().hash(&mut h);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .hash(&mut h);
    let a = h.finish();
    SALT.fetch_add(a | 1, std::sync::atomic::Ordering::Relaxed).hash(&mut h);
    format!("{a:016x}{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};

    #[test]
    fn descriptors_cross_and_reopen_through_dev_fd() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        let mut f = tempfile::tempfile().unwrap();
        f.write_all(b"across the socket").unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();

        let offer = offer_files(vec![f]).unwrap();
        let got = take_files(&offer.socket, &offer.token, 1).unwrap();
        assert_eq!(got.len(), 1);

        // The received fd reads the same bytes…
        let mut s = String::new();
        (&got[0]).read_to_string(&mut s).unwrap();
        assert_eq!(s, "across the socket");

        // …and /dev/fd re-opens it without consulting any path. On macOS the
        // open is a dup (shared offset, left at EOF by the read above), on
        // Linux a fresh open — seek explicitly so both read from the start.
        let mut again = std::fs::File::open(fd_path(&got[0])).unwrap();
        again.seek(SeekFrom::Start(0)).unwrap();
        let mut s2 = String::new();
        again.read_to_string(&mut s2).unwrap();
        assert_eq!(s2, "across the socket");
    }

    #[test]
    fn a_wrong_token_gets_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        let f = tempfile::tempfile().unwrap();
        let offer = offer_files(vec![f]).unwrap();
        assert!(take_files(&offer.socket, "not-the-token", 1).is_err());
    }
}
