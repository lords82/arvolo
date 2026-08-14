//! `ARVOLO_P2P=off` must be enforced where the socket is created, not only at the
//! commands that ask for one.
//!
//! One test in its own file on purpose: it writes to the process environment, which
//! every thread in a test binary shares. Alone in this binary it races with nothing.

use arvolo_core::transfer::{bind_endpoint, p2p_enabled, RelayChoice};

#[tokio::test]
async fn the_switch_is_enforced_at_the_bind_not_at_the_callers() {
    // Baseline: unset means on, and a relay-less endpoint binds (a local UDP socket,
    // no network needed). Without this half, the test below would also pass if
    // binding were simply broken.
    std::env::remove_var("ARVOLO_P2P");
    assert!(p2p_enabled());
    bind_endpoint(RelayChoice::Disabled)
        .await
        .expect("binds with p2p on");

    std::env::set_var("ARVOLO_P2P", "off");
    assert!(!p2p_enabled());
    let err = bind_endpoint(RelayChoice::Disabled)
        .await
        .expect_err("must refuse to bind a P2P endpoint when P2P is off");
    // The message has to send the reader somewhere, not just say no: this is reached
    // by paths that never mentioned P2P, so it names the way out.
    let msg = err.to_string();
    assert!(msg.contains("mailbox"), "unhelpful refusal: {msg}");
    assert!(msg.contains("--deposit"), "unhelpful refusal: {msg}");

    std::env::remove_var("ARVOLO_P2P");
}
