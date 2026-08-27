//! Builds the browser decryptor bundle from `web/decryptor/src/`.
//!
//! The page is assembled here, once, and the result is used for two different
//! things: the relay embeds it (`/dl/{claim}`), and `arvolo-relay
//! --print-decryptor` writes it out for publication to a static host. Both come
//! from this function, so the copy a self-hoster serves and the copy on the
//! canonical origin cannot drift — not "are checked for drift", cannot drift.
//!
//! Two things happen to the source:
//!
//! 1. **`dl.js` is inlined** into the page. One file means one artifact to hash,
//!    which is what makes the published bundle checkable (`arvolo verify-page`)
//!    with a single number rather than a set. It also removes the last
//!    subresource, so the page has nothing left to fetch before it can run.
//! 2. **The CSP is generated** with that inline script's SHA-256 and written
//!    both into the page's `<meta>` and to a file the server sends as a header.
//!    `script-src` names the hash and nothing else — not even `'self'` — so no
//!    second script can run on this origin, whatever else is served from it.
//!
//! No JS toolchain, no minifier: the page ships as it is written. "Reproducible
//! from a tag" and "readable by whoever wants to check" are then the same
//! property, and a contributor with no node installed can still change the page.

use std::path::{Path, PathBuf};

/// Where the source lives, relative to this crate.
const SRC: &str = "../web/decryptor/src";

/// The page's Content-Security-Policy, with `{hash}` filled in with the inline
/// script's SHA-256.
///
/// `connect-src 'self'` is what a relay-served copy needs; the split-origin
/// deployment widens it to reach a sender-chosen relay. `worker-src`/`frame-src`
/// are for the streaming service worker and the hidden iframe it drives.
fn csp(script_sha256_b64: &str) -> String {
    format!(
        "default-src 'none'; script-src 'sha256-{script_sha256_b64}'; \
         style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; \
         worker-src 'self'; frame-src 'self'; frame-ancestors 'none'; \
         base-uri 'none'; form-action 'none'"
    )
}

fn read(dir: &Path, name: &str) -> String {
    let path = dir.join(name);
    println!("cargo:rerun-if-changed={}", path.display());
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn write(out: &Path, name: &str, body: &str) {
    let path = out.join(name);
    std::fs::write(&path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    let src = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join(SRC);
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let html = read(&src, "index.html");
    let script = read(&src, "dl.js");
    let sw = read(&src, "arvolo-sw.js");

    // An inline script containing `</script>` would end the element early — the
    // browser would then run a prefix of the file and render the rest as text,
    // silently. Nothing in dl.js does this today; fail the build if it ever does
    // rather than ship a page that half-works.
    assert!(
        !script.to_ascii_lowercase().contains("</script"),
        "dl.js contains `</script`, which cannot be inlined — \
         write it as `<\\/script` inside a string, or keep it out of the file"
    );

    // The hash must cover exactly the bytes between the tags, so the script is
    // inlined with no surrounding whitespace of its own.
    let digest = {
        use sha2::Digest;
        sha2::Sha256::digest(script.as_bytes())
    };
    let policy = csp(&data_encoding::BASE64.encode(&digest));

    const TAG: &str = r#"<script src="/dl.js"></script>"#;
    assert!(
        html.contains(TAG),
        "index.html no longer contains `{TAG}` — the inlining step has nothing to replace"
    );
    let page = html
        .replace(TAG, &format!("<script>{script}</script>"))
        .replace("{{CSP}}", &policy);
    assert!(
        !page.contains("{{CSP}}") && !page.contains("<script src="),
        "index.html must carry the CSP placeholder and no remaining subresource"
    );

    // The manifest covers the two files a browser actually runs. The service
    // worker cannot be inlined — a worker must be its own URL — and it handles
    // *decrypted* plaintext on its way to disk, so leaving it out of the hashes
    // would be the one gap in an otherwise checkable bundle.
    let manifest = format!(
        "arvolo-decryptor {}\n{}  index.html\n{}  arvolo-sw.js\n",
        std::env::var("CARGO_PKG_VERSION").unwrap(),
        sha256_hex(&page),
        sha256_hex(&sw),
    );

    write(&out, "dl_page.html", &page);
    write(&out, "dl_csp.txt", &policy);
    // Still served at `/dl.js` for one release, for a page cached before 0.14.
    write(&out, "dl_script.js", &script);
    write(&out, "arvolo-sw.js", &sw);
    write(&out, "manifest.txt", &manifest);
}

fn sha256_hex(body: &str) -> String {
    use sha2::Digest;
    data_encoding::HEXLOWER.encode(&sha2::Sha256::digest(body.as_bytes()))
}
