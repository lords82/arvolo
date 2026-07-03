//! Local config + contacts (address book), stored under ~/.config/arvolo.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::crypto::PublicId;
use serde::{Deserialize, Serialize};

pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("ARVOLO_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/arvolo")
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}
fn contacts_path() -> PathBuf {
    config_dir().join("contacts.toml")
}
fn seen_path() -> PathBuf {
    config_dir().join("seen.toml")
}
fn verified_path() -> PathBuf {
    config_dir().join("verified.toml")
}

#[derive(Default, Deserialize)]
struct Config {
    relay: Option<String>,
}

fn load_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub use arvolo_core::code::normalize_relay;

/// The default relay: the `ARVOLO_RELAY` env var wins, else the config file's
/// `relay` key. Used so `--relay`/`ARVOLO_RELAY` need not be repeated. The value
/// is normalized to a full URL (a bare host gets `https://`); to use plaintext,
/// write an explicit `http://…` in the env var or config file.
pub fn default_relay() -> Option<String> {
    let raw = std::env::var("ARVOLO_RELAY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| load_config().relay.filter(|s| !s.trim().is_empty()))?;
    Some(normalize_relay(&raw, false))
}

#[derive(Default, Serialize, Deserialize)]
struct Contacts {
    #[serde(default)]
    contacts: BTreeMap<String, String>,
}

fn load_contacts() -> Contacts {
    std::fs::read_to_string(contacts_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_contacts(c: &Contacts) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let s = toml::to_string_pretty(c).context("serialize contacts")?;
    std::fs::write(contacts_path(), s).context("write contacts")?;
    Ok(())
}

fn decode_id(s: &str) -> Result<PublicId> {
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

/// What we know about a sender before recording this receipt: their contact name
/// (if saved), whether we've received from them before (TOFU), and whether the
/// user has verified their identity out-of-band.
pub struct SenderStatus {
    pub name: Option<String>,
    pub seen_before: bool,
    pub verified: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Seen {
    #[serde(default)]
    seen: BTreeMap<String, u64>,
}

fn load_seen() -> Seen {
    std::fs::read_to_string(seen_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// Contact name + whether this sender id has been seen before + verified. Read-only.
pub fn sender_status(id_b32: &str) -> SenderStatus {
    SenderStatus {
        name: resolve_name(id_b32),
        seen_before: load_seen().seen.contains_key(id_b32),
        verified: is_verified(id_b32),
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Verified {
    #[serde(default)]
    verified: BTreeSet<String>,
}

fn load_verified() -> Verified {
    std::fs::read_to_string(verified_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_verified(v: &Verified) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(v).context("serialize verified")?;
    std::fs::write(verified_path(), text).context("write verified")
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

/// Record a receipt from `id_b32` (TOFU ledger): increments its counter. Best
/// effort — a failure to persist must not break a completed transfer.
pub fn record_seen(id_b32: &str) {
    let mut s = load_seen();
    *s.seen.entry(id_b32.to_string()).or_insert(0) += 1;
    if let Ok(text) = toml::to_string_pretty(&s) {
        std::fs::create_dir_all(config_dir()).ok();
        let _ = std::fs::write(seen_path(), text);
    }
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
            // The old key was possibly verified; the new one is not — clear it.
            let mut v = load_verified();
            if v.verified.remove(old_id) {
                save_verified(&v)?;
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
pub fn contact_remove(name: &str) -> Result<bool> {
    let mut c = load_contacts();
    let existed = c.contacts.remove(name).is_some();
    save_contacts(&c)?;
    Ok(existed)
}

/// All contacts, sorted by name.
pub fn contact_list() -> Vec<(String, String)> {
    load_contacts().contacts.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arvolo_core::crypto::Identity;

    #[test]
    fn relay_scheme_defaults_to_https() {
        // Bare host → https, unless --use-http is asked for.
        assert_eq!(
            normalize_relay("relay.example.com", false),
            "https://relay.example.com"
        );
        assert_eq!(
            normalize_relay("relay.example.com", true),
            "http://relay.example.com"
        );
        assert_eq!(
            normalize_relay("  relay:8787 ", false),
            "https://relay:8787"
        );
        // An explicit scheme always wins over the flag.
        assert_eq!(
            normalize_relay("http://relay.local", false),
            "http://relay.local"
        );
        assert_eq!(
            normalize_relay("https://relay.example.com", true),
            "https://relay.example.com"
        );
    }

    #[test]
    fn contacts_and_config_roundtrip() {
        let _guard = crate::testlock::ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        std::env::remove_var("ARVOLO_RELAY");

        // Config: default_relay reads the config.toml `relay`.
        std::fs::write(
            dir.path().join("config.toml"),
            "relay = \"https://relay.example.com\"\n",
        )
        .unwrap();
        assert_eq!(
            default_relay().as_deref(),
            Some("https://relay.example.com")
        );

        // Contacts: add, resolve by name, list, remove.
        let id = Identity::generate().public();
        let id_b32 = data_encoding::BASE32_NOPAD
            .encode(&id.to_bytes())
            .to_lowercase();
        contact_add("alice", &id_b32).unwrap();
        assert_eq!(
            resolve_recipient("alice").unwrap().to_bytes(),
            id.to_bytes()
        );
        // A raw id resolves too (not a contact name).
        assert_eq!(
            resolve_recipient(&id_b32).unwrap().to_bytes(),
            id.to_bytes()
        );

        // Reverse-lookup: id -> saved contact name.
        assert_eq!(resolve_name(&id_b32).as_deref(), Some("alice"));
        assert_eq!(resolve_name("nonexistentid"), None);

        // TOFU ledger: unseen at first, then seen after recording a receipt.
        let st = sender_status(&id_b32);
        assert_eq!(st.name.as_deref(), Some("alice"));
        assert!(!st.seen_before, "sender not seen before the first receipt");
        record_seen(&id_b32);
        assert!(
            sender_status(&id_b32).seen_before,
            "sender is seen after recording a receipt"
        );

        assert_eq!(contact_list(), vec![("alice".into(), id_b32.clone())]);

        // Verify + key-change detection: re-adding the same name under a *new* id
        // reports a key change and drops the verified mark.
        mark_verified("alice").unwrap();
        assert!(is_verified(&id_b32), "alice is verified");
        // Same id again → no key change, verified preserved.
        assert!(contact_add("alice", &id_b32).unwrap().is_none());
        assert!(is_verified(&id_b32), "re-adding the same id keeps verified");
        // A different id → key change reported, verified cleared.
        let new_id = Identity::generate().public();
        let new_b32 = data_encoding::BASE32_NOPAD
            .encode(&new_id.to_bytes())
            .to_lowercase();
        let change = contact_add("alice", &new_b32).unwrap();
        assert!(change.is_some(), "key change is reported");
        let change = change.unwrap();
        assert_ne!(change.old_fingerprint, change.new_fingerprint);
        assert!(!is_verified(&id_b32), "old key's verified mark is cleared");
        assert!(!is_verified(&new_b32), "the new key is not auto-verified");

        assert!(contact_remove("alice").unwrap());
        assert!(contact_list().is_empty());

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
