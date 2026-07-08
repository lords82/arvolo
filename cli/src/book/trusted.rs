use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Trusted {
    #[serde(default)]
    pub(crate) trusted: BTreeSet<String>,
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
    load_trusted().trusted.contains(id_b32)
}

/// Trust a contact (by name) to auto-download. Errors if the name isn't saved.
pub fn mark_trusted(name: &str) -> Result<String> {
    let id = load_contacts()
        .contacts
        .get(name)
        .cloned()
        .with_context(|| format!("no such contact '{name}'"))?;
    let mut t = load_trusted();
    t.trusted.insert(id.clone());
    save_trusted(&t)?;
    Ok(id)
}

/// Remove a contact's trusted mark (by name).
pub fn unmark_trusted(name: &str) -> Result<()> {
    if let Some(id) = load_contacts().contacts.get(name) {
        let mut t = load_trusted();
        if t.trusted.remove(id) {
            save_trusted(&t)?;
        }
    }
    Ok(())
}

// ---- advertised display names (TOFU on the sender's self-chosen name) ------
//
// A sender may advertise a self-chosen display name inside the sealed offer
// (`Offer::sender_name`). It is a *petname claim*, never an authenticated
// identity: it never enters the verified/trusted trust decision. We pin the name
// the user approved (`pinned`) and quarantine any later change (`pending`) until
// they approve it — a TOFU on the name, mirroring the key-change flow.
