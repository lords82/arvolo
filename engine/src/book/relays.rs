//! Where each contact listens.
//!
//! Everything else arvolo hands out carries its own relay — the `arvc…` ticket, the
//! `arvm…`, the `/dl/` link, the `N-word-word@relay` code — so those work between
//! two people on different relays. A contact was the exception: `name -> id` says
//! who, never where, and the sender ends up posting the offer into an inbox on a
//! relay the recipient never reads. Nothing errors; they just look permanently
//! offline.
//!
//! This ledger closes that gap. It is **routing, not identity**: the public key is
//! still what proves who someone is, and `verified`/`trusted` still key off it. A
//! wrong relay here costs a delivery, never confidentiality — the far relay only
//! ever sees a blinded slot and an opaque blob. So it is stored id-keyed next to
//! the other ledgers, and an absent row means "use my own relay", which is exactly
//! what every send did before this existed.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

/// Base32 id → the relay that identity listens on, canonicalized.
#[derive(Default, Serialize, Deserialize)]
pub struct Relays {
    #[serde(default)]
    pub relays: BTreeMap<String, String>,
}

pub fn load_relays() -> Relays {
    std::fs::read_to_string(relays_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_relays(r: &Relays) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(r).context("serialize relays")?;
    write_private(&relays_path(), &text).context("write relays")
}

/// One canonical spelling for a relay, so "is this the same relay I use?" is a
/// string comparison and not a guess. A bare host gains `https://` (an explicit
/// `http://` is left alone, for a LAN or dev relay), and the trailing slash goes:
/// every URL is built by appending `/v1/…`, so `host/` and `host` are the same
/// relay written two ways.
pub fn canonical_relay(raw: &str) -> String {
    arvolo_core::code::normalize_relay(raw.trim())
        .trim_end_matches('/')
        .to_string()
}

/// Split the shareable form of an identity — `<id>@<relay>` — into its two halves.
///
/// Unambiguous by construction: base32 is `A-Z2-7`, so an `@` can only be the
/// separator. A bare id (no `@`) yields no relay, which is the "use my own relay"
/// default and the shape every id had before this.
pub fn split_id_and_relay(s: &str) -> (&str, Option<String>) {
    match s.trim().split_once('@') {
        Some((id, relay)) if !relay.trim().is_empty() => (id, Some(canonical_relay(relay))),
        // A trailing bare `@` is a typo, not a relay; let the id half fail to
        // decode on its own rather than inventing an empty relay for it.
        Some((id, _)) => (id, None),
        None => (s.trim(), None),
    }
}

/// The relay a given identity listens on, if one was recorded.
pub fn relay_of(id_b32: &str) -> Option<String> {
    load_relays()
        .relays
        .get(id_b32.trim().to_lowercase().as_str())
        .filter(|r| !r.is_empty())
        .cloned()
}

/// Record where an identity listens. Stored even when it matches your own relay:
/// it is a fact about them, and your own relay can change later without theirs
/// having moved. Listings are what hide the redundant case, not storage.
pub fn set_relay(id_b32: &str, relay: &str) -> Result<()> {
    let relay = canonical_relay(relay);
    anyhow::ensure!(!relay.is_empty(), "empty relay");
    let mut r = load_relays();
    r.relays.insert(id_b32.trim().to_lowercase(), relay);
    save_relays(&r)
}

/// Forget where an identity listens, falling back to your own relay. Returns
/// whether there was a row.
pub fn clear_relay(id_b32: &str) -> Result<bool> {
    let mut r = load_relays();
    if r.relays
        .remove(id_b32.trim().to_lowercase().as_str())
        .is_none()
    {
        return Ok(false);
    }
    save_relays(&r)?;
    Ok(true)
}

/// Record where a sender said they listen, from an offer they just sent us.
///
/// Only for identities already in the address book. A stranger's offer is a
/// stranger's claim, and writing a row for it would grow a routing table out of
/// unsolicited mail — the id is not saved either, for the same reason.
///
/// Last write wins over a hand-set value: the sender is describing where they are
/// *now*, which is fresher than what someone typed once. Best-effort — a failed
/// write costs the shortcut, never the transfer, so it never surfaces an error.
/// Returns the relay when it changed something, for callers that want to say so.
pub fn learn_sender_relay(id_b32: &str, relay: &str) -> Option<String> {
    let relay = relay.trim();
    if relay.is_empty() || resolve_name(id_b32).is_none() {
        return None;
    }
    let relay = canonical_relay(relay);
    if relay_of(id_b32).as_deref() == Some(relay.as_str()) {
        return None;
    }
    set_relay(id_b32, &relay).ok().map(|()| relay)
}

/// The shareable spelling of an identity: `<id>@<relay>`, with `https://` dropped
/// the way a pairing code drops it. This is what `arvolo me` prints and what
/// `arvolo contacts add` reads back.
pub fn shareable_id(id_b32: &str, relay: Option<&str>) -> String {
    match relay {
        Some(r) => format!("{id_b32}@{}", arvolo_core::code::compact_relay(r)),
        None => id_b32.to_string(),
    }
}
