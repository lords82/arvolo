use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

/// What we know about a sender before recording this receipt: their contact name
/// (if saved), whether we've received from them before (TOFU), whether the user
/// has verified their identity out-of-band, and whether the user trusts them to
/// auto-download without a prompt.
pub struct SenderStatus {
    pub name: Option<String>,
    pub seen_before: bool,
    pub verified: bool,
    /// The standing "don't ask me about this person" from `contacts trust`. Read
    /// by the daemon's auto-download policy and by an inline `listen`, so it is
    /// consulted on every platform.
    pub trusted: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Seen {
    #[serde(default)]
    pub(crate) seen: BTreeMap<String, u64>,
}

pub(crate) fn load_seen() -> Seen {
    std::fs::read_to_string(seen_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_seen(s: &Seen) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(s).context("serialize seen")?;
    write_private(&seen_path(), &text).context("write seen")
}

/// Contact name + whether this sender id has been seen before + verified + trusted.
/// Read-only.
pub fn sender_status(id_b32: &str) -> SenderStatus {
    SenderStatus {
        name: resolve_name(id_b32),
        seen_before: load_seen().seen.contains_key(id_b32),
        verified: is_verified(id_b32),
        trusted: is_trusted(id_b32),
    }
}

/// Record a receipt from `id_b32` (TOFU ledger): increments its counter. Best
/// effort — a failure to persist must not break a completed transfer.
pub fn record_seen(id_b32: &str) {
    let mut s = load_seen();
    *s.seen.entry(id_b32.to_string()).or_insert(0) += 1;
    if let Ok(text) = toml::to_string_pretty(&s) {
        std::fs::create_dir_all(config_dir()).ok();
        let _ = write_private(&seen_path(), &text);
    }
}
