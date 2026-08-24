//! The browser decryptor bundle, as assembled by `build.rs`.
//!
//! One place produces these bytes and two places consume them: the HTTP routes
//! that serve `/dl/{claim}` for a relay's own visitors, and `--print-decryptor`,
//! which writes the same bundle out for publication on a static origin. That is
//! the whole anti-drift design — a relay and the canonical page cannot serve
//! different code, because there is only one artifact and it is compiled in.
//!
//! Why publish it at all: on the split-origin link the page comes from an origin
//! that stores no ciphertext, so a relay operator can never run code in a context
//! that holds the key. See `docs/PROTOCOL.md` §7.6.

use std::path::Path;

/// The download page: one self-contained HTML file, script inlined, no
/// subresources. Served for any claim — the claim is read by the script from
/// the URL, never substituted here.
pub const PAGE: &str = include_str!(concat!(env!("OUT_DIR"), "/dl_page.html"));

/// The streaming download service worker. Must stay a separate URL (a worker
/// cannot be inlined), and handles decrypted plaintext, which is why the
/// manifest hashes it.
pub const SERVICE_WORKER: &str = include_str!(concat!(env!("OUT_DIR"), "/arvolo-sw.js"));

/// The un-inlined script, still served at `/dl.js` for one release so a page
/// cached before 0.14 keeps working. Removable in 0.15.
pub const SCRIPT: &str = include_str!(concat!(env!("OUT_DIR"), "/dl_script.js"));

/// The Content-Security-Policy for [`PAGE`], byte-identical to the one baked
/// into its `<meta>`: `build.rs` writes both from one string.
pub const CSP: &str = include_str!(concat!(env!("OUT_DIR"), "/dl_csp.txt"));

/// SHA-256 of the two files a browser runs, plus the version that produced them.
/// `sha256sum -c` reads it as-is.
pub const MANIFEST: &str = include_str!(concat!(env!("OUT_DIR"), "/manifest.txt"));

/// Write the publishable bundle into `dir`, creating it if needed.
///
/// This is what CI uploads to the canonical origin and what a self-hoster runs
/// to put the page on a static host of their own. The bytes are the ones this
/// binary serves, so `arvolo verify-page` against either deployment compares the
/// same numbers.
pub fn print_bundle(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, body) in [
        ("index.html", PAGE),
        ("arvolo-sw.js", SERVICE_WORKER),
        ("manifest.txt", MANIFEST),
    ] {
        std::fs::write(dir.join(name), body)?;
    }
    Ok(())
}
