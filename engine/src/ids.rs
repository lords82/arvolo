//! How a public identity is written down.
//!
//! One function, and it is here rather than in whichever caller needed it first
//! because every store that files something under an identity — deposits,
//! contacts, the device register — has to spell it the same way, and a second
//! spelling is a row nothing can find again.

use arvolo_core::crypto::PublicId;

/// The canonical text form of a public id: lowercase unpadded base32.
pub fn encode_id(p: &PublicId) -> String {
    data_encoding::BASE32_NOPAD
        .encode(&p.to_bytes())
        .to_lowercase()
}
