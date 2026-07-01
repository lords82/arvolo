//! Local config + contacts (address book), stored under ~/.config/arvolo.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::crypto::PublicId;
use serde::{Deserialize, Serialize};

fn config_dir() -> PathBuf {
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

/// The default relay: the `ARVOLO_RELAY` env var wins, else the config file's
/// `relay` key. Used so `--relay`/`ARVOLO_RELAY` need not be repeated.
pub fn default_relay() -> Option<String> {
    if let Ok(r) = std::env::var("ARVOLO_RELAY") {
        if !r.trim().is_empty() {
            return Some(r);
        }
    }
    load_config().relay.filter(|s| !s.trim().is_empty())
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
/// (if saved) and whether we've received from them before (TOFU).
pub struct SenderStatus {
    pub name: Option<String>,
    pub seen_before: bool,
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

/// Contact name + whether this sender id has been seen before. Read-only.
pub fn sender_status(id_b32: &str) -> SenderStatus {
    SenderStatus {
        name: resolve_name(id_b32),
        seen_before: load_seen().seen.contains_key(id_b32),
    }
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

/// Add or update a contact (validates the id).
pub fn contact_add(name: &str, id: &str) -> Result<()> {
    decode_id(id).context("invalid public id")?;
    let mut c = load_contacts();
    c.contacts
        .insert(name.to_string(), id.trim().to_lowercase());
    save_contacts(&c)
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
    fn contacts_and_config_roundtrip() {
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

        assert_eq!(contact_list(), vec![("alice".into(), id_b32)]);
        assert!(contact_remove("alice").unwrap());
        assert!(contact_list().is_empty());

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
