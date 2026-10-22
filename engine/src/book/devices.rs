//! The machines holding this identity, as a local ledger.
//!
//! The projection of `SyncState::devices` into TOML, alongside the contacts and
//! the marks — with one thing none of those have: **this device writes its own
//! row and nobody else's**. Pairing hands over the seed; a paired machine then
//! announces itself on its next sync round, and that announcement is the only
//! source for its row (the rule is enforced on the merge side, in `core::sync`).
//!
//! What is written here is deliberately thin — a name, a platform, a "last seen"
//! — because it is self-reported and nothing outside can check any of it. It is
//! enough to aim a send at one machine rather than at whichever wakes first, and
//! it is not enough to be called an inventory.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use arvolo_core::sync::DeviceId;
use serde::{Deserialize, Serialize};

use super::*;

/// How much a `last_seen` may move before it is worth a new Lamport tick.
///
/// A sync round runs every five minutes and would otherwise stamp a new clock
/// every time, on every machine, for ever — a counter climbing at a steady rate
/// with nothing behind it. A minute is finer than anything the UI shows (it
/// reads as "visto N fa") and coarse enough that the register only moves when
/// the address book does.
const LAST_SEEN_GRANULARITY: u64 = 60;

/// One row of `devices.toml`.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct DeviceRow {
    pub name: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub last_seen: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Devices {
    /// Lowercase hex device id → what that machine says about itself.
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceRow>,
}

pub fn load_devices() -> Devices {
    std::fs::read_to_string(devices_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_devices(d: &Devices) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let s = toml::to_string_pretty(d).context("serialize devices")?;
    write_private(&devices_path(), &s).context("write devices")?;
    Ok(())
}

/// The default name for a machine that has never been named: its hostname, or
/// what the platform is if even that is unavailable. Editable afterwards, on
/// the machine itself — see the authorship rule in `core::sync`.
///
/// A phone has no hostname worth the name: iOS answers `localhost`, which is
/// not a name a person can pick out of a list and not a machine anyone can aim
/// a send at. That answer counts as "unavailable" and the platform's own word
/// stands in for it.
pub fn default_device_name() -> String {
    let host = gethostname::gethostname().to_string_lossy().to_string();
    let host = host.trim().trim_end_matches(".local").to_string();
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        platform_device_name().to_string()
    } else {
        host
    }
}

/// What a machine is called when it cannot say: the word on its box.
pub fn platform_device_name() -> &'static str {
    match std::env::consts::OS {
        "ios" => "iPhone",
        "android" => "Android",
        "macos" => "Mac",
        other => other,
    }
}

/// Resolve `--device <who>` against the register: an exact name, else a
/// case-insensitive name, else a unique id prefix.
///
/// Ambiguity is an error rather than a pick. The whole point of aiming a send is
/// that it lands on the machine the user meant, so guessing between two would
/// defeat the feature in the one case where it matters.
pub fn resolve_device(who: &str) -> Result<DeviceId> {
    let who = who.trim();
    anyhow::ensure!(!who.is_empty(), "name a device");
    let devices = load_devices().devices;

    let mut hits: Vec<(&String, &DeviceRow)> =
        devices.iter().filter(|(_, r)| r.name == who).collect();
    if hits.is_empty() {
        let lower = who.to_lowercase();
        hits = devices
            .iter()
            .filter(|(id, r)| r.name.to_lowercase() == lower || id.starts_with(&lower))
            .collect();
    }
    match hits.len() {
        0 => anyhow::bail!(
            "no device called '{who}' — `arvolo device list` shows the ones this \
             identity knows about"
        ),
        1 => decode_device_id(hits[0].0),
        n => {
            let names: Vec<&str> = hits.iter().map(|(_, r)| r.name.as_str()).collect();
            anyhow::bail!(
                "'{who}' matches {n} devices ({}) — be more specific",
                names.join(", ")
            )
        }
    }
}

/// Parse a lowercase-hex device id back into its bytes.
pub fn decode_device_id(hex: &str) -> Result<DeviceId> {
    let bytes = data_encoding::HEXLOWER
        .decode(hex.as_bytes())
        .context("invalid device id")?;
    let mut id = [0u8; 16];
    anyhow::ensure!(bytes.len() == id.len(), "invalid device id length");
    id.copy_from_slice(&bytes);
    Ok(id)
}

/// Round a timestamp down to [`LAST_SEEN_GRANULARITY`].
pub fn quantize_last_seen(now: u64) -> u64 {
    now - (now % LAST_SEEN_GRANULARITY)
}
