use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::*;

#[derive(Default, Deserialize)]
pub(crate) struct Config {
    pub(crate) relay: Option<String>,
    pub(crate) download_dir: Option<String>,
    pub(crate) temp_dir: Option<String>,
    pub(crate) identity: Option<String>,
    pub(crate) iroh_relay: Option<String>,
    pub(crate) seed: Option<bool>,
    pub(crate) seed_after: Option<u64>,
    pub(crate) swarm: Option<String>,
    pub(crate) concurrency: Option<u32>,
    ipv4_only: Option<bool>,
    pub(crate) max_fetch_bytes: Option<u64>,
    pub(crate) debug: Option<bool>,
    pub(crate) log: Option<String>,
    pub(crate) sync: Option<bool>,
    pub(crate) display_name: Option<String>,
}

/// The local user's self-chosen display name, advertised inside every outgoing
/// offer (`arvolo name "…"`). Empty string when unset — meaning "don't advertise a
/// name", the pre-existing behavior. It is a petname claim, never an authenticated
/// identity.
pub fn my_display_name() -> String {
    load_config()
        .display_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

/// Persist the local user's display name into `config.toml` (creating the file if
/// missing). An empty/blank value clears it. Edits the single `display_name` line
/// in place — replacing an existing (possibly commented) one, else appending —
/// so the file's comments and other keys are preserved. Values are TOML-escaped.
pub fn set_my_display_name(name: &str) -> Result<()> {
    let path = config_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let name = name.trim();
    // The line the key should become: an active assignment, or the commented
    // default when clearing (so `arvolo name ""` restores the documented stub).
    let new_line = if name.is_empty() {
        "#display_name = \"\"".to_string()
    } else {
        format!("display_name = {}", toml::Value::String(name.to_string()))
    };

    // Replace the first line that assigns `display_name` (commented or not).
    let is_display_line = |l: &str| {
        let t = l.trim_start().trim_start_matches('#').trim_start();
        t.strip_prefix("display_name")
            .is_some_and(|r| r.trim_start().starts_with('='))
    };
    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in existing.lines() {
        if !replaced && is_display_line(line) {
            out.push(new_line.clone());
            replaced = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !replaced {
        out.push(new_line);
    }
    let mut text = out.join("\n");
    text.push('\n');

    std::fs::create_dir_all(config_dir()).ok();
    std::fs::write(&path, text).context("write config.toml")
}

/// Whether automatic multi-device address-book sync runs in `listen`/`daemon`.
/// Defaults to on; set `sync = false` in config to disable (or pass `--no-sync`).
pub fn sync_enabled() -> bool {
    load_config().sync.unwrap_or(true)
}

pub(crate) fn load_config() -> Config {
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

# Your display name, advertised to recipients inside each sealed offer (a petname
# claim, never a verified identity). Set with `arvolo name "…"`; empty = none.
#display_name = ""
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
