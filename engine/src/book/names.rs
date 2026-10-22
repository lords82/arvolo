use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

/// One advertised-name record, keyed by base32 id: the approved name, the last
/// advertised name still awaiting approval (if any), and the very first name this
/// key was ever seen to claim.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct NameRow {
    #[serde(default)]
    pub pinned: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<String>,
    /// Written once, on the first claim ever observed from this key, and never
    /// touched again.
    ///
    /// Without it a stranger's name history is unreadable: `pending` is
    /// overwritten by every new claim, so a key that introduced itself as one
    /// person and comes back as another looks exactly like a key that has only
    /// ever used the second name. That difference is the whole signal, and until
    /// somebody approves a name (`pinned`) there is nowhere else it survives.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub first: String,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Names {
    #[serde(default)]
    pub names: BTreeMap<String, NameRow>,
}

pub fn load_names() -> Names {
    std::fs::read_to_string(names_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_names(n: &Names) -> Result<()> {
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

    // The first claim, kept for good. Deliberately not a pin — nobody approved
    // anything — just the record of how this key introduced itself.
    let first_claim = if row.first.is_empty() {
        row.first = advertised.to_string();
        true
    } else {
        false
    };

    if row.pinned.is_empty() {
        // Never approved a name for this id yet → brand-new.
        let moved = row.pending.as_deref() != Some(advertised);
        if moved {
            row.pending = Some(advertised.to_string());
        }
        if moved || first_claim {
            let _ = save_names(&names);
        }
        NameStatus::New(advertised.to_string())
    } else if row.pinned == advertised {
        // Back to the approved name → drop any stale pending.
        let moved = row.pending.is_some();
        if moved {
            row.pending = None;
        }
        if moved || first_claim {
            let _ = save_names(&names);
        }
        NameStatus::Unchanged(advertised.to_string())
    } else {
        let old = row.pinned.clone();
        let moved = row.pending.as_deref() != Some(advertised);
        if moved {
            row.pending = Some(advertised.to_string());
        }
        if moved || first_claim {
            let _ = save_names(&names);
        }
        NameStatus::Changed {
            old,
            new: advertised.to_string(),
        }
    }
}

/// What this key was already on record as calling itself: the approved name if
/// there is one, otherwise the first claim ever seen from it.
///
/// This is the half of the name history that means something *before* anyone is
/// saved. Compared against the claim on the offer in hand, it answers the only
/// question a display name can be made to answer: is this the same story as last
/// time? Empty when the key has never advertised anything.
pub fn known_name_of(id_b32: &str) -> Option<String> {
    load_names()
        .names
        .get(id_b32)
        .map(|r| {
            if r.pinned.is_empty() {
                r.first.clone()
            } else {
                r.pinned.clone()
            }
        })
        .filter(|s| !s.is_empty())
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
pub fn resolve_recipient_id(who: &str) -> Result<String> {
    if let Some(id) = load_contacts().contacts.get(who.trim()) {
        return Ok(id.clone());
    }
    // Validate + normalize a raw id, tolerating the shareable `<id>@<relay>` form
    // so anything you can paste into `contacts add` also works here.
    let (id, _) = split_id_and_relay(who);
    decode_id(id).context("not a known contact name or a valid public id")?;
    Ok(id.trim().to_lowercase())
}
