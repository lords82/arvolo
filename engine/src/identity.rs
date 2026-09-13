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

/// What is taken without asking.
///
/// A phone is the reason this is a setting rather than a rule. On a desktop the
/// window is usually open and a question costs a glance; a phone is in a pocket,
/// and the same question is a notification, a walk to the app and a tap before
/// anything moves. Some people want the file to be there when they look. Others
/// want the opposite even from people they trust, because a device you carry is
/// a device somebody else can be holding.
///
/// So the answer is the user's, and all three answers are legitimate:
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoAccept {
    /// Nothing is taken automatically. Every arrival is a question — including
    /// from a contact marked trusted, and from your own other devices. It is the
    /// strict reading of "ask me first", and it is the only one that survives
    /// somebody else holding an unlocked phone of yours.
    Never,
    /// Contacts you marked trusted, and your own devices. What every version
    /// before this setting did, and what a desktop still starts on — see
    /// [`AutoAccept::default_here`].
    Trusted,
    /// Everything, from anybody who can reach you. Convenient, and worth being
    /// plain about: a stranger who has your public id can then put a file on
    /// this device without being asked about.
    Everyone,
}

impl AutoAccept {
    /// Read the policy from the config; anything unrecognised reads as the
    /// default rather than as an error, since a typo in a file must not turn
    /// into a stricter *or* a looser rule than the one that was meant.
    pub fn configured() -> Self {
        match book::load_config().auto_accept.as_deref() {
            Some("never") | Some("ask") => AutoAccept::Never,
            Some("everyone") | Some("always") => AutoAccept::Everyone,
            Some("trusted") => AutoAccept::Trusted,
            None | Some(_) => Self::default_here(),
        }
    }

    /// What "nothing configured" means, which is not the same thing on a desk as
    /// in a pocket.
    ///
    /// A computer is somewhere you are sitting: a file that lands on it lands in
    /// a folder, in front of you, on a machine nobody else is holding. A phone is
    /// none of those. It travels, it is handed to people, and a file arriving on
    /// it without a question is a file on a device that may not be in your hands
    /// at that moment — and the question there costs a notification and a tap,
    /// which is little enough to be worth asking every time until somebody says
    /// otherwise.
    ///
    /// So the phone asks and the desktop takes what it trusts. Both are one row
    /// away from the other, and the row says what each one costs.
    fn default_here() -> Self {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            AutoAccept::Never
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            AutoAccept::Trusted
        }
    }

    /// The word for the config file and the wire. One spelling, both directions.
    pub fn as_str(self) -> &'static str {
        match self {
            AutoAccept::Never => "never",
            AutoAccept::Trusted => "trusted",
            AutoAccept::Everyone => "everyone",
        }
    }
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
    match AutoAccept::configured() {
        AutoAccept::Never => false,
        AutoAccept::Trusted => book::sender_status(id_b32).trusted || is_self(id_b32),
        AutoAccept::Everyone => true,
    }
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

    /// Three settings, three answers, and the middle one is what the app did
    /// before the setting existed — which is what makes it the default.
    #[test]
    fn the_policy_decides_who_is_taken_without_asking() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        std::env::remove_var("ARVOLO_IDENTITY");

        let me = encode_id(&my_identity().unwrap().public());
        let trusted = encode_id(&Identity::generate().public());
        book::contact_add("giulia", &trusted).unwrap();
        book::mark_verified("giulia").unwrap();
        book::mark_trusted("giulia").unwrap();
        let stranger = encode_id(&Identity::generate().public());

        let set = |v: &str| {
            book::set_config_value("auto_accept", Some(toml::Value::String(v.into()))).unwrap()
        };

        set("never");
        assert_eq!(AutoAccept::configured(), AutoAccept::Never);
        // Including our own devices: "ask me first" said strictly is the only
        // reading that holds when the phone is in somebody else's hand.
        assert!(!auto_accepts(&me));
        assert!(!auto_accepts(&trusted));

        set("trusted");
        assert_eq!(AutoAccept::configured(), AutoAccept::Trusted);
        assert!(auto_accepts(&me));
        assert!(auto_accepts(&trusted));
        assert!(!auto_accepts(&stranger));

        set("everyone");
        assert_eq!(AutoAccept::configured(), AutoAccept::Everyone);
        assert!(auto_accepts(&stranger), "which is the whole point of it");

        // A word nobody wrote on purpose must not silently loosen or tighten it.
        set("banana");
        assert_eq!(AutoAccept::configured(), AutoAccept::Trusted);

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
