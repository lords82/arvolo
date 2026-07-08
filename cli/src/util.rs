use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::crypto::{Identity, PublicId};
use arvolo_core::flow::{self};

use crate::book;

use crate::output::vprintln;

/// Resolve the send inputs to a single payload file: a lone file is sent as-is;
/// a folder or several paths are packed into a temp tar. Returns
/// `(payload, suggested_name, is_archive, temp_to_cleanup)`.
pub(crate) fn resolve_payload(
    paths: &[PathBuf],
) -> Result<(PathBuf, String, bool, Option<PathBuf>)> {
    if paths.len() == 1 && paths[0].is_file() {
        let name = paths[0]
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        return Ok((paths[0].clone(), name, false, None));
    }
    for p in paths {
        anyhow::ensure!(p.exists(), "{} does not exist", p.display());
    }
    let name = if paths.len() == 1 {
        paths[0]
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "bundle".into())
    } else {
        "bundle".into()
    };
    let temp = book::temp_dir().join(format!("arvolo-send-{}.tar", std::process::id()));
    flow::pack_tar(paths, &temp).context("pack archive")?;
    Ok((temp.clone(), name, true, Some(temp)))
}

// ---- presence: stay-online receive (listen) and push-to-contact -----------

/// Human-readable byte size for offer/progress display.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// User-facing message when a transfer hits the relay's per-session offload cap:
/// explain it's a free/shared relay carrying only part of the transfer, and that a
/// private relay lifts the cap. `limit_bytes` is 0 when the relay didn't report one.
pub(crate) fn relay_capped_line(limit_bytes: u64) -> String {
    let cap = if limit_bytes > 0 {
        format!(" (~{} per transfer)", human_size(limit_bytes))
    } else {
        String::new()
    };
    format!(
        "Relay offload limit reached{cap}. This relay is free and shared, so it \
         carries only part of any single transfer — the rest goes over direct P2P \
         (both devices must be online at once). To offload more through a relay, run \
         your own private relay (arvolo-relay) and point at it with ARVOLO_RELAY."
    )
}

/// Current unix time in seconds.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A compact human duration, largest two units (e.g. "7d", "3h 20m", "45s").
pub(crate) fn human_duration(secs: u64) -> String {
    if secs == 0 {
        return "0s".into();
    }
    let units = [("d", 86400u64), ("h", 3600), ("m", 60), ("s", 1)];
    let mut parts = Vec::new();
    let mut rem = secs;
    for (label, size) in units {
        if rem >= size {
            parts.push(format!("{}{label}", rem / size));
            rem %= size;
        }
        if parts.len() == 2 {
            break;
        }
    }
    parts.join(" ")
}

/// Resolve the relay to use for presence, requiring one (offers can't work P2P).
pub(crate) fn require_relay(relay: Option<String>, use_http: bool) -> Result<String> {
    let resolved = relay
        .map(|r| book::normalize_relay(&r, use_http))
        .or_else(book::default_relay_or_builtin)
        .context(
            "a relay is required: pass --relay <host>, set ARVOLO_RELAY, or configure `relay`",
        )?;
    vprintln!("using relay: {resolved}");
    Ok(resolved)
}

/// Parse a download link (`https://<relay>/dl/<claim>[#key]`) into its relay base
/// URL and the claim. The `#fragment` (the key) is ignored — revoking needs only
/// the relay and claim.
pub(crate) fn parse_dl_link(link: &str) -> Result<(String, String)> {
    let no_frag = link.split('#').next().unwrap_or(link);
    let (relay, claim) = no_frag
        .rsplit_once("/dl/")
        .context("not an arvolo download link (expected …/dl/<claim>)")?;
    let claim = claim.trim_matches('/');
    anyhow::ensure!(!claim.is_empty(), "download link is missing its claim");
    Ok((relay.trim_end_matches('/').to_string(), claim.to_string()))
}

pub(crate) fn identity_path() -> PathBuf {
    if let Ok(p) = std::env::var("ARVOLO_IDENTITY") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/arvolo/identity.key")
}

pub(crate) fn my_identity() -> Result<Identity> {
    Identity::load_or_create(&identity_path()).context("load identity")
}

pub(crate) fn encode_id(p: &PublicId) -> String {
    data_encoding::BASE32_NOPAD
        .encode(&p.to_bytes())
        .to_lowercase()
}
