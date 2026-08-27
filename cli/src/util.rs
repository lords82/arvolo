use anyhow::{Context, Result};

use crate::book;

use crate::output::vprintln;

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
pub(crate) fn require_relay(relay: Option<String>) -> Result<String> {
    let resolved = relay
        .map(|r| book::normalize_relay(&r))
        .or_else(book::default_relay_or_builtin)
        .context(
            "a relay is required: pass --relay <host>, set ARVOLO_RELAY, or configure `relay`",
        )?;
    vprintln!("using relay: {resolved}");
    Ok(resolved)
}

/// Refuse a command that can only work over a direct P2P transfer when the user has
/// turned P2P off (`p2p = false` / `ARVOLO_P2P=off`).
///
/// The bind itself already refuses (see [`arvolo_core::transfer::p2p_enabled`]), so
/// this is not what makes the setting hold — it is what makes it *legible*. Without
/// it the user reads a message about endpoints and mailboxes at the bottom of a
/// command that never mentioned either. `what` names the thing being refused, e.g.
/// "a pairing code".
pub(crate) fn ensure_p2p(what: &str) -> Result<()> {
    if arvolo_core::transfer::p2p_enabled() {
        return Ok(());
    }
    Err(crate::output::refuse(
        format!(
            "{what} needs a direct transfer, and P2P is off. Go through the relay:\n\
             \x20   arvolo send <paths> --to <who> --mailbox   (or --link, for a browser link)"
        ),
        "P2P is off in config.toml (`p2p = false`) or in the environment \
         (ARVOLO_P2P=off). Both relay routes are plain HTTP, so they honour `proxy` too.",
    ))
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

// Everything below used to be written out here. It now lives in `arvolo-engine`,
// where a phone can reach it without a daemon, and is re-exported under the names
// this crate has always used — `crate::my_identity()`, `util::encode_id`, and the
// rest resolve exactly as before.
pub(crate) use arvolo_engine::identity::{
    auto_accepts, encode_id, identity_path, is_self, my_identity,
    resolve_recipient_or_self_with_relay, SELF_RECIPIENT,
};
pub(crate) use arvolo_engine::now_unix;
pub(crate) use arvolo_engine::payload::resolve_payload;
