use std::path::Path;

/// True if `e` (or anything in its source chain) is a *local* filesystem error
/// that won't fix itself by retrying: the disk is full (`ENOSPC`), a quota is
/// exhausted (`EDQUOT`), or the target is read-only (`EROFS`). Such an error is
/// not a provider/network failure — reassigning the piece to another peer would
/// just spin — so the receiver pauses (resumably) instead. Matched by raw errno
/// so it stays correct regardless of the toolchain's `io::ErrorKind` coverage.
pub(super) fn is_local_storage_error(e: &anyhow::Error) -> bool {
    const ENOSPC: i32 = 28;
    const EROFS: i32 = 30;
    // EDQUOT differs across platforms (Linux 122, macOS/BSD 69).
    #[cfg(target_os = "linux")]
    const EDQUOT: i32 = 122;
    #[cfg(not(target_os = "linux"))]
    const EDQUOT: i32 = 69;
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .and_then(|io| io.raw_os_error())
            .is_some_and(|errno| matches!(errno, ENOSPC | EROFS | EDQUOT))
    })
}

/// Best-effort free space (bytes) on the filesystem holding `path`, or `None` if
/// it can't be determined (non-unix, or the syscall fails). Used only for a
/// *pre-flight* check — an unknown or wrong figure must never block a valid
/// download, so callers treat `None` as "proceed".
#[cfg(unix)]
pub(super) fn available_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `cpath` is a valid NUL-terminated C string; `statvfs` fills `stat`
    // only on success (returns 0), and we read it only then.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(cpath.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // Blocks available to an unprivileged process × fragment size.
    Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

#[cfg(not(unix))]
pub(super) fn available_space(_path: &Path) -> Option<u64> {
    None
}

/// Human-facing reason for a disk-full [`RecvEvent::Paused`] / [`RecvOutcome::Paused`].
pub(super) fn disk_full_reason(output: &Path) -> String {
    format!(
        "not enough disk space to write {} — free up space and re-run to resume",
        output.display()
    )
}

#[cfg(test)]
mod storage_error_tests {
    use super::*;

    fn os_err(errno: i32) -> anyhow::Error {
        anyhow::Error::new(std::io::Error::from_raw_os_error(errno))
    }

    // ENOSPC (disk full), EROFS (read-only fs) and EDQUOT (over quota) are the
    // local, non-retryable conditions that should trigger a pause.
    #[test]
    fn recognizes_local_storage_errnos() {
        assert!(is_local_storage_error(&os_err(28)), "ENOSPC");
        assert!(is_local_storage_error(&os_err(30)), "EROFS");
        #[cfg(target_os = "linux")]
        assert!(is_local_storage_error(&os_err(122)), "EDQUOT (linux)");
        #[cfg(not(target_os = "linux"))]
        assert!(is_local_storage_error(&os_err(69)), "EDQUOT (bsd/macos)");
    }

    // A network/other error must NOT be mistaken for a disk-full pause.
    #[test]
    fn ignores_non_storage_errors() {
        assert!(!is_local_storage_error(&os_err(2)), "ENOENT");
        assert!(!is_local_storage_error(&anyhow::anyhow!(
            "connect chunk provider: timeout"
        )));
    }

    // The classifier walks the whole source chain, so a disk-full error still
    // counts once it's been wrapped with `.context(...)` (as the commit path does).
    #[test]
    fn detects_storage_error_through_context() {
        let wrapped = os_err(28).context("write chunk").context("commit piece 7");
        assert!(is_local_storage_error(&wrapped));
    }
}
