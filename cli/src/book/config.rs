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
/// missing). An empty/blank value clears it.
pub fn set_my_display_name(name: &str) -> Result<()> {
    let name = name.trim();
    let value = (!name.is_empty()).then(|| toml::Value::String(name.to_string()));
    set_config_value("display_name", value)
}

/// Set one key in `config.toml`, in place.
///
/// Replaces the first line that assigns `key` — commented or not — and appends
/// when there is none, so the file's comments, ordering and every other key
/// survive an edit. `None` comments the line back out, which is how a setting is
/// *cleared*: the key stops being assigned and the built-in default applies again.
/// Values go through `toml::Value`, so they are escaped rather than interpolated.
///
/// This is deliberately a line editor and not a parse-and-reserialize round trip:
/// `config.toml` is a documented file a human is expected to read, and rewriting
/// it through a TOML serializer would silently delete every comment in it — the
/// self-documenting commented defaults included. That is too high a price for a
/// GUI toggling one boolean.
pub fn set_config_value(key: &str, value: Option<toml::Value>) -> Result<()> {
    let path = config_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let new_line = value.as_ref().map(|v| format!("{key} = {v}"));

    let assigns_key = |l: &str| {
        let t = l.trim_start().trim_start_matches('#').trim_start();
        t.strip_prefix(key)
            .is_some_and(|r| r.trim_start().starts_with('='))
    };

    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in existing.lines() {
        // Clearing keeps the old value visible behind the comment marker, so the
        // file still shows what it used to be — restoring it is uncommenting a line.
        if !replaced && assigns_key(line) {
            match &new_line {
                Some(l) => out.push(l.clone()),
                None => {
                    let t = line.trim_start();
                    if t.starts_with('#') {
                        out.push(line.to_string());
                    } else {
                        out.push(format!("#{t}"));
                    }
                }
            }
            replaced = true;
        } else {
            out.push(line.to_string());
        }
    }
    // Nothing to comment out when the key was never there in the first place.
    if !replaced {
        if let Some(l) = new_line {
            out.push(l);
        }
    }
    let mut text = out.join("\n");
    text.push('\n');

    std::fs::create_dir_all(config_dir()).ok();
    std::fs::write(&path, text).context("write config.toml")
}

/// The `config.toml` keys a settings screen both shows *and* offers to change,
/// read in one go.
///
/// Deliberately a subset. Everything else in the file — temp dir, identity path,
/// iroh relay, log level — is either derivable from elsewhere or is a knob whose
/// wrong setting is hard to recover from through a dialog; those stay text-file
/// settings, and the UI links to the file instead of half-exposing them.
pub struct ConfigSnapshot {
    pub relay: Option<String>,
    pub download_dir: Option<String>,
    pub seed: Option<bool>,
    pub swarm: Option<String>,
    pub concurrency: Option<u32>,
}

/// Read `config.toml` into a [`ConfigSnapshot`]. A missing or unparseable file
/// reads as "nothing configured" rather than an error — the same fallback every
/// other reader here uses, so a settings screen opens on a fresh install.
pub fn config_snapshot() -> ConfigSnapshot {
    let c = load_config();
    ConfigSnapshot {
        relay: c.relay,
        download_dir: c.download_dir,
        seed: c.seed,
        swarm: c.swarm,
        concurrency: c.concurrency,
    }
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
