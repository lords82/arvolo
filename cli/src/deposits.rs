//! Persisted relay-deposit sessions. Every `send-offline` — whether sealed to a
//! contact (`arvm` ticket) or a public browser `--link` — leaves the encrypted
//! file on a relay. We record it here, including the sender-only **revoke
//! token**, so the sender can later list and cancel it without having kept the
//! printed token. Removing a record revokes the blob on the relay (see
//! `sessions rm`), so the file/link lives exactly as long as this local session.
//!
//! A record holds the revoke token (a capability secret), so its file is written
//! owner-only (0600), like the resumable-send [`crate::sessions`] store.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use arvolo_core::reexport::Hash;
use data_encoding::HEXLOWER;
use serde::{Deserialize, Serialize};

use crate::book;

/// Record kind: a public download link vs a recipient-sealed offline ticket.
pub const KIND_LINK: &str = "link";
pub const KIND_OFFLINE: &str = "offline";

/// Sentinel download cap meaning "no limit" (a link's default). Passed to the
/// relay as-is; the relay clamps it to its own maximum.
pub const UNLIMITED: u32 = u32::MAX;

fn deposits_dir() -> PathBuf {
    book::config_dir().join("deposits")
}

fn record_path(id: &str) -> PathBuf {
    deposits_dir().join(format!("{id}.toml"))
}

/// A file left on a relay by `send-offline`.
#[derive(Serialize, Deserialize, Clone)]
pub struct DepositRecord {
    pub id: String,
    /// [`KIND_LINK`] (public download URL) or [`KIND_OFFLINE`] (sealed `arvm`).
    pub kind: String,
    pub relay: String,
    pub claim: String,
    /// Sender-only revoke secret. Secret — the file is 0600.
    pub revoke_token: String,
    pub name: String,
    pub size: u64,
    /// Download cap requested at deposit. `u32::MAX` means effectively unlimited
    /// (the relay clamps it to its own cap); a link defaults to this, a sealed
    /// offline deposit defaults to 1 (burn-after-read).
    pub max: u32,
    /// For a public link: the full browser URL. `None` for a sealed deposit.
    pub link: Option<String>,
    /// For a sealed deposit: the recipient's base32 id. `None` for a link.
    pub recipient: Option<String>,
    pub created: u64,
    /// Unix seconds when the relay auto-expires the blob (`created + ttl`).
    pub expires: u64,
}

impl DepositRecord {
    /// Whether the relay TTL has already elapsed (the blob is likely gone).
    pub fn expired(&self) -> bool {
        now_secs() >= self.expires
    }

    /// A human label for the download cap (`unlimited` for the link default).
    pub fn max_label(&self) -> String {
        if self.max == UNLIMITED {
            "unlimited".to_string()
        } else {
            self.max.to_string()
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A short, stable id derived from the claim (first 4 bytes of its BLAKE3, hex).
pub fn id_for(claim: &str) -> String {
    HEXLOWER.encode(&Hash::new(claim.as_bytes()).as_bytes()[..4])
}

/// Build and persist a deposit session record. Returns it (its `id` is what the
/// user passes to `sessions rm`).
#[allow(clippy::too_many_arguments)]
pub fn save(
    kind: &str,
    relay: &str,
    claim: &str,
    revoke_token: &str,
    name: &str,
    size: u64,
    max: u32,
    link: Option<String>,
    recipient: Option<String>,
    ttl: u64,
) -> Result<DepositRecord> {
    let created = now_secs();
    let rec = DepositRecord {
        id: id_for(claim),
        kind: kind.to_string(),
        relay: relay.to_string(),
        claim: claim.to_string(),
        revoke_token: revoke_token.to_string(),
        name: name.to_string(),
        size,
        max,
        link,
        recipient,
        created,
        expires: created.saturating_add(ttl),
    };
    let dir = deposits_dir();
    std::fs::create_dir_all(&dir).context("create deposits dir")?;
    let s = toml::to_string_pretty(&rec).context("serialize deposit")?;
    let path = record_path(&rec.id);
    std::fs::write(&path, s).with_context(|| format!("write deposit {}", path.display()))?;
    restrict(&path);
    Ok(rec)
}

/// Load a deposit record by id (`None` if there is no such record).
pub fn load(id: &str) -> Option<DepositRecord> {
    let s = std::fs::read_to_string(record_path(id)).ok()?;
    toml::from_str(&s).ok()
}

/// All saved deposit sessions, newest first.
pub fn list() -> Vec<DepositRecord> {
    let mut out: Vec<DepositRecord> = std::fs::read_dir(deposits_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| toml::from_str::<DepositRecord>(&s).ok())
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.created));
    out
}

/// Delete a deposit record locally (does not touch the relay).
pub fn remove(id: &str) -> Result<()> {
    std::fs::remove_file(record_path(id)).with_context(|| format!("remove deposit '{id}'"))?;
    Ok(())
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deposit_record_roundtrips_and_lists() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let rec = save(
            KIND_LINK,
            "https://relay.example",
            "abc123claim",
            "revoketoken",
            "photo.jpg",
            4242,
            UNLIMITED,
            Some("https://relay.example/dl/abc123claim#key".into()),
            None,
            3600,
        )
        .unwrap();
        assert_eq!(rec.kind, KIND_LINK);
        assert_eq!(rec.expires, rec.created + 3600);
        assert_eq!(rec.max_label(), "unlimited");
        assert!(!rec.expired());

        let loaded = load(&rec.id).expect("load");
        assert_eq!(loaded.claim, "abc123claim");
        assert_eq!(loaded.revoke_token, "revoketoken");
        assert_eq!(list().len(), 1);

        remove(&rec.id).unwrap();
        assert!(load(&rec.id).is_none());
        assert!(list().is_empty());

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
