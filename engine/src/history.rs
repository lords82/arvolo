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
    /// Where a received file came to rest — the published location, not the
    /// staging one. Written so a client that rebuilds its list from here can
    /// still offer to open the file; `None` for a send, and for every record
    /// written before this field existed.
    ///
    /// This one is TOML, not postcard, so `default` is a real tolerance and not
    /// the decorative kind — see docs/PROTOCOL.md.
    #[serde(default)]
    pub path: Option<String>,
    /// The platform's own handle when there is no path — an Android
    /// `content://`. Same reason as [`arvolo_core::manager::Transfer::saved_uri`].
    #[serde(default)]
    pub saved_uri: Option<String>,
    /// The row was taken off the board by hand.
    ///
    /// Not a deletion: `remove` has always meant "forget the row, not the deed",
    /// and the log is the deed. What changed is that the board is now *rebuilt*
    /// from this log where the engine does not outlive the window — so without
    /// somewhere to write "they did not want to see this", every removed row came
    /// back at the next launch, which is a control that undoes itself.
    #[serde(default)]
    pub hidden: bool,
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
#[allow(clippy::too_many_arguments)]
pub fn record(
    direction: &str,
    peer_id: Option<String>,
    name: &str,
    total_size: u64,
    transferred: u64,
    status: &str,
    path: Option<String>,
    saved_uri: Option<String>,
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
        path,
        saved_uri,
        hidden: false,
    };
    write_record(&rec)?;
    Ok(rec)
}

/// Persist a finished transfer the engine already knows about (best-effort).
///
/// Every terminal event goes through here, whichever client is driving — the
/// daemon's intake loop, a one-shot `send`, a `recv` in a terminal — which is
/// also why the address-book stamp rides along: "when did I last deal with this
/// person" is answered by a file arriving or leaving, and this is the one place
/// both directions pass through already holding the peer. A second hook would be
/// a second place to forget.
pub fn record_transfer(manager: &arvolo_core::manager::TransferManager, id: u64, status: &str) {
    use arvolo_core::manager::Direction;

    let Some(t) = manager.get(id) else { return };
    let direction = match t.direction {
        Direction::Send => "send",
        Direction::Recv => "recv",
    };
    let peer_id = t.peer.as_ref().map(crate::identity::encode_id);
    if status == "completed" {
        if let Some(pk) = &peer_id {
            book::record_exchange(pk, crate::now_unix());
        }
    }
    let _ = record(
        direction,
        peer_id,
        &t.name,
        t.total_size,
        t.transferred,
        status,
        t.path.as_ref().map(|p| p.display().to_string()),
        t.saved_uri.clone(),
    );
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

/// Put the most recent finished transfers back on the manager's list, newest
/// first, and return how many went.
///
/// For an embedder whose engine restarts with its window — which is a phone, and
/// only a phone. A desktop daemon holds its list for weeks and calling this there
/// would double every row it already has.
///
/// `limit` is how far back the mailbox reaches. Not "all of it": the log is the
/// log, it has its own screen, and a home screen that grows without bound is one
/// that gets slower every month it is used.
pub fn restore_board(manager: &arvolo_core::manager::TransferManager, limit: usize) -> usize {
    use arvolo_core::manager::{Direction, FinishedRow, TransferStatus};

    let mut n = 0;
    // Oldest first, so the ids the manager allocates run in the same order as the
    // events did: anything that sorts by id — and the prune that keeps the map
    // bounded — then agrees with the clock.
    let mut recent: Vec<HistoryRecord> = list()
        .into_iter()
        .filter(|r| !r.hidden)
        .take(limit)
        .collect();
    recent.reverse();
    for rec in recent {
        let status = match rec.status.as_str() {
            "completed" => TransferStatus::Completed,
            "cancelled" => TransferStatus::Cancelled,
            // "failed: <reason>", and anything a later version writes that this
            // one does not know: a row that says it did not finish is closer to
            // the truth than no row at all.
            other => TransferStatus::Failed(
                other.strip_prefix("failed: ").unwrap_or(other).to_string(),
            ),
        };
        let row = FinishedRow {
            direction: if rec.direction == "send" {
                Direction::Send
            } else {
                Direction::Recv
            },
            peer: rec
                .peer_id
                .as_deref()
                .and_then(|s| crate::book::decode_id(s).ok()),
            name: rec.name,
            total_size: rec.total_size,
            transferred: rec.transferred,
            status,
            created: rec.created,
            path: rec.path.map(std::path::PathBuf::from),
            saved_uri: rec.saved_uri,
        };
        if manager.adopt_finished(row).is_some() {
            n += 1;
        }
    }
    n
}

/// Take a row off the board for good: the deed stays in the log, and the log
/// stops putting it back.
///
/// Matched on what a record and a row have in common — who, what, how big, and
/// when — because the ids do not survive: a history record is filed under a hash
/// and a transfer id is allocated per run (see `restore_board`, which allocates
/// fresh ones for the rows it puts back).
pub fn hide(t: &arvolo_core::manager::Transfer) {
    use arvolo_core::manager::Direction;
    let direction = match t.direction {
        Direction::Send => "send",
        Direction::Recv => "recv",
    };
    for rec in list() {
        if rec.hidden
            || rec.direction != direction
            || rec.name != t.name
            || rec.total_size != t.total_size
        {
            continue;
        }
        // The clock is the tie-breaker, and it is not exact: a restored row
        // carries the record's `created`, a live one carries its own start,
        // which is earlier than the record written when it ended. A minute is
        // wide enough for that gap and narrow enough that two sends of the same
        // file to the same person are only confused when they were a minute
        // apart — in which case either answer is the one the person meant.
        if rec.created.abs_diff(t.created) > 60 {
            continue;
        }
        let hidden = HistoryRecord {
            hidden: true,
            ..rec
        };
        let _ = write_record(&hidden);
        return;
    }
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

/// Not a no-op by oversight: on Windows a file's access comes from the list on
/// the directory it is created in, which [`crate::book::restrict_config_dir`]
/// narrows once at startup. Re-applying it per file would spend a process
/// spawn on each record for a protection the file already inherited.
#[cfg(not(unix))]
fn restrict(_path: &Path) {}
