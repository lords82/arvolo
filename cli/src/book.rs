//! Local config + contacts (address book), stored under ~/.config/arvolo.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::crypto::PublicId;
use serde::{Deserialize, Serialize};

/// The user's home directory, cross-platform: `$HOME` on Unix/macOS, else
/// `%USERPROFILE%` (or `%HOMEDRIVE%%HOMEPATH%`) on Windows. Falls back to `.`.
pub fn home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    if let Ok(p) = std::env::var("USERPROFILE") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        if !drive.is_empty() && !path.is_empty() {
            return PathBuf::from(format!("{drive}{path}"));
        }
    }
    PathBuf::from(".")
}

/// The default download directory when none is configured: an `Arvolo` folder in
/// the user's home (`~/Arvolo`), created on every platform.
pub fn default_home_downloads() -> PathBuf {
    home_dir().join("Arvolo")
}

pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("ARVOLO_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    home_dir().join(".config/arvolo")
}

/// Scratch directory for the client's own temporary artifacts — e.g. the tar
/// packed when sending a folder, or an archive staged while receiving. Defaults
/// to `<config>/tmp` (kept off the system temp dir, which may be a small tmpfs,
/// and out of the download directory); override with `ARVOLO_TEMP_DIR`. The
/// directory is created if missing.
pub fn temp_dir() -> PathBuf {
    let dir = std::env::var("ARVOLO_TEMP_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir().join("tmp"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Whether a config file already exists (drives the first-run setup wizard).
pub fn config_exists() -> bool {
    config_path().exists()
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
fn trusted_path() -> PathBuf {
    config_dir().join("trusted.toml")
}

#[derive(Default, Deserialize)]
struct Config {
    relay: Option<String>,
    download_dir: Option<String>,
    temp_dir: Option<String>,
    identity: Option<String>,
    iroh_relay: Option<String>,
    seed: Option<bool>,
    seed_after: Option<u64>,
    swarm: Option<String>,
    concurrency: Option<u32>,
    ipv4_only: Option<bool>,
    max_fetch_bytes: Option<u64>,
    debug: Option<bool>,
    log: Option<String>,
}

fn load_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// Bridge `config.toml` settings into the `ARVOLO_*` environment that
/// `arvolo-core` reads, so a config key actually drives behavior. Precedence is
/// **env > config > default**: a value already present in the environment is left
/// untouched. Also pins `ARVOLO_TEMP_DIR` to a concrete directory. Call once at
/// startup, after any first-run wizard has written the file.
pub fn apply_config_to_env() {
    let cfg = load_config();

    fn set_if_unset(key: &str, val: impl AsRef<std::ffi::OsStr>) {
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, val);
        }
    }

    if let Some(v) = cfg.temp_dir.filter(|s| !s.trim().is_empty()) {
        set_if_unset("ARVOLO_TEMP_DIR", v);
    }
    if let Some(v) = cfg.identity.filter(|s| !s.trim().is_empty()) {
        set_if_unset("ARVOLO_IDENTITY", v);
    }
    if let Some(v) = cfg.iroh_relay.filter(|s| !s.trim().is_empty()) {
        set_if_unset("ARVOLO_IROH_RELAY", v);
    }
    if let Some(b) = cfg.seed {
        set_if_unset("ARVOLO_SEED", if b { "1" } else { "0" });
    }
    if let Some(n) = cfg.seed_after {
        set_if_unset("ARVOLO_SEED_AFTER", n.to_string());
    }
    if let Some(v) = cfg.swarm.filter(|s| !s.trim().is_empty()) {
        set_if_unset("ARVOLO_SWARM", v);
    }
    if let Some(n) = cfg.concurrency {
        set_if_unset("ARVOLO_CONCURRENCY", n.to_string());
    }
    if let Some(b) = cfg.ipv4_only {
        set_if_unset("ARVOLO_IPV4_ONLY", if b { "1" } else { "0" });
    }
    if let Some(n) = cfg.max_fetch_bytes {
        set_if_unset("ARVOLO_MAX_FETCH_BYTES", n.to_string());
    }
    // Core treats *any* value of ARVOLO_DEBUG as on, so only set it when enabled.
    if cfg.debug == Some(true) {
        set_if_unset("ARVOLO_DEBUG", "1");
    }
    if let Some(v) = cfg.log.filter(|s| !s.trim().is_empty()) {
        set_if_unset("RUST_LOG", v);
    }

    // Always pin the scratch dir to a concrete path (config value if bridged
    // above, else `<config>/tmp`) — off the download dir and off a system tmpfs.
    std::env::set_var("ARVOLO_TEMP_DIR", temp_dir());
}

/// Write a fresh `config.toml`: the answered `relay` active (or commented with an
/// example when skipped), and every other client setting listed **commented at
/// its default** so the file self-documents what can be tuned. Overwrites any
/// existing file — callers gate on [`config_exists`].
pub fn write_default_config(relay: Option<&str>) -> Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

    let relay_line = match relay.map(str::trim).filter(|s| !s.is_empty()) {
        Some(r) => format!("relay = \"{}\"", r.replace('"', "")),
        None => "#relay = \"relay.example.com\"".to_string(),
    };
    let downloads = default_home_downloads();
    let tmp = config_dir().join("tmp");
    let identity = config_dir().join("identity.key");

    let body = format!(
        r#"# Arvolo client configuration.
#
# Only `relay` is needed for full functionality; everything else is optional and
# shown below commented at its default. Uncomment a line to change it.
# Environment variables (ARVOLO_*) always override these keys.

# Relay URL — brokers pairing codes, `send --to`, the mailbox, download links and
# the swarm. A bare host assumes https://; for a plaintext/LAN relay write the
# scheme and port, e.g. "http://relay.local:6282".
{relay_line}

# Where received files are saved.
#download_dir = "{downloads}"

# Scratch dir for temporary artifacts (packed tars, staged archives).
#temp_dir = "{tmp}"

# Path to your identity key.
#identity = "{identity}"

# Self-hosted iroh NAT relay for P2P hole-punching (default: n0 public relays).
#iroh_relay = ""

# Keep seeding a completed file into the swarm.
#seed = true

# Seconds to keep backfilling the relay after a transfer completes (0 = off).
#seed_after = 0

# Swarm mode for shared arvc… tickets: "on", "off", or "relay-only" (privacy).
#swarm = "on"

# Parallel chunk fetches (1–16).
#concurrency = 4

# Force IPv4-only transport (default: auto-detected).
#ipv4_only = false

# Max bytes a single download link/blob will fetch (default 512 MiB).
#max_fetch_bytes = 536870912

# Extra diagnostics.
#debug = false

# Log level (tracing / RUST_LOG syntax).
#log = "info"
"#,
        relay_line = relay_line,
        downloads = downloads.display(),
        tmp = tmp.display(),
        identity = identity.display(),
    );

    std::fs::write(config_path(), body).with_context(|| "write config.toml")?;
    Ok(())
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

/// The compiled-in public default relay, so a fresh install works with zero
/// configuration for relay-requiring features (short codes, `--to`, the mailbox).
/// Overridable at build time with `ARVOLO_DEFAULT_RELAY`; build with
/// `ARVOLO_DEFAULT_RELAY=""` to ship no default (relay then always explicit).
pub const BUILTIN_RELAY: &str = match option_env!("ARVOLO_DEFAULT_RELAY") {
    Some(v) => v,
    None => "arvolo.duckdns.org",
};

/// Like [`default_relay`] but falls back to the compiled-in [`BUILTIN_RELAY`] when
/// neither `ARVOLO_RELAY` nor config sets one. Use only on paths that *require* a
/// relay (short codes, `--to`, mailbox, code receive) — never for opportunistic
/// seeding of a P2P ticket, which must stay pure-P2P unless the user configured a
/// relay of their own.
pub fn default_relay_or_builtin() -> Option<String> {
    default_relay().or_else(|| {
        let b = BUILTIN_RELAY.trim();
        (!b.is_empty()).then(|| normalize_relay(b, false))
    })
}

/// The configured download directory for accepted files: the `ARVOLO_DOWNLOAD_DIR`
/// env var wins, else the config file's `download_dir` key. `None` if neither is
/// set — callers fall back to [`default_home_downloads`] (`~/Arvolo`).
pub fn default_download_dir() -> Option<PathBuf> {
    std::env::var("ARVOLO_DOWNLOAD_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| load_config().download_dir.filter(|s| !s.trim().is_empty()))
        .map(PathBuf::from)
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
/// (if saved), whether we've received from them before (TOFU), whether the user
/// has verified their identity out-of-band, and whether the user trusts them to
/// auto-download without a prompt.
pub struct SenderStatus {
    pub name: Option<String>,
    pub seen_before: bool,
    pub verified: bool,
    /// Only consumed by the daemon's auto-download policy, which is `#[cfg(unix)]`;
    /// on non-unix it's computed but never read.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub trusted: bool,
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

#[derive(Default, Serialize, Deserialize)]
struct Trusted {
    #[serde(default)]
    trusted: BTreeSet<String>,
}

fn load_trusted() -> Trusted {
    std::fs::read_to_string(trusted_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_trusted(t: &Trusted) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(t).context("serialize trusted")?;
    std::fs::write(trusted_path(), text).context("write trusted")
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
            // The old key was possibly verified/trusted; the new one is neither
            // until the user re-verifies out-of-band — clear both marks.
            let mut v = load_verified();
            if v.verified.remove(old_id) {
                save_verified(&v)?;
            }
            let mut t = load_trusted();
            if t.trusted.remove(old_id) {
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
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

        // Trust ledger: default untrusted; mark/unmark round-trips.
        assert!(!sender_status(&id_b32).trusted, "untrusted by default");
        mark_trusted("alice").unwrap();
        assert!(is_trusted(&id_b32), "alice is trusted after marking");
        assert!(sender_status(&id_b32).trusted);
        unmark_trusted("alice").unwrap();
        assert!(!is_trusted(&id_b32), "trust cleared after unmark");
        // Re-trust for the key-change check below.
        mark_trusted("alice").unwrap();

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
        assert!(
            !is_trusted(&id_b32),
            "old key's trust is cleared on key change"
        );
        assert!(!is_trusted(&new_b32), "the new key is not auto-trusted");

        assert!(contact_remove("alice").unwrap());
        assert!(contact_list().is_empty());

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
