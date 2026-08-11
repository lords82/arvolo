use std::path::Path;

/// True if `e` (or anything in its source chain) is a *local* filesystem error
/// that won't fix itself by retrying: the disk is full, a quota is exhausted, or
/// the target is read-only. Such an error is not a provider/network failure —
/// reassigning the piece to another peer would just spin — so the receiver pauses
/// (resumably) instead. Matched by raw OS error code so it stays correct
/// regardless of the toolchain's `io::ErrorKind` coverage.
///
/// The codes are per-platform and must not be pooled into one list: `raw_os_error`
/// yields errnos on unix and Win32 codes on Windows, and the two spaces collide
/// with different meanings (28 is `ENOSPC` on unix but `ERROR_OUT_OF_PAPER` on
/// Windows; 30 is `EROFS` vs `ERROR_READ_FAULT`). A shared list would pause a
/// download on a printer error and, worse, miss a genuinely full Windows disk.
pub(super) fn is_local_storage_error(e: &anyhow::Error) -> bool {
    #[cfg(unix)]
    const CODES: &[i32] = &[
        28, // ENOSPC — no space left on device
        30, // EROFS  — read-only filesystem
        // EDQUOT (over quota) differs across platforms: Linux 122, macOS/BSD 69.
        #[cfg(target_os = "linux")]
        122,
        #[cfg(not(target_os = "linux"))]
        69,
    ];
    #[cfg(windows)]
    const CODES: &[i32] = &[
        112,  // ERROR_DISK_FULL           — no space left on the volume
        39,   // ERROR_HANDLE_DISK_FULL    — the older, handle-scoped disk-full
        19,   // ERROR_WRITE_PROTECT       — the read-only-medium analogue of EROFS
        1816, // ERROR_NOT_ENOUGH_QUOTA   — the process quota, closest to EDQUOT
    ];
    #[cfg(not(any(unix, windows)))]
    const CODES: &[i32] = &[];
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .and_then(|io| io.raw_os_error())
            .is_some_and(|code| CODES.contains(&code))
    })
}

/// Which path to ask the OS about when the question is "how much room is there
/// for `path`". The file itself once it exists (it settles any doubt about which
/// filesystem it actually landed on, symlinks included); otherwise its directory,
/// since neither `statvfs` nor `GetDiskFreeSpaceExW` will answer for a name that
/// isn't there yet — and the current directory when the name has no parent at
/// all, which is exactly where a bare relative filename would be written.
#[cfg(any(unix, windows))]
fn probe_target(path: &Path) -> &Path {
    if path.exists() {
        return path;
    }
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Best-effort free space (bytes) on the filesystem holding `path`, or `None` if
/// it can't be determined (unsupported platform, or the syscall fails). Used only
/// for a *pre-flight* check — an unknown or wrong figure must never block a valid
/// download, so callers treat `None` as "proceed".
#[cfg(unix)]
pub(super) fn available_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let cpath = std::ffi::CString::new(probe_target(path).as_os_str().as_bytes()).ok()?;
    // SAFETY: `cpath` is a valid NUL-terminated C string; `statvfs` fills `stat`
    // only on success (returns 0), and we read it only then.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(cpath.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // Blocks available to an unprivileged process × fragment size.
    Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

/// Windows counterpart. The first out-parameter is the space available to the
/// calling user, i.e. with any quota already taken off — the closest match to
/// `statvfs`'s "blocks available to an unprivileged process".
#[cfg(windows)]
pub(super) fn available_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = probe_target(path)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut free_to_caller: u64 = 0;
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 path that outlives the call;
    // the two unused out-params are passed as null, which the API documents as
    // allowed, and `free_to_caller` is read only when the call reports success.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free_to_caller)
}

#[cfg(not(any(unix, windows)))]
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

    fn os_err(code: i32) -> anyhow::Error {
        anyhow::Error::new(std::io::Error::from_raw_os_error(code))
    }

    /// What the running platform reports when the volume is full.
    const DISK_FULL: i32 = if cfg!(windows) { 112 } else { 28 };

    // Disk full, read-only medium and over-quota are the local, non-retryable
    // conditions that should trigger a pause — under each platform's own names.
    #[test]
    fn recognizes_local_storage_errors() {
        #[cfg(unix)]
        {
            assert!(is_local_storage_error(&os_err(28)), "ENOSPC");
            assert!(is_local_storage_error(&os_err(30)), "EROFS");
            #[cfg(target_os = "linux")]
            assert!(is_local_storage_error(&os_err(122)), "EDQUOT (linux)");
            #[cfg(not(target_os = "linux"))]
            assert!(is_local_storage_error(&os_err(69)), "EDQUOT (bsd/macos)");
        }
        #[cfg(windows)]
        {
            assert!(is_local_storage_error(&os_err(112)), "ERROR_DISK_FULL");
            assert!(
                is_local_storage_error(&os_err(39)),
                "ERROR_HANDLE_DISK_FULL"
            );
            assert!(is_local_storage_error(&os_err(19)), "ERROR_WRITE_PROTECT");
            assert!(
                is_local_storage_error(&os_err(1816)),
                "ERROR_NOT_ENOUGH_QUOTA"
            );
        }
    }

    // A network/other error must NOT be mistaken for a disk-full pause.
    #[test]
    fn ignores_non_storage_errors() {
        assert!(
            !is_local_storage_error(&os_err(2)),
            "ENOENT / FILE_NOT_FOUND"
        );
        assert!(!is_local_storage_error(&anyhow::anyhow!(
            "connect chunk provider: timeout"
        )));
    }

    // The two code spaces overlap with unrelated meanings, so each platform must
    // match only its own: 28/30 are ENOSPC/EROFS on unix but "out of paper" and
    // "read fault" on Windows, and 112/19 are the reverse.
    #[test]
    fn does_not_borrow_the_other_platforms_codes() {
        #[cfg(unix)]
        {
            assert!(
                !is_local_storage_error(&os_err(112)),
                "ENOMEDIUM is not full"
            );
            assert!(!is_local_storage_error(&os_err(19)), "ENODEV is not full");
        }
        #[cfg(windows)]
        {
            assert!(!is_local_storage_error(&os_err(28)), "out of paper");
            assert!(!is_local_storage_error(&os_err(30)), "read fault");
        }
    }

    // The classifier walks the whole source chain, so a disk-full error still
    // counts once it's been wrapped with `.context(...)` (as the commit path does).
    #[test]
    fn detects_storage_error_through_context() {
        let wrapped = os_err(DISK_FULL)
            .context("write chunk")
            .context("commit piece 7");
        assert!(is_local_storage_error(&wrapped));
    }

    // The pre-flight check must produce a real figure on every supported platform:
    // a silent `None` would disable the check without anyone noticing. Both cases
    // matter — the file already there (a resume) and the file not yet created (a
    // fresh download, which is when there is most left to write).
    #[test]
    fn reports_free_space_whether_or_not_the_file_exists_yet() {
        let dir = tempfile::tempdir().unwrap();
        let not_yet = dir.path().join("download.bin");
        let before = available_space(&not_yet).expect("free space is knowable for a new file");
        assert!(
            before > 0,
            "a writable temp dir should report some free space"
        );

        std::fs::write(&not_yet, b"partial").unwrap();
        let after = available_space(&not_yet).expect("free space is knowable for a partial");
        assert!(after > 0);
    }
}
