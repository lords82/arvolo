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
    pub(crate) share_days: Option<u64>,
    pub(crate) share_copies: Option<u64>,
    pub(crate) swarm: Option<String>,
    pub(crate) concurrency: Option<u32>,
    ipv4_only: Option<bool>,
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
    // A `[table]` header changes what an appended line would *mean*: everything
    // after it belongs to that table, so `download_dir = "…"` appended at the end
    // of a file that has one becomes `table.download_dir` — written successfully,
    // read back as unset, and silently never applied.
    let opens_table = |l: &str| {
        let t = l.trim_start();
        !t.starts_with('#') && t.starts_with('[')
    };

    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    let mut saw_table = false;
    for line in existing.lines() {
        if opens_table(line) {
            saw_table = true;
        }
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
        match new_line {
            Some(_) if saw_table => anyhow::bail!(
                "config.toml has a [table] section, so `{key}` cannot be appended safely — \
                 add it by hand above the first section header"
            ),
            Some(l) => out.push(l),
            None => {}
        }
    }
    let mut text = out.join("\n");
    text.push('\n');

    // Never leave the file unparsable. It is edited line by line rather than
    // reserialized (so its comments survive), which means an edit can collide
    // with something a human wrote — a duplicate key being the easy one: TOML
    // rejects the file outright and `load_config` swallows that into
    // `Config::default()`, so *every* setting silently reverts to a built-in and
    // the settings screen cheerfully agrees nothing was ever configured.
    if let Err(e) = text.parse::<toml::Table>() {
        anyhow::bail!(
            "that edit would make config.toml unparsable ({e}) — it was left untouched. \
             Check for a duplicate `{key}` further down the file."
        );
    }

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
// Used only by the daemon-side code, which is `#[cfg(unix)]` — so on Windows it
// really is unreferenced, and `-D warnings` turns that into a build failure. It
// is not dead: it is alive on the platforms where the daemon exists.
#[cfg_attr(not(unix), allow(dead_code))]
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
// Used only by the daemon-side code, which is `#[cfg(unix)]` — so on Windows it
// really is unreferenced, and `-D warnings` turns that into a build failure. It
// is not dead: it is alive on the platforms where the daemon exists.
#[cfg_attr(not(unix), allow(dead_code))]
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
    let bridges: [(&str, Option<String>); 12] = [
        ("ARVOLO_TEMP_DIR", cfg.temp_dir.and_then(nonempty)),
        ("ARVOLO_IDENTITY", cfg.identity.and_then(nonempty)),
        ("ARVOLO_IROH_RELAY", cfg.iroh_relay.and_then(nonempty)),
        ("ARVOLO_SEED", cfg.seed.map(bool_env)),
        ("ARVOLO_SEED_AFTER", cfg.seed_after.map(|n| n.to_string())),
        ("ARVOLO_SHARE_DAYS", cfg.share_days.map(|n| n.to_string())),
        (
            "ARVOLO_SHARE_COPIES",
            cfg.share_copies.map(|n| n.to_string()),
        ),
        ("ARVOLO_SWARM", cfg.swarm.and_then(nonempty)),
        ("ARVOLO_CONCURRENCY", cfg.concurrency.map(|n| n.to_string())),
        ("ARVOLO_IPV4_ONLY", cfg.ipv4_only.map(bool_env)),
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

# When to stop sharing a file you are making available (a ticket, a code, or the
# seeding a finished download turns into). Unset = until you stop it by hand,
# which is the default: the file is meant to be fetchable. Set one or both when
# you would rather it lapsed on its own — otherwise every file you receive leaves
# a share that comes back at every restart.
#share_days = 30
#share_copies = 5

# Swarm mode for shared arvc… tickets: "on", "off", or "relay-only" (privacy).
#swarm = "on"

# Parallel chunk fetches (1–16).
#concurrency = 4

# Force IPv4-only transport (default: auto-detected).
#ipv4_only = false

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `body` with `ARVOLO_CONFIG_DIR` pointed at a scratch dir.
    ///
    /// Takes the crate-wide [`crate::testlock::ENV`] rather than a private
    /// mutex: the config dir is a *process-global* env var that every store in
    /// this crate reads, so a private lock would only serialise these tests
    /// against each other while still racing the book's.
    fn with_config_dir(initial: &str, body: impl FnOnce(&std::path::Path)) {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var("ARVOLO_CONFIG_DIR").ok();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        if !initial.is_empty() {
            std::fs::write(dir.path().join("config.toml"), initial).unwrap();
        }
        body(dir.path());
        match prev {
            Some(p) => std::env::set_var("ARVOLO_CONFIG_DIR", p),
            None => std::env::remove_var("ARVOLO_CONFIG_DIR"),
        }
    }

    #[test]
    fn setting_a_key_keeps_the_comments_around_it() {
        with_config_dir("# a comment\n#relay = \"example\"\n#seed = true\n", |dir| {
            set_config_value("relay", Some(toml::Value::String("r.test".into()))).unwrap();
            let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
            assert!(
                text.contains("# a comment"),
                "comments must survive an edit"
            );
            assert!(text.contains("relay = \"r.test\""));
            assert!(text.contains("#seed = true"), "other keys are untouched");
        });
    }

    /// Clearing comments the line out and keeps the old value visible behind the
    /// marker, so restoring it is uncommenting a line rather than retyping one.
    #[test]
    fn clearing_a_key_comments_it_out_in_place() {
        with_config_dir("relay = \"r.test\"\n", |dir| {
            set_config_value("relay", None).unwrap();
            let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
            assert!(text.contains("#relay = \"r.test\""));
            assert_eq!(load_config().relay, None);
        });
    }

    /// The failure this guard exists for. A human writes the key twice — the
    /// generated stub plus their own line further down — and the line editor
    /// replaces only the first. TOML rejects duplicate keys, `load_config`
    /// swallows that into `Config::default()`, and *every* setting silently
    /// reverts to a built-in while the settings screen agrees nothing was ever
    /// configured. Refusing loudly is the only honest outcome.
    #[test]
    fn an_edit_that_would_corrupt_the_file_is_refused_and_changes_nothing() {
        let before = "#seed = true\nrelay = \"a\"\nseed = false\n";
        with_config_dir(before, |dir| {
            let err = set_config_value("seed", Some(toml::Value::Boolean(true)))
                .expect_err("a duplicate key must not be written");
            assert!(format!("{err:#}").contains("duplicate"));
            assert_eq!(
                std::fs::read_to_string(dir.join("config.toml")).unwrap(),
                before,
                "the file must be left exactly as it was"
            );
        });
    }

    /// Appending after a `[table]` header would file the key under that table:
    /// written successfully, read back as unset, silently never applied.
    #[test]
    fn a_key_is_never_appended_into_a_table() {
        let before = "relay = \"a\"\n\n[advanced]\nverbose = true\n";
        with_config_dir(before, |dir| {
            let err = set_config_value("download_dir", Some(toml::Value::String("/tmp".into())))
                .expect_err("appending under a table must be refused");
            assert!(format!("{err:#}").contains("[table]"));
            assert_eq!(
                std::fs::read_to_string(dir.join("config.toml")).unwrap(),
                before
            );
        });
    }

    /// Replacing an *existing* key is still fine even when the file has tables —
    /// the guard is only about where a new line would land.
    #[test]
    fn replacing_an_existing_key_still_works_with_tables_present() {
        with_config_dir("relay = \"a\"\n\n[advanced]\nverbose = true\n", |dir| {
            set_config_value("relay", Some(toml::Value::String("b".into()))).unwrap();
            let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
            assert!(text.contains("relay = \"b\""));
            assert_eq!(load_config().relay.as_deref(), Some("b"));
        });
    }
}
