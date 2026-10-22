//! What this identity calls itself — the one name that leaves for other people.
//!
//! There are two names in this program and they are not the same thing, which is
//! the confusion this file exists to end:
//!
//! - **the identity's name** (here): chosen by the user, identical on every
//!   machine holding the identity, and advertised inside each sealed offer as
//!   `Offer::sender_name`. It is a petname claim and never an authenticated fact.
//! - **a device's name** (`devices.rs`): defaulted from the hostname, editable
//!   only on the machine it describes, and visible to nobody but its owner. It
//!   exists to aim `send --to me --device laptop`, not to introduce anybody.
//!
//! The identity name lived in `config.toml` until now, which does not sync — so
//! it was per-machine by accident. A phone paired to a desktop called "Lorenzo"
//! advertised nothing at all, and a recipient taking files from both saw one key
//! that kept changing its story: the exact shape the advertised-name warning is
//! built to flag. It is a register in the address-book CRDT instead
//! ([`arvolo_core::sync::SelfName`]), projected here.
//!
//! This file is the projection, in the same relationship to the sidecar as
//! `contacts.toml` — the ledger every reader uses, folded back into the CRDT by
//! `build_local_snapshot` when it is edited behind the daemon's back.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

/// `me.toml`: one key, because there is one thing an identity says about itself.
#[derive(Default, Serialize, Deserialize)]
pub struct Me {
    #[serde(default)]
    pub display_name: String,
}

pub fn load_me() -> Me {
    std::fs::read_to_string(me_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_me(m: &Me) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(m).context("serialize me")?;
    write_private(&me_path(), &text).context("write me")
}

/// The local user's self-chosen display name, advertised inside every outgoing
/// offer (`arvolo me name "…"`). Empty string when unset — meaning "don't
/// advertise a name", which is the default and always has been.
///
/// Falls back to the old `config.toml` key only while `me.toml` does not exist
/// yet, so an install that predates the register keeps its name from the first
/// read rather than from the first sync round. The guard is the *file*, not an
/// empty value: conditioning it on the value would resurrect an old config name
/// every time the user cleared the new one.
pub fn my_display_name() -> String {
    let me = load_me();
    let name = me.display_name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    if me_path().exists() {
        return String::new();
    }
    legacy_display_name()
}

/// Persist the local user's display name and stamp it into the sync register, so
/// it reaches the identity's other machines on their next round. A blank value
/// clears it.
///
/// Both halves, in one place, for the reason [`forget_device`] gives about its
/// own pair: the ledger is what this machine shows and the register is what it
/// tells the others, and either alone is a name that silently disagrees with
/// itself. Every caller — the daemon's `SetMyName`, the config patch from the
/// window, `arvolo me name` with no daemon running — goes through here, which is
/// why none of them has to know about the clock.
pub fn set_my_display_name(name: &str) -> Result<()> {
    // The one string here that ends up rendered on somebody else's screen, so it
    // is sanitised on the way in exactly as a device rename already is.
    let name = crate::text::sanitize_display(name.trim());
    save_me(&Me {
        display_name: name.clone(),
    })?;
    stamp_my_name(&name)
}
