//! Browser download-link path over a real relay: the relay serves the static
//! `/dl` page + script, and a deposited link container round-trips through the
//! public `/v1/fetch/{claim}` endpoint and decrypts byte-identically — exactly
//! what the in-browser decoder does, but exercised from Rust.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::link::{decode_key, decrypt_link, deposit_link};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

async fn spawn_relay() -> String {
    spawn_relay_with(true).await
}

async fn spawn_relay_with(links_enabled: bool) -> String {
    let dir = tempfile::tempdir().unwrap();
    let node = BlobNode::spawn(dir.path(), RelayChoice::Disabled)
        .await
        .expect("blob node");
    let mailbox = Arc::new(Mailbox::in_memory().expect("mailbox"));
    let mut state = AppState::new(mailbox, Arc::new(node));
    state.links_enabled = links_enabled;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _dir = dir;
        axum::serve(listener, router(state)).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn dl_page_and_script_are_served_from_the_relay() {
    let relay = spawn_relay().await;
    let c = reqwest::Client::new();

    let page = c.get(format!("{relay}/dl/anyclaim")).send().await.unwrap();
    assert!(page.status().is_success());
    let ct = page
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("text/html"), "page content-type: {ct}");
    assert!(
        page.headers().get("content-security-policy").is_some(),
        "download page must carry a CSP"
    );
    let body = page.text().await.unwrap();
    assert!(body.contains("arvolo"), "branded page");
    assert!(body.contains("/dl.js"), "page references its script");
    // The page ships English inline as the fallback and tags each string so the
    // script can swap it for the reader's language before first paint.
    assert!(
        body.contains("data-t=\"heading\"") && body.contains("data-html=\"footer\""),
        "page strings are tagged for translation"
    );

    let js = c.get(format!("{relay}/dl.js")).send().await.unwrap();
    assert!(js.status().is_success());
    let js_ct = js
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(js_ct.contains("javascript"), "script content-type: {js_ct}");
    // …and the script carries all four dictionaries, picked off the browser's
    // own language list. Nothing is fetched at runtime, so they ship together.
    let js_body = js.text().await.unwrap();
    assert!(
        js_body.contains("navigator.languages"),
        "browser-language pick"
    );
    for probe in [
        "Qualcuno ti ha mandato un file",
        "Quelqu'un vous a envoyé un fichier",
        "Jemand hat Ihnen eine Datei geschickt",
    ] {
        assert!(js_body.contains(probe), "missing translation: {probe}");
    }

    // The streaming service worker is served from the root with a root scope.
    let sw = c.get(format!("{relay}/arvolo-sw.js")).send().await.unwrap();
    assert!(sw.status().is_success());
    assert_eq!(
        sw.headers()
            .get("service-worker-allowed")
            .and_then(|v| v.to_str().ok()),
        Some("/"),
        "service worker must be allowed a root scope"
    );
    assert!(sw.text().await.unwrap().contains("ReadableStream"));
}

#[tokio::test]
async fn link_deposit_fetch_decrypt_roundtrip() {
    let relay = spawn_relay().await;

    // A payload spanning several 1 MiB chunks plus a partial tail.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("quarterly report.pdf");
    let bytes: Vec<u8> = (0..(1024 * 1024 * 2 + 4242u32))
        .map(|i| (i % 251) as u8)
        .collect();
    std::fs::write(&src, &bytes).unwrap();

    let out = deposit_link(&src, &relay, 3600, 1).await.expect("deposit");
    assert!(
        out.link.contains("/dl/"),
        "link points at the page: {}",
        out.link
    );
    assert_eq!(out.name, "quarterly report.pdf");
    assert_eq!(out.size, bytes.len() as u64);

    // Recover the key from the URL fragment (what the browser reads from `#`).
    let frag = out.link.rsplit_once('#').unwrap().1;
    let key = decode_key(frag).unwrap();

    // Fetch the ciphertext over the same public HTTP endpoint the browser uses.
    let blob = reqwest::get(format!("{relay}/v1/fetch/{}", out.claim))
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();

    // Decrypt exactly as the in-browser decoder would.
    let (name, data) = decrypt_link(&blob, &key).expect("decrypt");
    assert_eq!(name, "quarterly report.pdf");
    assert_eq!(data, bytes, "recovered bytes are identical");

    // Burn-after-read: a second fetch is gone (one-time link).
    let second = reqwest::get(format!("{relay}/v1/fetch/{}", out.claim))
        .await
        .unwrap();
    assert!(!second.status().is_success(), "one-time link is consumed");
}

#[tokio::test]
async fn links_can_be_disabled_by_the_relay() {
    use arvolo_core::link::{deposit_link, relay_allows_links};

    let relay = spawn_relay_with(false).await;
    let c = reqwest::Client::new();

    // Advertised as off, so a client can fail fast.
    assert!(!relay_allows_links(&relay).await.unwrap());
    let feats = c.get(format!("{relay}/v1/features")).send().await.unwrap();
    assert!(feats.text().await.unwrap().contains("\"links\":false"));

    // The download page is refused (403) rather than served.
    let page = c.get(format!("{relay}/dl/anyclaim")).send().await.unwrap();
    assert_eq!(page.status(), reqwest::StatusCode::FORBIDDEN);
    assert!(page
        .text()
        .await
        .unwrap()
        .to_lowercase()
        .contains("administrator"));

    // ...in the reader's language, which the 403 page can only get from the
    // request: it ships no script, so nothing translates it in the browser.
    for (accept, want) in [
        ("it-IT,it;q=0.9,en;q=0.8", "it"),
        ("de-AT", "de"),
        ("es-ES,fr;q=0.7", "fr"),
        ("ja,zh-CN;q=0.8", "en"),
    ] {
        let page = c
            .get(format!("{relay}/dl/anyclaim"))
            .header("accept-language", accept)
            .send()
            .await
            .unwrap();
        let body = page.text().await.unwrap();
        assert!(
            body.contains(&format!("<html lang=\"{want}\">")),
            "{accept:?} should render the page as {want}"
        );
        // All four translations ship; CSS reveals the negotiated one.
        assert!(body.contains("I link di download sono disattivati"));
        // An unsubstituted placeholder would match no `:lang()` rule. English
        // still shows (the CSS defaults to it), but the negotiation would be
        // silently dead, so catch it here rather than in a screenshot.
        assert!(
            !body.contains("{{"),
            "template placeholder left in the page"
        );
    }

    // A link deposit fails fast (before uploading) with the admin explanation.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("f.bin");
    std::fs::write(&src, b"data").unwrap();
    let err = deposit_link(&src, &relay, 3600, 1)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("administrator"), "clear admin message: {err}");

    // Defense in depth: even a raw HPKE-less deposit is refused at the relay.
    let raw = c
        .post(format!("{relay}/v1/deposit?ttl=3600&max=1"))
        .header("x-arvolo-encapped-key", "")
        .body(b"ciphertext".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status(), reqwest::StatusCode::FORBIDDEN);
}
