use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Trusted {
    #[serde(default, deserialize_with = "de_marks")]
    pub(crate) trusted: Marks,
}

pub(crate) fn load_trusted() -> Trusted {
    std::fs::read_to_string(trusted_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_trusted(t: &Trusted) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(t).context("serialize trusted")?;
    write_private(&trusted_path(), &text).context("write trusted")
}

/// Does the user trust this identity to auto-download without a prompt? Distinct
/// from [`is_verified`]: verified = key authenticity; trusted = auto-accept.
pub fn is_trusted(id_b32: &str) -> bool {
    load_trusted().trusted.contains_key(id_b32)
}

/// Trust an identity to auto-download. Takes a saved contact name **or** a raw
/// base32 id — see [`mark_verified`] for why both. Returns the id that was marked.
pub fn mark_trusted(who: &str) -> Result<String> {
    let id = resolve_recipient_id(who)?;
    let mut t = load_trusted();
    t.trusted.insert(id.clone(), marked_now());
    save_trusted(&t)?;
    Ok(id)
}

/// Clear an identity's trusted mark. `Ok(false)` means there was nothing to clear.
pub fn unmark_trusted(who: &str) -> Result<bool> {
    let id = resolve_recipient_id(who)?;
    let mut t = load_trusted();
    if t.trusted.remove(&id).is_none() {
        return Ok(false);
    }
    save_trusted(&t)?;
    Ok(true)
}

// ---- advertised display names (TOFU on the sender's self-chosen name) ------
//
// A sender may advertise a self-chosen display name inside the sealed offer
// (`Offer::sender_name`). It is a *petname claim*, never an authenticated
// identity: it never enters the verified/trusted trust decision. We pin the name
// the user approved (`pinned`) and quarantine any later change (`pending`) until
// they approve it — a TOFU on the name, mirroring the key-change flow.
