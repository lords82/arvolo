//! Persisted transfer history, so a stay-open client remembers past transfers
//! across restarts. Mirrors the `sessions` store: one TOML file per record under
//! `~/.config/arvolo/history`, written owner-only (a record names a peer, which is
//! not a secret, but we match the sessions store's 0600 posture anyway).
//!
//! The `TransferManager` keeps live transfers in memory; the CLI event loop writes
//! a record here on each terminal event (completed / failed / cancelled).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use arvolo_core::reexport::Hash;
use data_encoding::HEXLOWER;
use serde::{Deserialize, Serialize};

use crate::book;

fn history_dir() -> PathBuf {
    book::config_dir().join("history")
}

fn record_path(id: &str) -> PathBuf {
    history_dir().join(format!("{id}.toml"))
}

/// A finished transfer, as shown by `arvolo history`.
#[derive(Serialize, Deserialize, Clone)]
pub struct HistoryRecord {
    pub id: String,
    /// "send" or "recv".
    pub direction: String,
    /// The peer's base32 public id (recipient for a send, sender for a receive).
    pub peer_id: Option<String>,
    pub name: String,
    pub total_size: u64,
    pub transferred: u64,
    /// "completed" / "failed" / "cancelled" (plus an optional reason for failed).
    pub status: String,
    /// Unix seconds when the record was written.
    pub created: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Persist one finished transfer. The file id is derived from a high-resolution
/// timestamp + name, so concurrent terminal events don't collide.
pub fn record(
    direction: &str,
    peer_id: Option<String>,
    name: &str,
    total_size: u64,
    transferred: u64,
    status: &str,
) -> Result<HistoryRecord> {
    let seed = format!("{}:{name}:{status}", now_nanos());
    let id = HEXLOWER.encode(&Hash::new(seed.as_bytes()).as_bytes()[..6]);
    let rec = HistoryRecord {
        id,
        direction: direction.to_string(),
        peer_id,
        name: name.to_string(),
        total_size,
        transferred,
        status: status.to_string(),
        created: now_secs(),
    };
    write_record(&rec)?;
    Ok(rec)
}

fn write_record(rec: &HistoryRecord) -> Result<()> {
    let dir = history_dir();
    std::fs::create_dir_all(&dir).context("create history dir")?;
    let s = toml::to_string_pretty(rec).context("serialize history record")?;
    let path = record_path(&rec.id);
    std::fs::write(&path, s).with_context(|| format!("write history {}", path.display()))?;
    restrict(&path);
    Ok(())
}

/// All history records, newest first.
pub fn list() -> Vec<HistoryRecord> {
    let mut out: Vec<HistoryRecord> = std::fs::read_dir(history_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| toml::from_str::<HistoryRecord>(&s).ok())
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.created));
    out
}

/// Delete all history records; returns how many were removed.
pub fn clear() -> Result<usize> {
    let mut n = 0;
    for entry in std::fs::read_dir(history_dir())
        .into_iter()
        .flatten()
        .flatten()
    {
        if entry.path().extension().is_some_and(|x| x == "toml")
            && std::fs::remove_file(entry.path()).is_ok()
        {
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}
