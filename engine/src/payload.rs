//! What a send actually reads from, once the user's several paths have become one
//! thing.

use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::flow;

use crate::book;

/// Resolve the send inputs to a single payload file: a lone file is sent as-is;
/// a folder or several paths are packed into a temp tar. Returns
/// `(payload, suggested_name, is_archive, temp_to_cleanup)`.
pub fn resolve_payload(paths: &[PathBuf]) -> Result<(PathBuf, String, bool, Option<PathBuf>)> {
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
