//! Where a finished download comes to rest, when that is the platform's call and
//! not the filesystem's.
//!
//! On a desktop the two are the same thing: the download is written straight into
//! the user's folder and it is already where it belongs. A phone does not work
//! that way. A receive here writes **in place** — the output file is pre-allocated
//! ([`set_len`](std::fs::File::set_len)) and each verified piece is written at its
//! own offset, with a `.arvticket` sidecar beside the partial so an interrupted
//! download can resume. None of that is possible inside Android's MediaStore or a
//! SAF tree: there is no path, no directory to list for the `name (1).ext` dance,
//! and no file at all until the handle is closed.
//!
//! So on mobile the download lands in the app's own storage and is **published**
//! once it is complete and verified — which is also the better promise: nothing
//! appears in a person's Downloads until it is whole.
//!
//! That publication is the embedder's business, not the engine's. The engine
//! offers this seam and calls it exactly once per finished receive; a manager
//! without a publisher behaves as it always has, byte for byte.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Where a completed receive came to rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavedLocation {
    /// A path in the filesystem — every desktop, and iOS, where publishing is a
    /// rename into the app's `Documents` folder (same volume, no copy).
    Path(PathBuf),
    /// An opaque handle from the platform's own store. Android's MediaStore hands
    /// back a `content://` URI and no path at all: the file is real, indexed and
    /// visible to every other app, and unreachable by `open()`.
    Uri(String),
}

impl SavedLocation {
    /// The filesystem path, when there is one.
    ///
    /// `None` is not "missing": it says the file exists and cannot be opened by
    /// path, so whoever wants its bytes — seeding it back to the swarm, re-sending
    /// it — has to ask the platform for a descriptor and read through that. The
    /// engine already knows how: see [`SendSource::Handle`](crate::source::SendSource).
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            SavedLocation::Path(p) => Some(p),
            SavedLocation::Uri(_) => None,
        }
    }

    /// The URI, when the platform gave one instead of a path.
    pub fn as_uri(&self) -> Option<&str> {
        match self {
            SavedLocation::Uri(u) => Some(u),
            SavedLocation::Path(_) => None,
        }
    }
}

impl From<PathBuf> for SavedLocation {
    fn from(p: PathBuf) -> Self {
        SavedLocation::Path(p)
    }
}

/// The platform's say in where a finished download lives.
///
/// Implemented by the embedder — on mobile by a thin native plugin (MediaStore on
/// Android, a rename into `Documents` on iOS), and by nobody at all on the
/// desktop, where [`None`] leaves the engine's behaviour untouched.
pub trait SavePublisher: Send + Sync {
    /// Move `staged` out of the app's storage and into wherever this platform
    /// keeps a person's files, under `name`.
    ///
    /// `staged` is usually a file. It is a **directory** when the payload was an
    /// archive, which arrives unpacked — a publisher that can only place files
    /// walks it and returns the destination of the folder it created.
    ///
    /// Called once, and only for a download that finished whole. Returning an
    /// error is not fatal to the transfer: the file stays in staging and the row
    /// says so, because a 250 MB download is not worth throwing away over a full
    /// public volume.
    fn publish(&self, staged: &Path, name: &str) -> Result<SavedLocation>;
}
