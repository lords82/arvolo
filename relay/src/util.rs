//! Small shared helpers: time, claim tokens, constant-time comparison.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Current unix time in seconds.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn random_claim() -> String {
    let bytes: [u8; 16] = rand::random();
    data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
}

/// Delete files in `dir` that `keep` does not recognise and that have not been
/// touched for `grace_secs`.
///
/// Every other cleanup path here is row-driven: it deletes a file by naming it
/// from a database row. That can only ever free a file the database still knows
/// about — a file whose row never landed (the process died mid-write, or the
/// write succeeded and the insert did not) is invisible to it and would sit on
/// disk forever. This is the backstop that reconciles the directory against the
/// table. The grace period is what keeps it from racing a file being written
/// right now, whose row is about to appear.
///
/// Returns how many files were removed.
pub(crate) fn sweep_orphan_files(
    dir: &Path,
    grace_secs: u64,
    keep: impl Fn(&str) -> bool,
) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if keep(&name) {
            continue;
        }
        // Unreadable or future-dated mtime: treat as "just written" and leave it.
        let recent = meta
            .modified()
            .map(|m| {
                now.duration_since(m)
                    .map(|age| age.as_secs() < grace_secs)
                    .unwrap_or(true)
            })
            .unwrap_or(true);
        if recent {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Length-independent constant-time byte comparison (equal lengths only).
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
