//! End-to-end short-code pairing over a real relay server: the sender publishes
//! a ticket under a code, the receiver resolves the code back to the same ticket.

use std::sync::Arc;

use arvolo_core::backfill::BlobNode;
use arvolo_core::code::{publish_ticket, resolve_code};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};

/// Spawn the relay HTTP server on an ephemeral port; return its base URL.
async fn spawn_relay() -> String {
    let dir = tempfile::tempdir().unwrap();
    let node = BlobNode::spawn(dir.path(), RelayChoice::Disabled)
        .await
        .expect("blob node");
    let state = AppState {
        mailbox: Arc::new(Mailbox::in_memory().expect("mailbox")),
        blobs: Arc::new(node),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Keep the temp dir alive for the server's lifetime.
    tokio::spawn(async move {
        let _dir = dir;
        axum::serve(listener, router(state)).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn code_roundtrip_delivers_ticket() {
    let relay = spawn_relay().await;
    let ticket = "arvcTHISISAFAKETICKETBUTITROUNDTRIPS";

    let (code, complete) = publish_ticket(ticket, &relay, false).await.expect("publish");
    // Code is `N-word-word` (no @relay when embed=false).
    assert!(!code.contains('@'));
    let sender = tokio::spawn(complete.run());

    let got = resolve_code(&code, Some(&relay)).await.expect("resolve");
    assert_eq!(got, ticket, "receiver recovers the exact ticket");
    sender.await.unwrap().expect("sender completes");
}

#[tokio::test]
async fn self_contained_code_needs_no_default_relay() {
    let relay = spawn_relay().await;
    let ticket = "arvcSELFCONTAINEDTICKET";

    // embed_relay=true -> code carries the relay; receiver passes no default.
    let (code, complete) = publish_ticket(ticket, &relay, true).await.expect("publish");
    assert!(code.contains('@'), "self-contained code embeds the relay");
    let sender = tokio::spawn(complete.run());

    let got = resolve_code(&code, None).await.expect("resolve with no default");
    assert_eq!(got, ticket);
    sender.await.unwrap().expect("sender completes");
}
