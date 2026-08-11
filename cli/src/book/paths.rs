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

/// Shut the config directory to its owner, once, so everything written inside it
/// inherits that.
///
/// This is the Windows half of a protection unix gets per file: there, every
/// record holding a secret is chmod'd 0600 and the directory 0700. Here there are
/// no modes — access is a list of entries on the object — and the practical
/// equivalent is to fix the list on the *directory* and let new files inherit it,
/// rather than to re-apply it on every write.
///
/// Done with `icacls` rather than the Win32 ACL functions, which would mean
/// `windows-sys` and a page of unsafe to build a descriptor by hand. `icacls`
/// ships with Windows, and this runs once per process at startup: `/inheritance:r`
/// drops whatever the parent handed down, then the current user is granted full
/// control and nobody else is named. Administrators and SYSTEM keep their access
/// by other means — as they do on unix, where root reads a 0600 file regardless.
///
/// Best effort by design: a machine where this fails is one where the directory
/// already inherits the profile's own restrictions, which is the common case and
/// is why this is hardening rather than a gate.
#[cfg(windows)]
pub fn restrict_config_dir() {
    let dir = config_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(user) = std::env::var("USERNAME") else {
        return;
    };
    let out = std::process::Command::new("icacls")
        .arg(&dir)
        .args(["/inheritance:r", "/grant"])
        .arg(format!("{user}:(OI)(CI)F"))
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        Ok(o) => tracing::debug!(
            "could not restrict {}: {}",
            dir.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => tracing::debug!("could not run icacls: {e}"),
    }
}

/// Nothing to do: on unix each file carries its own mode, applied where it is
/// written (see the `restrict` helpers beside the record stores).
#[cfg(unix)]
pub fn restrict_config_dir() {}

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
///
/// Writes a sibling temp file, chmods it, then `rename`s it over the target. The
/// rename is atomic within a directory, so a reader never sees a half-written
/// ledger — which a plain `write` allowed, because `arvolo contacts …` runs as a
/// separate process from the daemon and `project_to_ledgers` rewrites all five
/// files from scratch on every sync round. Permissions go on the temp file
/// *before* the rename, so the final path is never briefly world-readable.
///
/// This closes torn reads, **not** lost updates: two processes that each
/// read-modify-write the same ledger can still have one overwrite the other. That
/// needs locking across the read too, and is left as a known limitation.
pub(crate) fn write_private_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    // A failed rename would otherwise leave the temp file behind for good.
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
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

pub(crate) fn blocked_path() -> PathBuf {
    config_dir().join("blocked.toml")
}

pub(crate) fn names_path() -> PathBuf {
    config_dir().join("names.toml")
}

/// A content fingerprint of the address book — the four ledgers that shape what a
/// front-end shows in "Persone". Any `arvolo contacts …` run is a *separate process*
/// writing these files behind the daemon's back, so the daemon polls this stamp to
/// notice; a change in the value means "the book moved, refetch it".
///
/// It hashes the bytes rather than (len, mtime) on purpose: the files are tiny, and
/// two edits landing inside one mtime tick with an unchanged length would otherwise
/// read as "no change". `seen.toml` is deliberately left out — it records every
/// receipt and would fire on traffic, not on a book edit.
pub(crate) fn book_stamp() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in [
        contacts_path(),
        names_path(),
        verified_path(),
        trusted_path(),
        blocked_path(),
    ] {
        // A missing file is a state like any other (hashes as the empty vec), so
        // creating or deleting one moves the stamp.
        std::fs::read(&p).unwrap_or_default().hash(&mut h);
    }
    h.finish()
}
