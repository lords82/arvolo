use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Unpack a tar archive into `dir`, hardened against path-traversal and symlink
/// escapes. The archive may come from an anonymous `arvc` sender, so every entry is
/// validated explicitly (defense in depth, not trusting the extractor):
///
/// - entry paths (and link targets) must contain only normal components — no
///   absolute paths, no root/prefix, no `..`;
/// - symlink and hardlink entries are refused outright. Our own [`pack_tar`] only
///   ever emits `Directory`/`Regular` entries (symlinks are dereferenced when
///   packing), so a legitimate transfer never contains a link — a link entry is a
///   red flag and dropping it can't break an honest send.
///
/// Attacker-chosen unix permissions/mtime are also not restored.
pub(super) fn unpack_archive_safely(archive: &Path, dir: &Path) -> Result<()> {
    use std::path::Component;

    // True only if every component is a plain name or `.` (rejects absolute paths,
    // a root/prefix, and any `..`).
    fn stays_inside(p: &Path) -> bool {
        p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    }

    let f = std::fs::File::open(archive).context("open downloaded archive")?;
    let mut ar = tar::Archive::new(f);
    ar.set_preserve_permissions(false);
    ar.set_preserve_mtime(false);
    ar.set_overwrite(true);

    for entry in ar.entries().context("read archive entries")? {
        let mut entry = entry.context("read archive entry")?;
        let path = entry.path().context("entry path")?.into_owned();
        anyhow::ensure!(
            stays_inside(&path),
            "archive entry escapes target dir: {}",
            path.display()
        );
        let etype = entry.header().entry_type();
        anyhow::ensure!(
            !matches!(etype, tar::EntryType::Symlink | tar::EntryType::Link),
            "archive contains a link entry ({}), refused",
            path.display()
        );
        // `unpack_in` runs its own containment check and returns Ok(false) if it
        // still refused the entry — treat that as an error rather than a silent skip.
        anyhow::ensure!(
            entry
                .unpack_in(dir)
                .with_context(|| format!("unpack {}", path.display()))?,
            "archive entry refused by extractor: {}",
            path.display()
        );
    }
    Ok(())
}

/// Pack files and/or directories into a tar archive at `dest` (each top-level
/// input keeps its base name inside the archive). Used to send folders/multiple
/// files as one transfer; the receiver unpacks it (see [`crate::flow::recv_chunked`]).
/// Pack `paths` into a tar at `dest` *deterministically*: entries are emitted in
/// a stable sorted order with normalized metadata (mtime/uid/gid zeroed, fixed
/// mode), so the same inputs always yield byte-identical output. That is what
/// lets an interrupted archive send be resumed — repacking reproduces the exact
/// chunk hashes the original ticket promised (verified on resume). Symlinks are
/// followed to their target; broken or special files are skipped.
pub fn pack_tar(paths: &[PathBuf], dest: &Path) -> Result<()> {
    // Gather every regular file (name-in-archive → source) plus the directory
    // entries (so empty dirs survive), then sort for a stable layout.
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    for p in paths {
        let base = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        if p.is_dir() {
            collect_dir(p, &base, &mut files, &mut dirs)?;
        } else {
            files.push((base, p.clone()));
        }
    }
    files.sort();
    dirs.sort();
    dirs.dedup();

    let out = std::fs::File::create(dest)
        .with_context(|| format!("create archive {}", dest.display()))?;
    let mut builder = tar::Builder::new(out);
    builder.mode(tar::HeaderMode::Deterministic);

    for d in &dirs {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        builder
            .append_data(&mut header, format!("{d}/"), std::io::empty())
            .with_context(|| format!("archive dir {d}"))?;
    }
    for (name, src) in &files {
        let data = std::fs::File::open(src).with_context(|| format!("open {}", src.display()))?;
        let len = data
            .metadata()
            .with_context(|| format!("stat {}", src.display()))?
            .len();
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(len);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        builder
            .append_data(&mut header, name, data)
            .with_context(|| format!("archive file {name}"))?;
    }
    builder.finish().context("finish archive")?;
    Ok(())
}

