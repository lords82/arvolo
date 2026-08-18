//! The one visible handle shape: 8 lowercase hex chars, matched by unique
//! prefix.
//!
//! Everything a user can point at — a deposit, a resumable send, a waiting
//! offer — already derives (or here gains) the same 8-hex form, first 4 bytes
//! of a BLAKE3, so one rule covers every list: copy any unambiguous prefix of
//! what a listing shows. The long identifiers stay internal (a relay offer id
//! is 26 chars of base32 nobody should ever retype).

use arvolo_core::reexport::Hash;
use data_encoding::HEXLOWER;

/// The 8-hex handle for any internal identifier — same construction as
/// `deposits::id_for` / `sessions::id_for`, shared so every namespace hashes
/// alike.
pub(crate) fn short(full: &str) -> String {
    HEXLOWER.encode(&Hash::new(full.as_bytes()).as_bytes()[..4])
}

/// Could `input` be a handle (or a prefix of one)? Cheap shape check used to
/// decide whether prefix resolution is even worth attempting.
pub(crate) fn looks_like_handle(input: &str) -> bool {
    !input.is_empty()
        && input.len() <= 8
        && input
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// The outcome of matching a typed prefix against a set of handles.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Match<T> {
    /// Exactly one candidate starts with the prefix.
    One(T),
    /// More than one does — the handles, so the caller can list them.
    Many(Vec<String>),
    /// None does.
    None,
}

/// Resolve a typed prefix against `(handle, value)` candidates. Case-insensitive
/// on the input (handles are lowercase); an exact 8-char match wins immediately
/// even if it is also a prefix of nothing else.
pub(crate) fn resolve_prefix<T>(
    input: &str,
    candidates: impl IntoIterator<Item = (String, T)>,
) -> Match<T> {
    let want = input.to_ascii_lowercase();
    let mut hits: Vec<(String, T)> = candidates
        .into_iter()
        .filter(|(h, _)| h.starts_with(&want))
        .collect();
    match hits.len() {
        0 => Match::None,
        1 => Match::One(hits.remove(0).1),
        _ => Match::Many(hits.into_iter().map(|(h, _)| h).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_8_hex() {
        let h = short("4kt2m9xq7blf3wnd8vz5rc6ap0");
        assert_eq!(h.len(), 8);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        // Stable: same input, same handle.
        assert_eq!(h, short("4kt2m9xq7blf3wnd8vz5rc6ap0"));
    }

    #[test]
    fn prefix_resolution_is_unambiguous_or_says_so() {
        let cands = || {
            vec![
                ("8cd63bda".to_string(), 1),
                ("8c1200aa".to_string(), 2),
                ("73406183".to_string(), 3),
            ]
        };
        assert_eq!(resolve_prefix("8cd6", cands()), Match::One(1));
        assert_eq!(resolve_prefix("73", cands()), Match::One(3));
        assert_eq!(resolve_prefix("8C1200AA", cands()), Match::One(2));
        assert!(matches!(resolve_prefix("8c", cands()), Match::Many(v) if v.len() == 2));
        assert_eq!(resolve_prefix("ff", cands()), Match::None);
    }

    #[test]
    fn handle_shape_check() {
        assert!(looks_like_handle("8cd6"));
        assert!(looks_like_handle("8cd63bda"));
        assert!(!looks_like_handle("8cd63bda0")); // too long
        assert!(!looks_like_handle("arvcAAAA")); // not hex
        assert!(!looks_like_handle(""));
        assert!(!looks_like_handle("8CD6")); // handles are lowercase
    }
}
