use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Verified {
    #[serde(default)]
    pub(crate) verified: BTreeSet<String>,
}

pub(crate) fn load_verified() -> Verified {
    std::fs::read_to_string(verified_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_verified(v: &Verified) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(v).context("serialize verified")?;
    write_private(&verified_path(), &text).context("write verified")
}

/// Has the user verified this identity's fingerprint out-of-band?
pub fn is_verified(id_b32: &str) -> bool {
    load_verified().verified.contains(id_b32)
}

/// Mark a contact (by name) verified, after the user compared its fingerprint
/// out-of-band. Errors if the name isn't a saved contact.
pub fn mark_verified(name: &str) -> Result<String> {
    let id = load_contacts()
        .contacts
        .get(name)
        .cloned()
        .with_context(|| format!("no such contact '{name}'"))?;
    let mut v = load_verified();
    v.verified.insert(id.clone());
    save_verified(&v)?;
    Ok(id)
}

/// Remove a contact's verified mark (by name).
pub fn unmark_verified(name: &str) -> Result<()> {
    if let Some(id) = load_contacts().contacts.get(name) {
        let mut v = load_verified();
        if v.verified.remove(id) {
            save_verified(&v)?;
        }
    }
    Ok(())
}
