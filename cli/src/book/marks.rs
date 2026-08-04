use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

/// When each marked identity was marked, in unix seconds.
///
/// The ledgers used to be a plain list of ids. They still *read* as one — see
/// [`de_marks`] — because dropping a verified mark on upgrade would silently undo
/// a security decision the user made deliberately; those simply come back with a
/// `0`, meaning "marked, but before we recorded when".
pub(crate) type Marks = BTreeMap<String, u64>;

/// Accept both shapes: the current table `id = <unix-seconds>` and the original
/// list of bare ids.
pub(crate) fn de_marks<'de, D: Deserializer<'de>>(d: D) -> Result<Marks, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Timed(BTreeMap<String, u64>),
        Ids(Vec<String>),
    }
    Ok(match Either::deserialize(d)? {
        Either::Timed(m) => m,
        Either::Ids(v) => v.into_iter().map(|id| (id, 0)).collect(),
    })
}

/// Unix seconds now — the stamp put on a mark as it is made.
pub(crate) fn marked_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How long ago a mark was made, e.g. "3d" — or `None` when the ledger predates
/// the stamp, so a UI can say "unknown" instead of inventing "just now".
pub(crate) fn marked_ago(since: u64) -> Option<u64> {
    if since == 0 {
        return None;
    }
    Some(marked_now().saturating_sub(since))
}
