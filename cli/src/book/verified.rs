use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Verified {
    #[serde(default, deserialize_with = "de_marks")]
    pub(crate) verified: Marks,
}

pub(crate) fn load_verified() -> Verified {
    std::fs::read_to_string(verified_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_verified(v: &Verified) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(v).context("serialize verified")?;
    write_private(&verified_path(), &text).context("write verified")
}

/// Has the user verified this identity's fingerprint out-of-band?
pub fn is_verified(id_b32: &str) -> bool {
    load_verified().verified.contains_key(id_b32)
}

/// When this identity was verified, in unix seconds. `None` when it isn't
/// verified, or was before the stamp existed — the two are told apart by
/// [`is_verified`], and conflating them would be the difference between "you
/// never checked" and "you checked, we just don't know when".
pub fn verified_since(id_b32: &str) -> Option<u64> {
    load_verified()
        .verified
        .get(id_b32)
        .copied()
        .filter(|s| *s != 0)
}

/// Mark an identity verified, after the user compared its fingerprint
/// out-of-band. Takes a saved contact name **or** a raw base32 id, like
/// [`accept_name`] — the command that calls this shows the fingerprint for either,
/// so accepting only one of the two meant failing *after* the user had already
/// answered the confirmation. Returns the id that was marked.
pub fn mark_verified(who: &str) -> Result<String> {
    let id = resolve_recipient_id(who)?;
    let mut v = load_verified();
    v.verified.insert(id.clone(), marked_now());
    save_verified(&v)?;
    Ok(id)
}

/// Clear an identity's verified mark. `Ok(false)` means there was nothing to
/// clear — the caller needs that to avoid reporting a change it didn't make.
pub fn unmark_verified(who: &str) -> Result<bool> {
    let id = resolve_recipient_id(who)?;
    let mut v = load_verified();
    if v.verified.remove(&id).is_none() {
        return Ok(false);
    }
    save_verified(&v)?;
    Ok(true)
}

/// Warn, on stderr, when you're about to send to a key you never confirmed.
///
/// Deliberately **non-blocking**: a warning that stops the send is a warning that
/// gets switched off, and the risk here is real but not certain. It is also how a
/// key change surfaces at send time without keeping any extra state — a changed
/// key clears the verified mark ([`contact_add`]), so the contact simply *is*
/// unverified afterwards and this fires.
///
/// `who` is what the user typed; `id_b32` is the resolved identity.
pub fn warn_if_unverified(who: &str, id_b32: &str) {
    if is_verified(id_b32) {
        return;
    }
    let fp = fingerprint_of(id_b32).unwrap_or_default();
    if resolve_name(id_b32).is_some() {
        eprintln!("⚠  '{who}' is not verified — you have never confirmed this key is theirs.");
        eprintln!("   fingerprint: {fp}");
        eprintln!("   Compare it with them out-of-band, then: arvolo contacts verify {who}");
    } else {
        eprintln!("⚠  '{who}' is not in your contacts — sending to a key you've never confirmed.");
        eprintln!("   fingerprint: {fp}");
        eprintln!("   Save and verify: arvolo contacts add <name> {id_b32}");
    }
}
