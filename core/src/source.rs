//! Where a send reads its bytes from: a path, or a file **already open**.
//!
//! The path arm is the ordinary case. The handle arm exists because a path is
//! not always re-openable by the process doing the sending: macOS puts
//! Downloads/Desktop/Documents behind per-app consent, and a background daemon
//! is never shown the consent prompt — its `open()` gets a flat `Operation not
//! permitted`. Even `/dev/fd/N` re-opens are checked against the same policy
//! (measured, not assumed). What *does* work is using a descriptor some
//! entitled process — the CLI in the user's terminal — opened and handed over
//! (`arvolo-ipc`'s `fdpass`): a descriptor is a capability, and reads through
//! it never consult the path again.
//!
//! Everything here reads **positionally** (`read_at`/`seek_read`) or through a
//! private dup, so one shared handle serves concurrent chunk requests without
//! an offset race.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

/// A cloneable description of a send's byte source. `From<&Path>`/`From<PathBuf>`
/// keep every existing call site written in paths compiling unchanged.
#[derive(Clone)]
pub enum SendSource {
    /// Open the path afresh wherever bytes are needed — the historical behavior.
    Path(PathBuf),
    /// Read through this descriptor, wherever it came from. `label` carries the
    /// user-facing name the path would have provided (errors, link filenames).
    Handle {
        file: Arc<std::fs::File>,
        label: String,
    },
}

impl std::fmt::Debug for SendSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendSource::Path(p) => write!(f, "Path({})", p.display()),
            SendSource::Handle { label, .. } => write!(f, "Handle({label})"),
        }
    }
}

impl From<&Path> for SendSource {
    fn from(p: &Path) -> Self {
        SendSource::Path(p.to_path_buf())
    }
}

impl From<PathBuf> for SendSource {
    fn from(p: PathBuf) -> Self {
        SendSource::Path(p)
    }
}

impl From<&PathBuf> for SendSource {
    fn from(p: &PathBuf) -> Self {
        SendSource::Path(p.clone())
    }
}

impl SendSource {
    /// Wrap an already-open file. `label` is what error messages and download
    /// pages should call it — pass the original filename, since the handle has
    /// no path to derive one from.
    pub fn handle(file: std::fs::File, label: impl Into<String>) -> Self {
        SendSource::Handle {
            file: Arc::new(file),
            label: label.into(),
        }
    }

    /// What to call this source in messages: the path, or the handle's label.
    pub fn label(&self) -> String {
        match self {
            SendSource::Path(p) => p.display().to_string(),
            SendSource::Handle { label, .. } => label.clone(),
        }
    }

    /// Total size in bytes.
    pub fn len(&self) -> Result<u64> {
        match self {
            SendSource::Path(p) => Ok(std::fs::metadata(p)
                .with_context(|| format!("stat {}", p.display()))?
                .len()),
            SendSource::Handle { file, .. } => {
                Ok(file.metadata().context("stat handed-off file")?.len())
            }
        }
    }

    /// `len() == 0`, for clippy's sake and nobody else's.
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Fill `buf` from `offset` — reads until the buffer is full or EOF, and
    /// returns how many bytes landed. Positional on both arms: no shared file
    /// offset is disturbed, so concurrent chunk reads don't race.
    pub fn read_at_full(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
        match self {
            SendSource::Path(p) => {
                let f = std::fs::File::open(p)?;
                read_full_at(&f, buf, offset)
            }
            SendSource::Handle { file, .. } => read_full_at(file, buf, offset),
        }
    }

    /// A fresh sequential reader positioned at the start — for the one-pass
    /// hashing scan. For a handle this is a private dup (`try_clone`), rewound;
    /// dup(2) never consults path permissions.
    pub fn sequential_reader(&self) -> Result<Box<dyn Read + Send>> {
        match self {
            SendSource::Path(p) => Ok(Box::new(
                std::fs::File::open(p).with_context(|| format!("open {}", p.display()))?,
            )),
            SendSource::Handle { file, label } => {
                use std::io::Seek;
                let mut dup = file
                    .try_clone()
                    .with_context(|| format!("dup handle for {label}"))?;
                dup.seek(std::io::SeekFrom::Start(0))
                    .with_context(|| format!("rewind {label}"))?;
                Ok(Box::new(dup))
            }
        }
    }

    /// The whole payload in memory — the browser-link path encrypts in one go.
    pub fn read_all(&self) -> Result<Vec<u8>> {
        match self {
            SendSource::Path(p) => {
                std::fs::read(p).with_context(|| format!("read {}", p.display()))
            }
            SendSource::Handle { file, label } => {
                let len = file.metadata().map(|m| m.len() as usize).unwrap_or(0);
                let mut out = vec![0u8; len];
                let n = read_full_at(file, &mut out, 0).with_context(|| format!("read {label}"))?;
                out.truncate(n);
                Ok(out)
            }
        }
    }
}

/// Positional "fill or EOF" on any file, without touching its seek offset.
fn read_full_at(f: &std::fs::File, buf: &mut [u8], mut offset: u64) -> std::io::Result<usize> {
    let mut done = 0usize;
    while done < buf.len() {
        let n = read_at(f, &mut buf[done..], offset)?;
        if n == 0 {
            break;
        }
        done += n;
        offset += n as u64;
    }
    Ok(done)
}

#[cfg(unix)]
fn read_at(f: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, offset)
}

#[cfg(windows)]
fn read_at(f: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_handle_reads_positionally_and_survives_unlinking() {
        // `tempfile()` is already unlinked: there IS no path — the strongest
        // form of "the daemon could never have opened this itself".
        let mut f = tempfile::tempfile().unwrap();
        f.write_all(b"0123456789").unwrap();
        let src = SendSource::handle(f, "ten.bin");

        assert_eq!(src.len().unwrap(), 10);
        let mut buf = [0u8; 4];
        assert_eq!(src.read_at_full(&mut buf, 3).unwrap(), 4);
        assert_eq!(&buf, b"3456");
        // Positional reads left the shared offset alone: a sequential scan
        // still starts at zero.
        let mut all = String::new();
        src.sequential_reader()
            .unwrap()
            .read_to_string(&mut all)
            .unwrap();
        assert_eq!(all, "0123456789");
        assert_eq!(src.read_all().unwrap(), b"0123456789");
    }

    #[test]
    fn the_path_arm_still_reads_files_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.bin");
        std::fs::write(&p, b"by path").unwrap();
        let src: SendSource = p.as_path().into();
        assert_eq!(src.read_all().unwrap(), b"by path");
        let mut buf = [0u8; 2];
        assert_eq!(src.read_at_full(&mut buf, 3).unwrap(), 2);
        assert_eq!(&buf, b"pa");
    }
}
