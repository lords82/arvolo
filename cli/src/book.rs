//! Local config + contacts (address book), stored under ~/.config/arvolo.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use arvolo_core::crypto::PublicId;
use arvolo_core::sync::{self, ContactReg, DeviceId, Lamport, MarkReg, SyncSnapshot, SyncState};
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
/// Write a file with owner-only permissions (`0o600`) on unix — same posture as the
/// identity key. These ledgers hold privacy-sensitive data (who you talk to) and
/// the trust marks that gate the daemon's auto-download, so another local user
/// must not be able to read or tamper with them. On non-unix, permissions are not
/// applied (accepted limitation; the file still lands in the user's profile dir).
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    write_private_bytes(path, contents.as_bytes())
}

/// Binary counterpart of [`write_private`] for the postcard-encoded sync sidecar.
fn write_private_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
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
    sync: Option<bool>,
}

/// Whether automatic multi-device address-book sync runs in `listen`/`daemon`.
/// Defaults to on; set `sync = false` in config to disable (or pass `--no-sync`).
pub fn sync_enabled() -> bool {
    load_config().sync.unwrap_or(true)
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
    // String keys ignore a blank value; bools map to the core's "1"/"0".
    fn nonempty(s: String) -> Option<String> {
        Some(s).filter(|s| !s.trim().is_empty())
    }
    fn bool_env(b: bool) -> String {
        if b { "1" } else { "0" }.to_string()
    }

    // One row per config key → its ARVOLO_* env var, each normalized to the string
    // the env expects (or `None` to leave the env untouched). `debug` maps to "1"
    // only when on, since core treats *any* value of ARVOLO_DEBUG as enabled.
    // Bridged with env > config > default precedence via `set_if_unset`.
    let bridges: [(&str, Option<String>); 11] = [
        ("ARVOLO_TEMP_DIR", cfg.temp_dir.and_then(nonempty)),
        ("ARVOLO_IDENTITY", cfg.identity.and_then(nonempty)),
        ("ARVOLO_IROH_RELAY", cfg.iroh_relay.and_then(nonempty)),
        ("ARVOLO_SEED", cfg.seed.map(bool_env)),
        ("ARVOLO_SEED_AFTER", cfg.seed_after.map(|n| n.to_string())),
        ("ARVOLO_SWARM", cfg.swarm.and_then(nonempty)),
        ("ARVOLO_CONCURRENCY", cfg.concurrency.map(|n| n.to_string())),
        ("ARVOLO_IPV4_ONLY", cfg.ipv4_only.map(bool_env)),
        (
            "ARVOLO_MAX_FETCH_BYTES",
            cfg.max_fetch_bytes.map(|n| n.to_string()),
        ),
        (
            "ARVOLO_DEBUG",
            cfg.debug.filter(|&b| b).map(|_| "1".to_string()),
        ),
        ("RUST_LOG", cfg.log.and_then(nonempty)),
    ];
    for (key, val) in bridges {
        if let Some(v) = val {
            set_if_unset(key, v);
        }
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

# Automatically sync your address book across your linked devices (see `device`).
#sync = true
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
    write_private(&contacts_path(), &s).context("write contacts")?;
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

fn save_seen(s: &Seen) -> Result<()> {
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

// ---- CRDT sync sidecar ----------------------------------------------------
//
// The four TOML ledgers stay the authoritative projection every existing read
// path uses. Alongside them we keep a postcard sidecar under `<config>/sync/`
// holding the CRDT state (Lamport clocks + tombstones) so edits can be merged
// across a user's devices. `build_local_snapshot` reconciles the sidecar with the
// current TOMLs (capturing edits made out-of-band, e.g. by `contact_add`) and
// returns a full snapshot to publish; `apply_merged_state` folds an incoming
// snapshot in and re-projects the merged state back into the TOMLs.

fn sync_dir() -> PathBuf {
    config_dir().join("sync")
}
fn meta_path() -> PathBuf {
    sync_dir().join("meta.bin")
}
fn device_path() -> PathBuf {
    sync_dir().join("device.bin")
}

/// This device's local id (a random tiebreak for the Lamport clock, **not** the
/// shared identity). Minted and persisted on first use.
fn load_or_init_device() -> DeviceId {
    if let Ok(bytes) = std::fs::read(device_path()) {
        if bytes.len() == 16 {
            let mut id = [0u8; 16];
            id.copy_from_slice(&bytes);
            return id;
        }
    }
    let id = sync::random_device_id();
    std::fs::create_dir_all(sync_dir()).ok();
    let _ = write_private_bytes(&device_path(), &id);
    id
}

/// Load the CRDT sidecar: `(state, lamport, device)`. Missing/corrupt → empty.
fn load_meta() -> (SyncState, u64, DeviceId) {
    let device = load_or_init_device();
    match std::fs::read(meta_path())
        .ok()
        .and_then(|b| sync::decode_snapshot(&b).ok())
    {
        Some(snap) => (SyncState::from_snapshot(&snap), snap.lamport, device),
        None => (SyncState::default(), 0, device),
    }
}

fn save_meta(state: &SyncState, lamport: u64, device: DeviceId) -> Result<()> {
    std::fs::create_dir_all(sync_dir()).ok();
    let snap = state.to_snapshot(lamport, device);
    let bytes = sync::encode_snapshot(&snap).context("serialize sync meta")?;
    write_private_bytes(&meta_path(), &bytes).context("write sync meta")
}

fn tombstone_marks(
    state: &mut BTreeMap<String, MarkReg>,
    present: &BTreeSet<String>,
    lamport: &mut u64,
    device: DeviceId,
) {
    // A pubkey present in the ledger but not yet active in the sidecar → add.
    for pk in present {
        let need = !matches!(state.get(pk), Some(r) if !r.deleted);
        if need {
            *lamport += 1;
            state.insert(
                pk.clone(),
                MarkReg {
                    clock: Lamport {
                        counter: *lamport,
                        device,
                    },
                    deleted: false,
                },
            );
        }
    }
    // Active in the sidecar but no longer in the ledger → tombstone. This is what
    // carries `contact_add`'s key-change clearing (which already removed the old
    // pubkey from the verified/trusted TOMLs) into the CRDT and out to peers.
    let stale: Vec<String> = state
        .iter()
        .filter(|(pk, r)| !r.deleted && !present.contains(*pk))
        .map(|(pk, _)| pk.clone())
        .collect();
    for pk in stale {
        *lamport += 1;
        state.insert(
            pk.clone(),
            MarkReg {
                clock: Lamport {
                    counter: *lamport,
                    device,
                },
                deleted: true,
            },
        );
    }
}

/// Reconcile the sidecar with the current TOML ledgers and return a full snapshot
/// to publish to the user's other devices. Any TOML edit made without touching
/// the sidecar (e.g. via `contact_add`/`contact_remove`) is captured here as a
/// fresh-clock add or tombstone, so the published snapshot always reflects the
/// authoritative ledgers.
pub fn build_local_snapshot() -> Result<SyncSnapshot> {
    let (mut state, mut lamport, device) = load_meta();

    let contacts = load_contacts().contacts;
    // Adds / key-changes.
    for (name, pubkey) in &contacts {
        let changed =
            !matches!(state.contacts.get(name), Some(r) if !r.deleted && &r.pubkey == pubkey);
        if changed {
            lamport += 1;
            state.contacts.insert(
                name.clone(),
                ContactReg {
                    pubkey: pubkey.clone(),
                    clock: Lamport {
                        counter: lamport,
                        device,
                    },
                    deleted: false,
                },
            );
        }
    }
    // Contacts removed out-of-band → tombstone.
    let removed: Vec<(String, String)> = state
        .contacts
        .iter()
        .filter(|(name, r)| !r.deleted && !contacts.contains_key(*name))
        .map(|(name, r)| (name.clone(), r.pubkey.clone()))
        .collect();
    for (name, pubkey) in removed {
        lamport += 1;
        state.contacts.insert(
            name,
            ContactReg {
                pubkey,
                clock: Lamport {
                    counter: lamport,
                    device,
                },
                deleted: true,
            },
        );
    }

    // Marks: set-membership reconciliation (carries key-change clearing too).
    tombstone_marks(
        &mut state.verified,
        &load_verified().verified,
        &mut lamport,
        device,
    );
    tombstone_marks(
        &mut state.trusted,
        &load_trusted().trusted,
        &mut lamport,
        device,
    );

    // Seen counters: monotone max into the sidecar.
    for (pk, cnt) in load_seen().seen {
        let e = state.seen.entry(pk).or_insert(0);
        *e = (*e).max(cnt);
    }

    save_meta(&state, lamport, device)?;
    Ok(state.to_snapshot(lamport, device))
}

/// Merge an incoming snapshot into the sidecar and re-project the merged state
/// into the four TOML ledgers. Idempotent and order-independent (CRDT).
pub fn apply_merged_state(incoming: &SyncSnapshot) -> Result<()> {
    // Fold local out-of-band edits in first so a merge never silently reverts an
    // un-published local change.
    build_local_snapshot()?;

    let (mut state, mut lamport, device) = load_meta();
    let incoming_state = SyncState::from_snapshot(incoming);
    state.merge(&incoming_state);
    lamport = lamport.max(incoming.lamport).max(state.max_counter());
    save_meta(&state, lamport, device)?;

    project_to_ledgers(&state)?;
    Ok(())
}

/// Project the merged CRDT state into the four authoritative TOML ledgers: a
/// non-deleted register becomes a live row, a tombstone becomes absence.
fn project_to_ledgers(state: &SyncState) -> Result<()> {
    let mut c = Contacts::default();
    for (name, r) in &state.contacts {
        if !r.deleted {
            c.contacts.insert(name.clone(), r.pubkey.clone());
        }
    }
    save_contacts(&c)?;

    let mut v = Verified::default();
    for (pk, r) in &state.verified {
        if !r.deleted {
            v.verified.insert(pk.clone());
        }
    }
    save_verified(&v)?;

    let mut t = Trusted::default();
    for (pk, r) in &state.trusted {
        if !r.deleted {
            t.trusted.insert(pk.clone());
        }
    }
    save_trusted(&t)?;

    let mut s = Seen::default();
    for (pk, cnt) in &state.seen {
        s.seen.insert(pk.clone(), *cnt);
    }
    save_seen(&s)?;
    Ok(())
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

    fn b32(id: &PublicId) -> String {
        data_encoding::BASE32_NOPAD
            .encode(&id.to_bytes())
            .to_lowercase()
    }

    #[test]
    fn sync_merge_propagates_add_and_key_change() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let id1 = b32(&Identity::generate().public());
        let id2 = b32(&Identity::generate().public());

        // Device A: alice=id1, verified + trusted; publish snapshot.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        contact_add("alice", &id1).unwrap();
        mark_verified("alice").unwrap();
        mark_trusted("alice").unwrap();
        let snap1 = build_local_snapshot().unwrap();

        // Device B: apply → alice=id1 with both marks.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&snap1).unwrap();
        assert_eq!(
            resolve_recipient("alice").unwrap().to_bytes(),
            decode_id(&id1).unwrap().to_bytes()
        );
        assert!(is_verified(&id1) && is_trusted(&id1));

        // Device A: alice's key changes to id2 (clears id1's marks locally).
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        assert!(contact_add("alice", &id2).unwrap().is_some());
        assert!(!is_verified(&id1));
        let snap2 = build_local_snapshot().unwrap();

        // Device B: apply → alice=id2, id1's marks cleared, id2 not auto-verified.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&snap2).unwrap();
        assert_eq!(
            resolve_recipient("alice").unwrap().to_bytes(),
            decode_id(&id2).unwrap().to_bytes()
        );
        assert!(!is_verified(&id1), "old key's verified mark cleared on B");
        assert!(!is_trusted(&id1), "old key's trust cleared on B");
        assert!(!is_verified(&id2), "new key is not auto-verified");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn sync_merge_propagates_removal() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let id = b32(&Identity::generate().public());

        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        contact_add("bob", &id).unwrap();
        let s1 = build_local_snapshot().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&s1).unwrap();
        assert_eq!(contact_list().len(), 1);

        // A removes bob → tombstone propagates → B drops it.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        assert!(contact_remove("bob").unwrap());
        let s2 = build_local_snapshot().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&s2).unwrap();
        assert!(contact_list().is_empty(), "removal propagated to B");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