/// Recursively collect a directory's regular files and subdirectories in sorted
/// order (following symlinks to their targets), for deterministic packing.
fn collect_dir(
    dir: &Path,
    prefix: &str,
    files: &mut Vec<(String, PathBuf)>,
    dirs: &mut Vec<String>,
) -> Result<()> {
    dirs.push(prefix.to_string());
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("read dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let child = format!("{prefix}/{}", e.file_name().to_string_lossy());
        // `metadata` (not `symlink_metadata`) follows symlinks; skip anything
        // that isn't a plain file or directory (broken links, sockets, …).
        let Ok(md) = std::fs::metadata(&path) else {
            continue;
        };
        if md.is_dir() {
            collect_dir(&path, &child, files, dirs)?;
        } else if md.is_file() {
            files.push((child, path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    use crate::flow::safe_download_name;

    // A benign archive (dir + regular file, exactly what `pack_tar` emits) unpacks.
    #[test]
    fn benign_archive_unpacks() {
        let src = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("folder")).unwrap();
        std::fs::write(src.path().join("folder/hello.txt"), b"hi").unwrap();
        let tar_path = src.path().join("a.tar");
        pack_tar(&[src.path().join("folder")], &tar_path).unwrap();

        let out = tempfile::tempdir().unwrap();
        unpack_archive_safely(&tar_path, out.path()).unwrap();
        assert_eq!(
            std::fs::read(out.path().join("folder/hello.txt")).unwrap(),
            b"hi"
        );
    }

    // An entry whose path traverses out of the target dir is refused, and nothing
    // is written outside `dir`. `tar`'s high-level builder refuses to WRITE a `..`
    // path, so we hand-forge the header (as a real attacker would) by patching the
    // name field of a benign entry and recomputing the ustar checksum.
    #[test]
    fn path_traversal_entry_is_refused() {
        // A one-entry tar with a benign name + 5 bytes of data.
        let mut bytes = {
            let mut buf = Vec::new();
            {
                let mut b = tar::Builder::new(&mut buf);
                let data = b"pwned";
                let mut h = tar::Header::new_ustar();
                h.set_entry_type(tar::EntryType::Regular);
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, "x", &data[..]).unwrap();
                b.finish().unwrap();
            }
            buf
        };
        // Patch the first header's name field (bytes 0..100) to `../escape.txt`.
        let name = b"../escape.txt";
        for byte in bytes.iter_mut().take(100) {
            *byte = 0;
        }
        bytes[..name.len()].copy_from_slice(name);
        // Recompute the ustar checksum (field 148..156): sum of all 512 header bytes
        // with the checksum field treated as spaces, written as 6 octal digits, NUL,
        // then a space.
        for byte in bytes.iter_mut().take(156).skip(148) {
            *byte = b' ';
        }
        let sum: u32 = bytes[..512].iter().map(|&b| b as u32).sum();
        let chk = format!("{sum:06o}\0 ");
        bytes[148..156].copy_from_slice(chk.as_bytes());

        let dir = tempfile::tempdir().unwrap();
        let tar_path = dir.path().join("evil.tar");
        std::fs::write(&tar_path, &bytes).unwrap();

        let out = tempfile::tempdir().unwrap();
        assert!(unpack_archive_safely(&tar_path, out.path()).is_err());
        // The sibling of the target dir must not have been created.
        assert!(!out.path().parent().unwrap().join("escape.txt").exists());
    }

    // A malicious `arvc` ticket controls the archive `name`; the default unpack dir
    // must be reduced to a single safe component so it can't escape (an absolute
    // path, a `..` traversal, or a nested dir all collapse to their final segment).
    #[test]
    fn attacker_ticket_name_cannot_escape_download_dir() {
        assert_eq!(safe_download_name("photos").as_deref(), Some("photos"));
        assert_eq!(
            safe_download_name("../../.ssh/authorized_keys").as_deref(),
            Some("authorized_keys")
        );
        assert_eq!(
            safe_download_name("/home/victim/.config/autostart").as_deref(),
            Some("autostart")
        );
        assert_eq!(safe_download_name("a/b/c").as_deref(), Some("c"));
        // Names with nothing usable left fall through to the caller's generated name.
        assert_eq!(safe_download_name(".."), None);
        assert_eq!(safe_download_name("."), None);
        assert_eq!(safe_download_name(""), None);
        assert_eq!(safe_download_name("/"), None);
    }

    // A symlink entry is refused outright (our packer never emits one).
    #[test]
    fn symlink_entry_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let tar_path = dir.path().join("link.tar");
        {
            let out = std::fs::File::create(&tar_path).unwrap();
            let mut b = tar::Builder::new(out);
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            h.set_mode(0o777);
            b.append_link(&mut h, "pwn", "/etc/passwd").unwrap();
            b.finish().unwrap();
        }
        let out = tempfile::tempdir().unwrap();
        assert!(unpack_archive_safely(&tar_path, out.path()).is_err());
    }
}
