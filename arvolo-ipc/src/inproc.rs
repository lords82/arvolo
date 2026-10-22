//! Handing open files to an engine in the same process.
//!
//! A phone runs the engine *inside* the app: there are no two processes for
//! `SCM_RIGHTS` to carry a descriptor between. But the reason for handing over a
//! descriptor rather than a path is the same one, only stronger — Android hands
//! a picked file over as a `content://` URI, which is not a path at all and can
//! never be opened by name. The app resolves it to an open file; the engine has
//! to read *that*, or nothing.
//!
//! So: the same two fields in the same request, and the same shape at both ends
//! (leave the files, name a token, collect them once). Only the middle changes,
//! from a unix socket to a map. `socket` carries [`IN_PROCESS`] instead of a
//! path, which is how the daemon knows which way to collect — and, being a name
//! no filesystem can produce, a request from a *different* process can never
//! claim it by accident.

use anyhow::{Context, Result};

/// The `socket` value that means "the files are in this process, not on a
/// socket". Deliberately not a valid path.
pub const IN_PROCESS: &str = "@arvolo-in-process";

static STASH: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, Vec<std::fs::File>>>,
> = std::sync::OnceLock::new();

fn stash() -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<std::fs::File>>> {
    STASH.get_or_init(Default::default)
}

/// Leave `files` for the engine in this same process, and return the token that
/// collects them. Pair it with [`IN_PROCESS`] as the socket.
pub fn stash_files(files: Vec<std::fs::File>) -> String {
    let token = random_token();
    stash()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(token.clone(), files);
    token
}

/// Collect what [`stash_files`] left, once. A second call with the same token
/// gets nothing: like the socket, this hand-off is one-shot, so a token that
/// leaked cannot be replayed into a second send.
pub fn take_stashed(token: &str) -> Result<Vec<std::fs::File>> {
    stash()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(token)
        .with_context(|| format!("no descriptors stashed for {token}"))
}

/// Drop what a send never collected — a sheet closed, a send that failed before
/// it reached the engine. Without this a picked-and-abandoned file would hold
/// its descriptor open for the life of the process.
pub fn drop_stashed(token: &str) {
    stash()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(token);
}

/// A one-shot pairing token. No rand dependency here: pairing, not secrecy, is
/// the job — the socket hand-off checks it beside a path only its owner can
/// reach, and the stash is a map inside one process.
pub(crate) fn random_token() -> String {
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
    SALT.fetch_add(a | 1, std::sync::atomic::Ordering::Relaxed)
        .hash(&mut h);
    format!("{a:016x}{:016x}", h.finish())
}

#[cfg(test)]
mod in_process_tests {
    use super::*;
    use std::io::Write as _;
    use std::io::{Seek as _, SeekFrom};

    /// The phone's whole send depends on this: a file the app opened has to
    /// reach the engine in the same process, once, and be readable there.
    #[test]
    fn files_left_here_are_collected_once() {
        let mut f = tempfile::tempfile().unwrap();
        f.write_all(b"picked, not opened by name").unwrap();
        let token = stash_files(vec![f]);

        let mut got = take_stashed(&token).unwrap();
        assert_eq!(got.len(), 1);
        got[0].seek(SeekFrom::Start(0)).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut got[0], &mut s).unwrap();
        assert_eq!(s, "picked, not opened by name");

        // One-shot, like the socket: a token that leaked cannot be replayed
        // into a second send.
        assert!(take_stashed(&token).is_err());
    }

    #[test]
    fn an_unknown_token_gets_nothing() {
        assert!(take_stashed("never-stashed").is_err());
    }

    /// A sheet closed without sending must not leave the descriptor open for
    /// the life of the app.
    #[test]
    fn abandoned_files_can_be_given_back() {
        let token = stash_files(vec![tempfile::tempfile().unwrap()]);
        drop_stashed(&token);
        assert!(take_stashed(&token).is_err());
    }

    /// Two sends in flight must not be able to collect each other's files.
    #[test]
    fn tokens_do_not_collide() {
        let a = stash_files(vec![tempfile::tempfile().unwrap()]);
        let b = stash_files(vec![tempfile::tempfile().unwrap()]);
        assert_ne!(a, b);
        assert!(take_stashed(&a).is_ok());
        assert!(take_stashed(&b).is_ok());
    }
}
