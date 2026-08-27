use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::crypto::{Identity, PublicId};
use arvolo_core::flow::{self};

use crate::book;

use crate::output::vprintln;

/// Resolve the send inputs to a single payload file: a lone file is sent as-is;
/// a folder or several paths are packed into a temp tar. Returns
/// `(payload, suggested_name, is_archive, temp_to_cleanup)`.
pub(crate) fn resolve_payload(
    paths: &[PathBuf],
) -> Result<(PathBuf, String, bool, Option<PathBuf>)> {
    if paths.len() == 1 && paths[0].is_file() {
        let name = paths[0]
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        return Ok((paths[0].clone(), name, false, None));
    }
    for p in paths {
        anyhow::ensure!(p.exists(), "{} does not exist", p.display());
    }
    let name = if paths.len() == 1 {
        paths[0]
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "bundle".into())
    } else {
        "bundle".into()
    };
    let temp = book::temp_dir().join(format!("arvolo-send-{}.tar", std::process::id()));
    flow::pack_tar(paths, &temp).context("pack archive")?;
    Ok((temp.clone(), name, true, Some(temp)))
}

// ---- presence: stay-online receive (listen) and push-to-contact -----------

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

/// Current unix time in seconds.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

pub(crate) fn identity_path() -> PathBuf {
    if let Ok(p) = std::env::var("ARVOLO_IDENTITY") {
        return PathBuf::from(p);
    }
    crate::book::config_dir().join("identity.key")
}

pub(crate) fn my_identity() -> Result<Identity> {
    Identity::load_or_create(&identity_path()).context("load identity")
}

/// The reserved `--to` word for your own devices: `arvolo send file --to me`.
///
/// It beats a saved contact of the same name, and that precedence is chosen for
/// the direction it fails in. Read as the reserved word when a contact was meant,
/// the file is sealed to you and simply does not reach them — annoying, nothing
/// leaks. The other way round it would be sealed to *another person* when you
/// meant yourself, which is a disclosure. So the reserved word wins, and
/// [`crate::commands::send`] says so on stderr when the collision exists.
pub(crate) const SELF_RECIPIENT: &str = "me";

/// Resolve a `--to` argument the way `send` does: [`SELF_RECIPIENT`] first, then
/// the address book.
///
/// The precedence lives here rather than in `book` because only this layer knows
/// the identity, and it lives *at all* because two clients need the same answer.
/// `arvolo send --to me` resolved the word before the daemon ever saw it
/// ([`crate::commands::send::send_to`]), so a front-end that talks to the daemon
/// directly — the desktop app — asked for a contact called "me" and got an error.
/// One rule, both callers.
///
/// `None` means "wherever we listen", which is what every send did before a
/// contact could carry a relay. Our own devices resolve to `None` by
/// construction: they share our identity, so they read the inbox we do.
pub(crate) fn resolve_recipient_or_self_with_relay(
    arg: &str,
) -> Result<(PublicId, Option<String>)> {
    if arg == SELF_RECIPIENT {
        return Ok((my_identity()?.public(), None));
    }
    crate::book::resolve_recipient_with_relay(arg)
}

/// Is this base32 public id our own identity? Used to decide that an incoming
/// offer came from another of our devices — the only sender we auto-accept
/// without being asked to trust them.
pub(crate) fn is_self(id_b32: &str) -> bool {
    my_identity()
        .map(|me| encode_id(&me.public()) == id_b32)
        .unwrap_or(false)
}

/// Is an offer from this sender taken without asking?
///
/// Two senders qualify: a contact the user marked trusted, and another of our own
/// devices — the second because there is nobody there to be asked to trust, only
/// the user on the other side of the same desk.
///
/// One predicate rather than two matching expressions, because both ends of the
/// socket need the same answer: the daemon *decides* with it, and the front-end is
/// *told* with it (`stamp_offer` fills [`OfferReceived::auto`] from here). While
/// they were written out separately they disagreed on our own devices, and a
/// window that asks a question the daemon has already answered is exactly what
/// that disagreement looks like from a chair.
///
/// [`OfferReceived::auto`]: arvolo_ipc::protocol::EventDto
pub(crate) fn auto_accepts(id_b32: &str) -> bool {
    crate::book::sender_status(id_b32).trusted || is_self(id_b32)
}

/// How a public id is written down, now that more than one crate has to spell it
/// the same way. Re-exported rather than moved-and-renamed: `util::encode_id` is
/// what this crate has always called it.
pub(crate) use arvolo_engine::ids::encode_id;

#[cfg(test)]
mod tests {
    use super::*;

    /// The question the daemon answers by acting and the front-end is answered by
    /// being told. It is pinned because the two used to be written out separately
    /// and disagreed here: our own devices were downloaded automatically *and*
    /// asked about, so the window offered a button for an offer the daemon had
    /// already consumed, and pressing it produced an error next to a file that was
    /// arriving perfectly well.
    #[test]
    fn auto_accept_covers_trusted_contacts_and_our_own_devices() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        // Or the identity would be read from whoever is running the test.
        std::env::remove_var("ARVOLO_IDENTITY");

        let me = encode_id(&my_identity().unwrap().public());
        assert!(
            auto_accepts(&me),
            "another of our own devices is taken without asking"
        );

        let stranger = encode_id(&Identity::generate().public());
        assert!(!auto_accepts(&stranger), "a stranger is always a question");

        // Saved is not trusted: knowing a name is not a standing decision to take
        // whatever it sends.
        let known = encode_id(&Identity::generate().public());
        book::contact_add("giulia", &known).unwrap();
        assert!(!auto_accepts(&known));

        book::mark_verified("giulia").unwrap();
        book::mark_trusted("giulia").unwrap();
        assert!(auto_accepts(&known), "trust is what the flag is made of");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// The rule `send --to` and the daemon's `Push` now share. It is worth a test
    /// of its own because the failure it prevents is silent: before, the desktop
    /// app's "your devices" would have resolved against the address book, found
    /// nothing, and reported a missing contact for a destination that is always
    /// reachable.
    #[test]
    fn the_reserved_word_resolves_to_our_own_identity() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        // Or the identity would be read from whoever is running the test.
        std::env::remove_var("ARVOLO_IDENTITY");

        let me = my_identity().unwrap().public();
        assert_eq!(
            encode_id(
                &resolve_recipient_or_self_with_relay(SELF_RECIPIENT)
                    .unwrap()
                    .0
            ),
            encode_id(&me)
        );

        // An ordinary contact is untouched by the new branch, by name and by id.
        let other = Identity::generate().public();
        book::contact_add("proj", &encode_id(&other)).unwrap();
        assert_eq!(
            encode_id(&resolve_recipient_or_self_with_relay("proj").unwrap().0),
            encode_id(&other)
        );
        assert_eq!(
            encode_id(
                &resolve_recipient_or_self_with_relay(&encode_id(&other))
                    .unwrap()
                    .0
            ),
            encode_id(&other)
        );

        // And the word wins over a contact that happens to carry it — the
        // precedence documented on `SELF_RECIPIENT`, which fails towards sealing
        // a file to ourselves rather than to somebody else.
        book::contact_add(SELF_RECIPIENT, &encode_id(&other)).unwrap();
        assert_eq!(
            encode_id(
                &resolve_recipient_or_self_with_relay(SELF_RECIPIENT)
                    .unwrap()
                    .0
            ),
            encode_id(&me)
        );

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
