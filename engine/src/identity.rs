//! This installation's own identity, and the questions only something holding it
//! can answer.
//!
//! `arvolo-core` mints and loads identities but has no opinion about *where* this
//! machine keeps its key or which of the ids it meets is its own. That is local
//! knowledge, and both clients need the same answers out of it: while the CLI and
//! the daemon each wrote their own, they disagreed about our own devices, and a
//! window asking a question the daemon had already decided is exactly what that
//! disagreement looked like from a chair.

use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::crypto::{Identity, PublicId};

use crate::book;

/// The canonical text form of a public id: lowercase unpadded base32.
///
/// Here rather than in whichever caller needed it first because every store that
/// files something under an identity — deposits, contacts, the device register —
/// has to spell it the same way, and a second spelling is a row nothing can find
/// again.
pub fn encode_id(p: &PublicId) -> String {
    data_encoding::BASE32_NOPAD
        .encode(&p.to_bytes())
        .to_lowercase()
}

/// Where this machine's identity key lives: `ARVOLO_IDENTITY` when set, else
/// beside the config.
pub fn identity_path() -> PathBuf {
    if let Ok(p) = std::env::var("ARVOLO_IDENTITY") {
        return PathBuf::from(p);
    }
    book::config_dir().join("identity.key")
}

/// This installation's identity, minted on first use.
pub fn my_identity() -> Result<Identity> {
    Identity::load_or_create(&identity_path()).context("load identity")
}

/// The reserved `--to` word for your own devices: `arvolo send file --to me`.
///
/// It beats a saved contact of the same name, and that precedence is chosen for
/// the direction it fails in. Read as the reserved word when a contact was meant,
/// the file is sealed to you and simply does not reach them — annoying, nothing
/// leaks. The other way round it would be sealed to *another person* when you
/// meant yourself, which is a disclosure. So the reserved word wins, and `send`
/// says so on stderr when the collision exists.
pub const SELF_RECIPIENT: &str = "me";

/// Resolve a `--to` argument the way `send` does: [`SELF_RECIPIENT`] first, then
/// the address book.
///
/// The precedence lives here rather than in `book` because only this layer knows
/// the identity, and it lives *at all* because two clients need the same answer.
/// `arvolo send --to me` resolved the word before the daemon ever saw it, so a
/// front-end that talks to the daemon directly — the desktop app — asked for a
/// contact called "me" and got an error. One rule, both callers.
///
/// `None` means "wherever we listen", which is what every send did before a
/// contact could carry a relay. Our own devices resolve to `None` by
/// construction: they share our identity, so they read the inbox we do.
pub fn resolve_recipient_or_self_with_relay(arg: &str) -> Result<(PublicId, Option<String>)> {
    if arg == SELF_RECIPIENT {
        return Ok((my_identity()?.public(), None));
    }
    book::resolve_recipient_with_relay(arg)
}

/// Is this base32 public id our own identity? Used to decide that an incoming
/// offer came from another of our devices — the only sender we auto-accept
/// without being asked to trust them.
pub fn is_self(id_b32: &str) -> bool {
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
/// *told* with it (`stamp_offer` fills `OfferReceived::auto` from here).
pub fn auto_accepts(id_b32: &str) -> bool {
    book::sender_status(id_b32).trusted || is_self(id_b32)
}

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
