use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Blocked {
    #[serde(default, deserialize_with = "de_marks")]
    pub(crate) blocked: Marks,
}

pub(crate) fn load_blocked() -> Blocked {
    std::fs::read_to_string(blocked_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_blocked(b: &Blocked) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(b).context("serialize blocked")?;
    write_private(&blocked_path(), &text).context("write blocked")
}

/// Should offers from this identity be dropped without asking?
///
/// The fourth and only *negative* axis, alongside seen / verified / trusted. It is
/// checked before anything else on an incoming offer: without it, the only defence
/// against a stranger sending repeatedly is to decline each attempt by hand, which
/// is not a defence so much as a chore.
pub fn is_blocked(id_b32: &str) -> bool {
    load_blocked().blocked.contains_key(id_b32)
}

/// Block an identity. Takes a saved contact name **or** a raw base32 id — the id
/// matters, because the thing you want silenced is usually a stranger you have
/// no name for.
pub fn mark_blocked(who: &str) -> Result<String> {
    let id = resolve_recipient_id(who)?;
    let mut b = load_blocked();
    b.blocked.insert(id.clone(), marked_now());
    save_blocked(&b)?;
    Ok(id)
}

/// Unblock an identity. `Ok(false)` means it wasn't blocked.
pub fn unmark_blocked(who: &str) -> Result<bool> {
    let id = resolve_recipient_id(who)?;
    let mut b = load_blocked();
    if b.blocked.remove(&id).is_none() {
        return Ok(false);
    }
    save_blocked(&b)?;
    Ok(true)
}

/// Every blocked identity and when it was blocked, for listing.
pub fn blocked_list() -> Vec<(String, u64)> {
    load_blocked().blocked.into_iter().collect()
}
