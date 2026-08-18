use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

/// One advertised-name record, keyed by base32 id: the approved name and the last
/// advertised name still awaiting approval (if any).
#[derive(Default, Clone, Serialize, Deserialize)]
pub(crate) struct NameRow {
    #[serde(default)]
    pub(crate) pinned: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Names {
    #[serde(default)]
    pub(crate) names: BTreeMap<String, NameRow>,
}

pub(crate) fn load_names() -> Names {
    std::fs::read_to_string(names_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_names(n: &Names) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(n).context("serialize names")?;
    write_private(&names_path(), &text).context("write names")
}

/// The outcome of observing a sender's advertised name on receipt.
pub enum NameStatus {
    /// The sender advertised no name (empty) — nothing to show.
    None,
    /// First advertised name ever seen for this id — awaiting the user's approval.
    New(String),
    /// Advertised name matches the approved (pinned) name — nothing to do.
    Unchanged(String),
    /// Advertised name differs from the pinned one — quarantined until approved.
    Changed { old: String, new: String },
}

/// Observe a sender's advertised name on receipt and record any change as
/// `pending`. Never pins on its own — approval is always explicit
/// (`accept_name`), matching the user's "then I accept the name" requirement.
/// Best-effort persistence: a write failure degrades to a no-op, never breaks the
/// transfer.
pub fn observe_advertised_name(id_b32: &str, advertised: &str) -> NameStatus {
    let advertised = advertised.trim();
    if advertised.is_empty() {
        return NameStatus::None;
    }
    let mut names = load_names();
    let row = names.names.entry(id_b32.to_string()).or_default();

    if row.pinned.is_empty() {
        // Never approved a name for this id yet → brand-new.
        if row.pending.as_deref() != Some(advertised) {
            row.pending = Some(advertised.to_string());
            let _ = save_names(&names);
        }
        NameStatus::New(advertised.to_string())
    } else if row.pinned == advertised {
        // Back to the approved name → drop any stale pending.
        if row.pending.is_some() {
            row.pending = None;
            let _ = save_names(&names);
        }
        NameStatus::Unchanged(advertised.to_string())
    } else {
        let old = row.pinned.clone();
        if row.pending.as_deref() != Some(advertised) {
            row.pending = Some(advertised.to_string());
            let _ = save_names(&names);
        }
        NameStatus::Changed {
            old,
            new: advertised.to_string(),
        }
    }
}

/// Approve the pending advertised name for a contact alias or raw id: pin it and
/// clear the pending. Returns the newly pinned name. Errors if there's nothing to
/// approve.
pub fn accept_name(who: &str) -> Result<String> {
    let id_b32 = resolve_recipient_id(who)?;
    let mut names = load_names();
    let row = names
        .names
        .get_mut(&id_b32)
        .filter(|r| r.pending.is_some())
        .with_context(|| format!("no pending name to approve for '{who}'"))?;
    let approved = row.pending.take().unwrap();
    row.pinned = approved.clone();
    save_names(&names)?;
    Ok(approved)
}

/// The approved (pinned) advertised name for an id, if any.
pub fn display_name_of(id_b32: &str) -> Option<String> {
    load_names()
        .names
        .get(id_b32)
        .map(|r| r.pinned.clone())
        .filter(|s| !s.is_empty())
}

/// The pending (awaiting-approval) advertised name for an id, if any.
pub fn pending_name_of(id_b32: &str) -> Option<String> {
    load_names()
        .names
        .get(id_b32)
        .and_then(|r| r.pending.clone())
}

/// Resolve a contact alias or raw base32 id to the canonical base32 id string
/// (lowercased), so name records key consistently with the other id-keyed ledgers.
pub(crate) fn resolve_recipient_id(who: &str) -> Result<String> {
    if let Some(id) = load_contacts().contacts.get(who) {
        return Ok(id.clone());
    }
    // Validate + normalize a raw id.
    decode_id(who).context("not a known contact name or a valid public id")?;
    Ok(who.trim().to_lowercase())
}
