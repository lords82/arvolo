use std::path::{Path, PathBuf};

/// Path of the resume sidecar: a bitfield of the pieces already verified and
/// written into `download` (parallel to the sparse output an out-of-order piece
/// swarm produces). It is the sole resume state — there is no length-based resume.
pub(super) fn sidecar_path(download: &Path) -> PathBuf {
    let mut s = download.as_os_str().to_os_string();
    s.push(".arvhave");
    PathBuf::from(s)
}

/// Read the resume sidecar for `download` into a `total`-bit bitfield. A missing or
/// wrong-sized sidecar yields an empty bitfield (start fresh).
pub(super) fn read_sidecar(download: &Path, total: usize) -> Vec<u8> {
    let want = crate::swarm::bitfield_bytes(total);
    match std::fs::read(sidecar_path(download)) {
        Ok(b) if b.len() == want => b,
        _ => crate::swarm::bitfield_new(total),
    }
}

/// Write the resume sidecar (owner-only on unix). Best-effort: a failed write only
/// costs a re-fetch of some pieces on the next run, never correctness.
pub(super) fn write_sidecar(download: &Path, bitfield: &[u8]) {
    let path = sidecar_path(download);
    if std::fs::write(&path, bitfield).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

/// Path of the ticket sidecar: the `arvc…` ticket this partial is being fetched
/// under, so an interrupted download can be resumed without going back to
/// whatever delivered the ticket in the first place.
///
/// That last part is the whole point. A short pairing code hands the ticket over
/// exactly once — the rendezvous slot (or, in v2, that session) is burned on
/// fetch — so a receiver that kept the ticket only in memory had no way back:
/// the partial and its bitfield were on disk, and unusable. Costs one small file.
pub(crate) fn ticket_path(download: &Path) -> PathBuf {
    let mut s = download.as_os_str().to_os_string();
    s.push(".arvticket");
    PathBuf::from(s)
}

/// Record the ticket next to the partial (owner-only on unix). Best-effort: a
/// failed write only costs the ability to resume, never correctness.
///
/// Owner-only matters here — a `Plain`-key ticket carries the file's content key
/// in the clear, the same reason the manager's resume records are 0600.
pub(crate) fn write_ticket(download: &Path, ticket: &str) {
    let path = ticket_path(download);
    if std::fs::write(&path, ticket.as_bytes()).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

/// The ticket recorded next to a partial download, if any.
pub fn read_ticket(download: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(ticket_path(download)).ok()?;
    let t = raw.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// Remove any `{download}.arvpart.*` per-chunk staging files.
pub(super) fn remove_stage_files(download: &Path) {
    let Some(name) = download.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let prefix = format!("{name}.arvpart.");
    let dir = match download.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|f| f.starts_with(&prefix))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod sidecar_tests {
    use super::*;
    use crate::swarm::{bitfield_count, bitfield_has, bitfield_new, bitfield_set};

    // The resume sidecar round-trips an arbitrary (disjoint) piece set, and a
    // missing or wrong-size (corrupt) sidecar is ignored so the download restarts
    // fresh rather than trusting garbage about which pieces are on disk.
    #[test]
    fn sidecar_roundtrip_and_corruption_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("f.out");

        // Missing sidecar → empty bitfield (start fresh).
        assert_eq!(bitfield_count(&read_sidecar(&out, 5)), 0);

        // Round-trip a disjoint set {0, 3}.
        let mut bf = bitfield_new(5);
        bitfield_set(&mut bf, 0);
        bitfield_set(&mut bf, 3);
        write_sidecar(&out, &bf);
        let got = read_sidecar(&out, 5);
        assert!(bitfield_has(&got, 0) && bitfield_has(&got, 3));
        assert!(!bitfield_has(&got, 1) && !bitfield_has(&got, 2) && !bitfield_has(&got, 4));
        assert_eq!(bitfield_count(&got), 2);

        // A wrong-size sidecar (e.g. truncated/corrupt, or a stale one for a
        // different chunk count) is discarded → fresh.
        std::fs::write(sidecar_path(&out), vec![0xffu8; 999]).unwrap();
        assert_eq!(
            bitfield_count(&read_sidecar(&out, 5)),
            0,
            "corrupt/wrong-size sidecar must start fresh, not trust garbage"
        );
    }
}
