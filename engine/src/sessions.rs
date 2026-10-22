//! Persisted send sessions, so an interrupted `arvolo send` can be resumed and
//! its ticket stays valid — even without a relay. Each record carries the
//! per-transfer content key (a capability secret), so its file is written with
//! owner-only permissions under `~/.config/arvolo/sessions`.
//!
//! Two recovery paths use this differently (both via `send --resume <arg>`):
//!   - a *plain* `arvc…` ticket is self-describing, so `send --resume <arvc…> <file>`
//!     needs no stored record at all (the key is in the ticket);
//!   - a *sealed* (`--to`) ticket hides the key from the sender, so recovery
//!     relies on the record saved here, addressed by a short session id
//!     (`send --resume <id>`).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use arvolo_core::crypto::CHUNK_KEY_LEN;
use arvolo_core::reexport::Hash;
use data_encoding::HEXLOWER;
use serde::{Deserialize, Serialize};

use crate::book;

fn sessions_dir() -> PathBuf {
    book::config_dir().join("sessions")
}

fn record_path(id: &str) -> PathBuf {
    sessions_dir().join(format!("{id}.toml"))
}

/// A resumable send. We store the original *source paths* (not a copy of the
/// bytes): a single file is re-served as-is, while an archive is repacked
/// deterministically — [`arvolo_core::flow::pack_tar`] produces byte-identical
/// tars from the same inputs, so the resumed chunk hashes match the ticket.
#[derive(Serialize, Deserialize, Clone)]
pub struct SendRecord {
    pub id: String,
    /// Per-transfer content key, lowercase hex. Secret — the file is 0600.
    pub key_hex: String,
    /// Transport node seed, lowercase hex. Rebinds the same node id on resume so
    /// the *original* ticket reconnects. Secret — the file is 0600.
    pub node_key_hex: String,
    /// Absolute source paths originally sent (one file, or the archive inputs).
    pub sources: Vec<PathBuf>,
    /// Suggested output name shown to the receiver.
    pub name: String,
    /// The payload is a tar archive (folder / multiple files) to repack.
    pub archive: bool,
    pub total_size: u64,
    pub chunks: usize,
    /// Unix seconds when the session was created.
    pub created: u64,
    /// The ticket originally handed out (also carries the `--to` key delivery).
    pub ticket: String,
}

impl SendRecord {
    /// The raw content key, decoded from hex.
    pub fn key(&self) -> Result<[u8; CHUNK_KEY_LEN]> {
        decode_fixed(&self.key_hex, "session content key")
    }

    /// The raw transport node seed, decoded from hex.
    pub fn node_seed(&self) -> Result<[u8; 32]> {
        decode_fixed(&self.node_key_hex, "session node seed")
    }
}

fn decode_fixed<const N: usize>(hex: &str, what: &str) -> Result<[u8; N]> {
    let bytes = HEXLOWER
        .decode(hex.as_bytes())
        .with_context(|| format!("decode {what}"))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("{what} must be {N} bytes"))
}

/// A short, stable id for a ticket (first 4 bytes of its BLAKE3, hex).
pub fn id_for(ticket: &str) -> String {
    HEXLOWER.encode(&Hash::new(ticket.as_bytes()).as_bytes()[..4])
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Build and persist a record for a freshly prepared send. `sources` are the
/// original inputs, resolved to absolute paths so resume works from any cwd.
#[allow(clippy::too_many_arguments)]
pub fn save(
    key: [u8; CHUNK_KEY_LEN],
    node_seed: [u8; 32],
    sources: &[PathBuf],
    name: &str,
    archive: bool,
    total_size: u64,
    chunks: usize,
    ticket: &str,
) -> Result<SendRecord> {
    let sources = sources
        .iter()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    let rec = SendRecord {
        id: id_for(ticket),
        key_hex: HEXLOWER.encode(&key),
        node_key_hex: HEXLOWER.encode(&node_seed),
        sources,
        name: name.to_string(),
        archive,
        total_size,
        chunks,
        created: now_secs(),
        ticket: ticket.to_string(),
    };
    write_record(&rec)?;
    Ok(rec)
}

fn write_record(rec: &SendRecord) -> Result<()> {
    let dir = sessions_dir();
    std::fs::create_dir_all(&dir).context("create sessions dir")?;
    let s = toml::to_string_pretty(rec).context("serialize session")?;
    let path = record_path(&rec.id);
    std::fs::write(&path, s).with_context(|| format!("write session {}", path.display()))?;
    restrict(&path);
    Ok(())
}

/// Load a session record by id.
pub fn load(id: &str) -> Result<SendRecord> {
    let path = record_path(id);
    let s = std::fs::read_to_string(&path)
        .map_err(|_| anyhow!("no saved session '{id}' (see `arvolo status`)"))?;
    toml::from_str(&s).with_context(|| format!("parse session {}", path.display()))
}

/// How long a resumable-send record is kept before the sweep reaps it. A month:
/// a ticket someone still means to redeem is long dead by then, and a record
/// nobody ever cleans (the historical norm — only `cancel` removed them) stops
/// accumulating forever.
const SWEEP_AFTER_SECS: u64 = 30 * 24 * 3600;

/// All saved sessions, newest first. Records older than [`SWEEP_AFTER_SECS`]
/// are deleted on the way through — every reader sweeps, so no daemon or timer
/// has to.
pub fn list() -> Vec<SendRecord> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut out: Vec<SendRecord> = std::fs::read_dir(sessions_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| toml::from_str::<SendRecord>(&s).ok())
        .filter(|r| {
            let expired = now.saturating_sub(r.created) > SWEEP_AFTER_SECS;
            if expired {
                let _ = std::fs::remove_file(record_path(&r.id));
            }
            !expired
        })
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.created));
    out
}

/// Delete a session record if it exists — for the delivered path, where "there
/// was nothing to delete" is not an error worth surfacing.
pub fn remove_if_present(id: &str) {
    let _ = std::fs::remove_file(record_path(id));
}

/// Delete a session record by id.
pub fn remove(id: &str) -> Result<()> {
    load(id)?; // 404s with a helpful message if unknown
    std::fs::remove_file(record_path(id)).with_context(|| format!("remove session '{id}'"))?;
    Ok(())
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
