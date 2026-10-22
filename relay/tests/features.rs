//! The features document, from both ends.
//!
//! `/v1/features` is written by hand in the relay and read by hand in the client
//! — three separate substring matches, no JSON parser on either side of the
//! wire. That is a deliberate trade (see `features_handler`), and its cost is
//! that nothing in the type system connects the two. This is what connects them:
//! a real relay, answering a real probe, checked against what it actually is.
//!
//! Without it, adding a field to one side and reading it wrongly on the other is
//! a mistake that compiles, passes every unit test, and shows up as the desktop
//! app quietly never gating an update.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

async fn spawn_relay() -> String {
    let dir = tempfile::tempdir().unwrap();
    let node = BlobNode::spawn(dir.path(), RelayChoice::Disabled)
        .await
        .expect("blob node");
    let mailbox = Arc::new(Mailbox::in_memory().expect("mailbox"));
    let state = AppState::new(mailbox, Arc::new(node));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _dir = dir;
        axum::serve(listener, router(state)).await.unwrap();
    });
    format!("http://{addr}")
}

/// The version the relay reports is the version the relay is, and the client
/// probe reads back exactly that.
#[tokio::test]
async fn a_relay_reports_its_own_version_and_the_probe_reads_it() {
    let relay = spawn_relay().await;
    let probe = arvolo_core::code::relay_probe(&relay).await;

    assert!(probe.reachable, "a relay that is up must read as reachable");
    assert_eq!(
        probe.version.as_deref(),
        Some(arvolo_core::VERSION),
        "the probe must read back what this relay actually is"
    );
}

/// A relay that is not there is not an old relay.
///
/// The desktop app leans on this distinction: unreachable means "could not
/// check" and gates nothing, while a reachable relay with no version means "too
/// old to say" — and only one of the two is a fact about the relay's age. A
/// probe that collapsed them would gate every laptop on a train.
#[tokio::test]
async fn an_unreachable_relay_is_not_mistaken_for_an_old_one() {
    // A port nothing is listening on: bind one, learn its number, drop it.
    let addr = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap()
    };
    let probe = arvolo_core::code::relay_probe(&format!("http://{addr}")).await;

    assert!(!probe.reachable);
    assert_eq!(probe.version, None);
}

/// The two older probes read the same document, and each has its own failure
/// direction. Adding a field must not have moved either of them: `rz2` fails
/// closed (a missing flag means v1, so a code is never minted that no relay can
/// host) and `links` fails open.
#[tokio::test]
async fn the_older_probes_still_read_the_same_document() {
    use arvolo_core::code::{relay_rz_version, RzVersion};
    use arvolo_core::link::relay_allows_links;

    let relay = spawn_relay().await;

    assert_eq!(relay_rz_version(&relay).await, RzVersion::V2);
    assert!(relay_allows_links(&relay).await.unwrap());
}

/// A relay that has the cross-origin headers says so, so a client can mint a
/// link whose decryptor is not the relay. The matching "an older relay does not,
/// and the client must read that as no" case is a unit test in core against a
/// frozen literal document — it has to be, because the risk is precisely that
/// relay and client change together and nobody notices the old shape stopped
/// being handled.
#[tokio::test]
async fn a_relay_that_serves_ciphertext_cross_origin_advertises_it() {
    let relay = spawn_relay().await;
    let doc = reqwest::get(format!("{relay}/v1/features"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        doc.replace(' ', "").contains("\"cors\":true"),
        "features document must advertise cors: {doc}"
    );
}

/// The cap a deposit is held to is the cap the relay advertises, and the 413
/// that enforces it names the same figure in a header — so a sender can refuse
/// a file before uploading it, and say how large is too large after.
///
/// Set through the environment like the other cap tests, and read back through
/// the client's own probe: the point is the number surviving the hand-written
/// document and the hand-written parse, not the arithmetic.
#[tokio::test]
async fn the_advertised_blob_cap_is_the_one_a_deposit_is_held_to() {
    std::env::set_var("ARVOLO_MAX_BLOB_BYTES", "64");
    let relay = spawn_relay().await;

    assert_eq!(
        arvolo_core::link::relay_max_blob_bytes(&relay).await,
        Some(64),
        "the probe must read the cap the relay was given"
    );

    let resp = reqwest::Client::new()
        .post(format!("{relay}/v1/deposit?ttl=60"))
        .header("x-arvolo-encapped-key", "")
        .body(vec![0u8; 65])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        resp.headers()
            .get("x-arvolo-max-blob-bytes")
            .and_then(|v| v.to_str().ok()),
        Some("64"),
        "the 413 must carry the cap"
    );

    // Exactly the cap still fits: the bound is "larger than", not "as large as".
    let resp = reqwest::Client::new()
        .post(format!("{relay}/v1/deposit?ttl=60"))
        .header("x-arvolo-encapped-key", "")
        .body(vec![0u8; 64])
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "{}", resp.status());

    // A relay with no per-blob cap says so with a zero, and the probe reads that
    // as "no limit" rather than as a limit of nothing. Same test, not a second
    // one: the cap is process-wide, and two tests setting it would race.
    std::env::set_var("ARVOLO_MAX_BLOB_BYTES", "0");
    let doc = reqwest::get(format!("{relay}/v1/features"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        doc.replace(' ', "").contains("\"max_blob_bytes\":0"),
        "features document must advertise the cap: {doc}"
    );
    assert_eq!(arvolo_core::link::relay_max_blob_bytes(&relay).await, None);
    std::env::remove_var("ARVOLO_MAX_BLOB_BYTES");
}
