use std::path::{Path, PathBuf};

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
pub(crate) fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    write_private_bytes(path, contents.as_bytes())
}

/// Binary counterpart of [`write_private`] for the postcard-encoded sync sidecar.
pub(crate) fn write_private_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub(crate) fn contacts_path() -> PathBuf {
    config_dir().join("contacts.toml")
}

pub(crate) fn seen_path() -> PathBuf {
    config_dir().join("seen.toml")
}

pub(crate) fn verified_path() -> PathBuf {
    config_dir().join("verified.toml")
}

pub(crate) fn trusted_path() -> PathBuf {
    config_dir().join("trusted.toml")
}

pub(crate) fn names_path() -> PathBuf {
    config_dir().join("names.toml")
}
