use std::collections::BTreeMap;

use anyhow::{Context, Result};
use arvolo_core::crypto::PublicId;
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Contacts {
    #[serde(default)]
    pub(crate) contacts: BTreeMap<String, String>,
}

pub(crate) fn load_contacts() -> Contacts {
    std::fs::read_to_string(contacts_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_contacts(c: &Contacts) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let s = toml::to_string_pretty(c).context("serialize contacts")?;
    write_private(&contacts_path(), &s).context("write contacts")?;
    Ok(())
}

pub(crate) fn decode_id(s: &str) -> Result<PublicId> {
    let bytes = data_encoding::BASE32_NOPAD
        .decode(s.trim().to_uppercase().as_bytes())
        .context("invalid public id (base32)")?;
    PublicId::from_bytes(&bytes)
}

/// Resolve a `--to` argument to a recipient: a saved contact name, else a raw
/// base32 public id.
pub fn resolve_recipient(arg: &str) -> Result<PublicId> {
    if let Some(id) = load_contacts().contacts.get(arg) {
        return decode_id(id).with_context(|| format!("contact '{arg}' has an invalid id"));
    }
    decode_id(arg).context("not a known contact name or a valid public id")
}

/// The word fingerprint for a stored base32 id (for display in listings).
pub fn fingerprint_of(id_b32: &str) -> Option<String> {
    decode_id(id_b32).ok().map(|p| p.fingerprint())
}

/// Reverse-lookup: the saved contact name for a base32 public id, if any.
pub fn resolve_name(id_b32: &str) -> Option<String> {
    load_contacts()
        .contacts
        .into_iter()
        .find(|(_, id)| id == id_b32)
        .map(|(name, _)| name)
}

/// A saved contact's key (id) changed under an existing name — a possible
/// after-the-fact MITM (or an innocent reinstall). Surfaced so the user
/// re-verifies out-of-band before trusting the new key.
pub struct KeyChange {
    pub old_fingerprint: String,
    pub new_fingerprint: String,
}

/// Add or update a contact (validates the id). If the name already exists with a
/// *different* id, reports the key change and drops the contact's verified mark
/// (the new key is untrusted until re-verified).
pub fn contact_add(name: &str, id: &str) -> Result<Option<KeyChange>> {
    decode_id(id).context("invalid public id")?;
    let new_id = id.trim().to_lowercase();
    let mut c = load_contacts();

    let key_change = match c.contacts.get(name) {
        Some(old_id) if *old_id != new_id => {
            let change = KeyChange {
                old_fingerprint: fingerprint_of(old_id).unwrap_or_default(),
                new_fingerprint: fingerprint_of(&new_id).unwrap_or_default(),
            };
            // The old key was possibly verified/trusted; the new one is neither
            // until the user re-verifies out-of-band — clear both marks.
            let mut v = load_verified();
            if v.verified.remove(old_id).is_some() {
                save_verified(&v)?;
            }
            let mut t = load_trusted();
            if t.trusted.remove(old_id).is_some() {
                save_trusted(&t)?;
            }
            Some(change)
        }
        _ => None,
    };

    c.contacts.insert(name.to_string(), new_id);
    save_contacts(&c)?;
    Ok(key_change)
}

/// Remove a contact; returns whether it existed.
///
/// Also drops the identity's row in `names.toml`. A pinned advertised name for a
/// contact you deleted is state with no owner: it would survive forever and keep
/// syncing to your other devices. The `seen` counter is **kept** on purpose —
/// that is the TOFU ledger, and forgetting it would make an already-known sender
/// look brand new next time, which weakens a defence rather than tidying up.
pub fn contact_remove(name: &str) -> Result<bool> {
    let mut c = load_contacts();
    let Some(id) = c.contacts.remove(name) else {
        return Ok(false);
    };
    save_contacts(&c)?;
    // Only if no other alias still points at that identity.
    if !c.contacts.values().any(|other| *other == id) {
        forget_name_row(&id)?;
    }
    Ok(true)
}

/// Give a contact a different local name, keeping everything else about them.
///
/// The alternative people reach for — `remove` then `add` — silently loses the
/// verified and trusted marks, because `remove` clears them. Nothing needs to be
/// re-verified to rename someone: those ledgers are keyed by **id**, so moving the
/// name→id binding leaves them untouched by construction.
pub fn contact_rename(old: &str, new: &str) -> Result<()> {
    let mut c = load_contacts();
    anyhow::ensure!(
        !c.contacts.contains_key(new),
        "'{new}' is already a contact — pick another name, or remove that one first"
    );
    let id = c
        .contacts
        .remove(old)
        .with_context(|| format!("no such contact '{old}'"))?;
    c.contacts.insert(new.to_string(), id);
    save_contacts(&c)?;
    Ok(())
}

/// Advertised-name rows for identities no contact points at any more.
///
/// These accumulate from contacts removed before [`contact_remove`] learned to
/// clean up after itself, and they keep syncing between devices. Returns how many
/// were dropped.
pub fn prune_orphan_names() -> Result<usize> {
    let known: std::collections::BTreeSet<String> =
        load_contacts().contacts.into_values().collect();
    let mut names = load_names();
    let before = names.names.len();
    names.names.retain(|id, _| known.contains(id));
    let removed = before - names.names.len();
    if removed > 0 {
        save_names(&names)?;
    }
    Ok(removed)
}

fn forget_name_row(id_b32: &str) -> Result<()> {
    let mut names = load_names();
    if names.names.remove(id_b32).is_some() {
        save_names(&names)?;
    }
    Ok(())
}

/// All contacts, sorted by name.
pub fn contact_list() -> Vec<(String, String)> {
    load_contacts().contacts.into_iter().collect()
}

// ---- CRDT sync sidecar ----------------------------------------------------
//
// The four TOML ledgers stay the authoritative projection every existing read
// path uses. Alongside them we keep a postcard sidecar under `<config>/sync/`
// holding the CRDT state (Lamport clocks + tombstones) so edits can be merged
// across a user's devices. `build_local_snapshot` reconciles the sidecar with the
// current TOMLs (capturing edits made out-of-band, e.g. by `contact_add`) and
// returns a full snapshot to publish; `apply_merged_state` folds an incoming
// snapshot in and re-projects the merged state back into the TOMLs.
