//! Putting a finished download where a person can find it, on iOS.
//!
//! The same reason as Android's: a receive is written in place, pre-allocated and
//! filled at offsets with a resume sidecar beside it, so it has to land in the
//! app's own storage and be moved once it is whole. See
//! [`arvolo_core::manager::SavePublisher`].
//!
//! What differs is the destination, and how cheap it is. iOS has no shared
//! Downloads to write into — an app's files live in the app — but `Documents/`
//! **is** a folder in the Files app when the bundle says so, and this one's does:
//! `UIFileSharingEnabled` puts "Arvolo" under *On My iPhone*, and
//! `LSSupportsOpeningDocumentsInPlace` lets other apps open what is in it without
//! taking a copy first (both in `Info.ios.plist`). So publishing here is a
//! `rename` on the same volume: instant, no second copy, no doubled peak for a
//! 250 MB file — and the folder the person browses is the folder the engine can
//! still open by path, which keeps re-sending and seeding working with no
//! descriptor dance at all.
//!
//! Without this the files were real, complete and verified, and sat in
//! `Library/Application Support/.../incoming` where nothing on the phone could
//! reach them. A transfer nobody can open has not arrived.

use std::path::{Path, PathBuf};

#[cfg(target_os = "ios")]
use anyhow::{Context, Result};
#[cfg(target_os = "ios")]
use arvolo_core::manager::{SavePublisher, SavedLocation};

/// Publishes into the app's `Documents`, which is what Files shows as "Arvolo".
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub struct DocumentsPublisher {
    pub dir: PathBuf,
}

#[cfg(target_os = "ios")]
impl SavePublisher for DocumentsPublisher {
    fn publish(&self, staged: &Path, name: &str) -> Result<SavedLocation> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("create {}", self.dir.display()))?;
        let dest = free_name(&self.dir, name);

        // Same volume, so this is a directory entry being moved and nothing is
        // copied — which is also why it works unchanged for the folder an
        // unpacked archive arrives as. The fallback exists because "same volume"
        // is an assumption about a layout Apple owns, not a guarantee: if it
        // ever stops holding, a slow publish beats a failed one.
        if std::fs::rename(staged, &dest).is_err() {
            copy_across(staged, &dest).with_context(|| format!("publish {} to Documents", name))?;
            let _ = remove(staged);
        }
        Ok(SavedLocation::Path(dest))
    }
}

/// `name`, or `name (2)`, or `name (3)`… — the convention the engine already uses
/// for a download landing on top of one that is there, kept identical so a file
/// published here is named the way the same file would be on a desktop.
///
/// Compiled everywhere, unlike the `impl` above: it is the only part of this file
/// that is a decision rather than a system call, so it is the part a test on a
/// laptop can hold to account.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 2..10_000 {
        let candidate = match ext {
            Some(e) => dir.join(format!("{stem} ({n}).{e}")),
            None => dir.join(format!("{stem} ({n})")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    // Ten thousand files of one name is not a case to design for, but it is a
    // case to not overwrite in.
    dir.join(format!("{stem}-{}", std::process::id()))
}

#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn copy_across(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_across(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn remove(p: &Path) -> std::io::Result<()> {
    if p.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The naming is the only part of this that is worth a test and the only part
    /// that runs off the phone: everything else is a `rename` the platform owns.
    #[test]
    fn a_second_file_of_the_same_name_gets_a_number() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(free_name(d, "a.txt"), d.join("a.txt"));
        std::fs::write(d.join("a.txt"), b"x").unwrap();
        assert_eq!(free_name(d, "a.txt"), d.join("a (2).txt"));
        std::fs::write(d.join("a (2).txt"), b"x").unwrap();
        assert_eq!(free_name(d, "a.txt"), d.join("a (3).txt"));
    }

    #[test]
    fn a_name_without_an_extension_still_gets_one() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("notes"), b"x").unwrap();
        assert_eq!(free_name(d, "notes"), d.join("notes (2)"));
    }
}
