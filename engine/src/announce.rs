//! The handful of things the engine has to tell somebody, and no idea how.
//!
//! A daemon with nothing attached to it still has to speak: a file is arriving
//! from a trusted contact, an offer is waiting for an answer, an automatic take
//! failed and there is nothing left to accept. Where those go is not the engine's
//! business — a terminal prints them, a desktop raises a notification, a phone
//! asks the system to. Each of those is also a dependency the others must not
//! carry: `notify-rust` speaks D-Bus and WinRT, which is not something an app on
//! a phone should be linking.
//!
//! So the engine states the event and the embedder decides what happens to it.
//! Whether an attached front-end makes the announcement redundant is the
//! embedder's call too — it is the one that knows what is attached.

/// Who a file is from, as far as an announcement is concerned.
pub struct Sender<'a> {
    /// Another device of this same identity, rather than another person. Not a
    /// contact to be trusted or not: the user, on the other side of the same desk
    /// — which is a different sentence, in every language.
    pub own_device: bool,
    /// What this installation files them by: their contact name, or their id when
    /// there is no name. Filled for our own devices too, where it is the id — a
    /// message that has to name them anyway should not have to invent one.
    pub who: &'a str,
}

/// What the engine says when nobody asked it anything.
///
/// Every method has a default that does nothing, so an embedder implements only
/// what it wants to surface — and so a test can pass [`Silent`].
pub trait Announce: Send + Sync {
    /// A file from a trusted contact, or from one of our own devices, is being
    /// taken without asking. `size` is bytes.
    fn auto_downloading(&self, _name: &str, _from: Sender<'_>, _size: u64) {}

    /// The automatic take was refused and the offer is going back in front of the
    /// user: the deposit wants a password, P2P is off for a direct transfer.
    /// Not a failure — a question, after all.
    fn auto_declined(&self, _why: &str) {}

    /// The automatic take failed after the offer was consumed. There is nothing
    /// left to accept, so nothing will be asked: this is the only word the user
    /// gets that the file did not arrive.
    fn auto_download_failed(&self, _name: &str, _from: Sender<'_>, _why: &str) {}

    /// An offer is parked, waiting for an answer. `handle` is the short id the
    /// user can name it by.
    fn offer_waiting(&self, _name: &str, _from: Sender<'_>, _size: u64, _handle: &str) {}

    /// A receive finished and the file is at `where_it_is` — a path, or whatever
    /// the platform's publisher answered with.
    fn saved(&self, _where_it_is: &str) {}
}

/// An embedder with nothing to say. Useful in tests, and honest in a shell that
/// draws all of this itself.
pub struct Silent;

impl Announce for Silent {}
