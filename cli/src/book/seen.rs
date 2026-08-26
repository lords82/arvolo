use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::*;

/// What we know about a sender before recording this receipt: their contact name
/// (if saved), whether we've received from them before (TOFU), whether the user
/// has verified their identity out-of-band, and whether the user trusts them to
/// auto-download without a prompt.
pub struct SenderStatus {
    pub name: Option<String>,
    pub seen_before: bool,
    pub verified: bool,
    /// The standing "don't ask me about this person" from `contacts trust`. Read
    /// by the daemon's auto-download policy and by an inline `listen`, so it is
    /// consulted on every platform.
    pub trusted: bool,
}

/// The TOFU ledger: how many transfers have been taken from each key, and when
/// each key was last exchanged with.
///
/// The count came first and was deliberately alone — dating receipts feeds a
/// "last transfer" column, and a date kept only here would read differently on
/// each of your machines. That second objection is what changed: `SeenEntry` now
/// carries a `last` and merges it by `max`, exactly as it does the count, so the
/// answer converges instead of depending on which machine you look at. The
/// column earns its place by being the one thing an address book of forty people
/// is actually asked — who do I still deal with.
///
/// Two maps rather than one of pairs, because the file already exists on every
/// installation: a value that changed shape would fail to parse and take every
/// counter with it, silently. A missing `last` table simply reads as empty.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Seen {
    #[serde(default)]
    pub(crate) seen: BTreeMap<String, u64>,
    /// pubkey → unix seconds of the last completed transfer either way. Absent
    /// for a key last exchanged with before this was recorded, which reads as
    /// "not recorded" rather than as "never".
    #[serde(default)]
    pub(crate) last: BTreeMap<String, u64>,
}

pub(crate) fn load_seen() -> Seen {
    std::fs::read_to_string(seen_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_seen(s: &Seen) -> Result<()> {
    std::fs::create_dir_all(config_dir()).ok();
    let text = toml::to_string_pretty(s).context("serialize seen")?;
    write_private(&seen_path(), &text).context("write seen")
}

/// Contact name + whether this sender id has been seen before + verified + trusted.
/// Read-only.
pub fn sender_status(id_b32: &str) -> SenderStatus {
    SenderStatus {
        name: resolve_name(id_b32),
        seen_before: load_seen().seen.contains_key(id_b32),
        verified: is_verified(id_b32),
        trusted: is_trusted(id_b32),
    }
}

/// How many transfers have been taken from `id_b32`. `0` means never — the one
/// answer worth putting in front of the user, which is why this exists next to
/// [`sender_status`]'s `seen_before`: a count reads as "you know this key" in a
/// way a boolean does not.
pub fn seen_count(id_b32: &str) -> u64 {
    load_seen().seen.get(id_b32).copied().unwrap_or(0)
}

/// Record a receipt from `id_b32` (TOFU ledger): increments its counter. Best
/// effort — a failure to persist must not break a completed transfer.
pub fn record_seen(id_b32: &str) {
    let mut s = load_seen();
    *s.seen.entry(id_b32.to_string()).or_insert(0) += 1;
    if let Ok(text) = toml::to_string_pretty(&s) {
        std::fs::create_dir_all(config_dir()).ok();
        let _ = write_private(&seen_path(), &text);
    }
}

/// When a transfer with `id_b32` last completed, in unix seconds. `0` = never, or
/// before this was recorded.
pub fn last_exchange(id_b32: &str) -> u64 {
    load_seen().last.get(id_b32).copied().unwrap_or(0)
}

/// Stamp an exchange with `id_b32` as having just happened.
///
/// Both directions: what the address book is asked is "when did I last deal with
/// this person", and a file you sent them is as much of an answer as one they
/// sent you. Never moves backwards — the stamp is merged by `max` between your
/// devices, and a local write that went the other way would be undone on the next
/// round anyway, which is a confusing way to be wrong. Best effort, like the
/// counter beside it.
pub fn record_exchange(id_b32: &str, when: u64) {
    let mut s = load_seen();
    let e = s.last.entry(id_b32.to_string()).or_insert(0);
    if when <= *e {
        return;
    }
    *e = when;
    if let Ok(text) = toml::to_string_pretty(&s) {
        std::fs::create_dir_all(config_dir()).ok();
        let _ = write_private(&seen_path(), &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stamp under a name in the address book.
    ///
    /// Two properties, and the second is the one worth a test: it never moves
    /// backwards. The value is merged between devices by `max`, so a local write
    /// that went the other way would be undone on the next sync round — a row
    /// that changes its mind about the past, once, and then changes back.
    #[test]
    fn an_exchange_stamp_only_ever_moves_forward() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let id = "someidb32";
        assert_eq!(last_exchange(id), 0, "never dealt with = 0, not 'now'");

        record_exchange(id, 1_700_000_000);
        assert_eq!(last_exchange(id), 1_700_000_000);

        record_exchange(id, 1_600_000_000);
        assert_eq!(
            last_exchange(id),
            1_700_000_000,
            "an older exchange does not overwrite a newer one"
        );

        record_exchange(id, 1_800_000_000);
        assert_eq!(last_exchange(id), 1_800_000_000);

        // And it lives beside the counter without disturbing it: the file already
        // exists on every installation, so a shape that lost the counts would take
        // the whole TOFU ledger with it.
        record_seen(id);
        assert_eq!(seen_count(id), 1);
        assert_eq!(last_exchange(id), 1_800_000_000);

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// A `seen.toml` written before the stamp existed must still read — with its
    /// counters intact and no date, rather than as an empty ledger.
    #[test]
    fn a_ledger_from_before_the_stamp_still_loads() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        std::fs::write(seen_path(), "[seen]\noldkey = 4\n").unwrap();

        assert_eq!(
            seen_count("oldkey"),
            4,
            "the counters survive the new field"
        );
        assert_eq!(last_exchange("oldkey"), 0, "and there is simply no date");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
