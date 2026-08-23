//! Which release this machine's relay can actually carry.
//!
//! `docs/DEPLOY.md` asks people to update the relay before the clients. That was
//! advice as long as updating meant downloading a file on purpose; once the app
//! offers the upgrade itself it becomes the app's job not to talk somebody into
//! the order the project asks them to avoid.
//!
//! So before offering a version, it is checked against what the relay says it
//! is, and if the newest release is too far ahead the newest one that *is*
//! carriable is offered instead.
//!
//! The table this reads is deliberately almost empty (see `compat.json` at the
//! repository root). A requirement in it means somebody checked and found a real
//! break; a guess would hold people back from a release they could have had, and
//! the release they are held back from might be the one with a security fix in
//! it. An empty table is not a broken feature — it is the honest state of the
//! world until a release actually needs a newer relay.

use serde::Deserialize;

/// The table as published beside `latest.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Compat {
    pub clients: Vec<ClientRelease>,
}

/// One released version, and what it needs under it.
#[derive(Debug, Clone, Deserialize)]
pub struct ClientRelease {
    pub version: String,
    /// The oldest relay that can carry it, or `None` for "no known requirement".
    #[serde(default)]
    pub needs_relay: Option<String>,
}

/// A version as three numbers, for comparison.
///
/// Prereleases are truncated to the release they lead up to: `0.13.0-rc1` sorts
/// as `0.13.0`. Coarse on purpose — the question here is only ever "is the relay
/// far enough along", and an rc of the relay that is one commit short of the
/// release is not a distinction worth stranding somebody over.
fn triple(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Whether a relay at `relay` is far enough along for a release needing `needs`.
fn relay_is_enough(relay: &str, needs: &str) -> bool {
    match (triple(relay), triple(needs)) {
        (Some(have), Some(want)) => have >= want,
        // A version neither side can parse is not evidence of anything. Refusing
        // to offer an update over an unreadable string would be a gate nobody
        // could diagnose; letting it through is what happened before this
        // existed at all.
        _ => true,
    }
}

/// What to offer, given the newest release and what the relay says it is.
///
/// `relay` is `None` when the relay could not be reached, or is old enough not
/// to report a version. Both mean *cannot check*, and both leave the offer
/// alone: the alternative is holding back every user whose relay predates the
/// version field — which, on the day this ships, is all of them.
///
/// Returns `None` when the newest release is fine, and `Some(version)` naming
/// the newest one that is not, otherwise.
pub fn held_back_to(compat: &Compat, latest: &str, relay: Option<&str>) -> Option<String> {
    let relay = relay?;
    if fits(compat, latest, relay) {
        return None;
    }
    // Walk down from the newest. The table is ordered newest-first and short —
    // it only holds versions that shipped an update manifest, since a version
    // that never published one cannot be installed as an update no matter how
    // compatible it is.
    let best = compat
        .clients
        .iter()
        .filter(|c| fits(compat, &c.version, relay))
        .max_by_key(|c| triple(&c.version))?;
    // Offering the version already running is not an offer. Saying "there is a
    // newer one but you cannot have it" is the UI's business, and it does that
    // from the absence of a candidate rather than from a candidate that goes
    // nowhere.
    (best.version != latest).then(|| best.version.clone())
}

/// Whether one version's requirement is met by this relay. A version absent from
/// the table has no known requirement, which is the same as `null` in it.
fn fits(compat: &Compat, version: &str, relay: &str) -> bool {
    compat
        .clients
        .iter()
        .find(|c| c.version == version)
        .and_then(|c| c.needs_relay.as_deref())
        .is_none_or(|needs| relay_is_enough(relay, needs))
}

/// Fetch the table published beside the release manifest.
///
/// Through the engine's own HTTP client, not a fresh one: that is what routes
/// this through `ARVOLO_PROXY` when somebody set it. A person who put arvolo
/// behind Tor did not agree to one bare request to github.com out the side.
pub async fn fetch(repository: &str) -> Option<Compat> {
    let url = format!("{repository}/releases/latest/download/compat.json");
    let resp = arvolo_core::http::client().get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<Compat>().await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[(&str, Option<&str>)]) -> Compat {
        Compat {
            clients: rows
                .iter()
                .map(|(v, n)| ClientRelease {
                    version: v.to_string(),
                    needs_relay: n.map(str::to_string),
                })
                .collect(),
        }
    }

    /// The state the world is actually in on the day this ships: a table with no
    /// requirements in it holds nobody back.
    #[test]
    fn an_empty_table_never_holds_anything_back() {
        let t = table(&[("0.13.0", None)]);
        assert_eq!(held_back_to(&t, "0.13.0", Some("0.9.0")), None);
    }

    /// The case the whole module exists for.
    #[test]
    fn a_relay_too_old_gets_the_newest_it_can_carry() {
        let t = table(&[
            ("0.15.0", Some("0.15.0")),
            ("0.14.0", Some("0.14.0")),
            ("0.13.0", None),
        ]);
        assert_eq!(
            held_back_to(&t, "0.15.0", Some("0.14.2")),
            Some("0.14.0".to_string())
        );
        assert_eq!(
            held_back_to(&t, "0.15.0", Some("0.13.0")),
            Some("0.13.0".to_string())
        );
        // A relay that kept up is offered the newest, with nothing in the way.
        assert_eq!(held_back_to(&t, "0.15.0", Some("0.15.0")), None);
    }

    /// A relay that could not be asked must not gate anything. On the day this
    /// ships every relay in the world predates the version field, so a `None`
    /// that held people back would hold back everyone, permanently.
    #[test]
    fn an_unknown_relay_gates_nothing() {
        let t = table(&[("0.15.0", Some("0.15.0")), ("0.13.0", None)]);
        assert_eq!(held_back_to(&t, "0.15.0", None), None);
    }

    /// Nothing carriable at all: the UI has to say "update your relay", and it
    /// learns that from there being no candidate rather than from being handed
    /// the version already installed.
    #[test]
    fn nothing_to_fall_back_to_is_not_an_offer() {
        let t = table(&[("0.15.0", Some("0.15.0"))]);
        assert_eq!(held_back_to(&t, "0.15.0", Some("0.9.0")), None);
    }

    #[test]
    fn a_prerelease_relay_counts_as_the_release_it_leads_to() {
        let t = table(&[("0.14.0", Some("0.14.0"))]);
        // 0.14.0-rc1 is one commit short of 0.14.0 and carries the same
        // protocol; stranding it would be pedantry with a cost.
        assert_eq!(held_back_to(&t, "0.14.0", Some("0.14.0-rc1")), None);
    }

    /// Garbage in the version field is not evidence that anything is wrong, and a
    /// gate nobody can diagnose is worse than no gate.
    #[test]
    fn an_unreadable_version_lets_the_update_through() {
        let t = table(&[("0.15.0", Some("0.15.0"))]);
        assert_eq!(held_back_to(&t, "0.15.0", Some("who knows")), None);
    }
}
