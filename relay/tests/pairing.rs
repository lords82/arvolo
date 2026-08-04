//! End-to-end short-code pairing over a real relay server: the sender publishes
//! a ticket under a code, the receiver resolves the code back to the same ticket.

use std::sync::{Arc, Mutex};

use arvolo_core::backfill::BlobNode;
use arvolo_core::code::{
    claim_code, publish_auto, publish_bytes, publish_ticket, relay_rz_version, resolve_bytes,
    resolve_code, CloseReason, CodeHost, CodeSender, HostEvent, HostOpts, HostState, Reattach,
    RzVersion,
};
use arvolo_core::sync::{PairPayload, SyncSnapshot};
use arvolo_core::transfer::RelayChoice;
use arvolo_relay::{router, AppState, Mailbox};
use tokio_util::sync::CancellationToken;

/// Spawn the relay HTTP server on an ephemeral port; return its base URL.
async fn spawn_relay() -> String {
    let dir = tempfile::tempdir().unwrap();
    let node = BlobNode::spawn(dir.path(), RelayChoice::Disabled)
        .await
        .expect("blob node");
    let state = AppState::new(
        Arc::new(Mailbox::in_memory().expect("mailbox")),
        Arc::new(node),
    );
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

    let (code, complete) = publish_ticket(ticket, &relay, false)
        .await
        .expect("publish");
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

    let got = resolve_code(&code, None)
        .await
        .expect("resolve with no default");
    assert_eq!(got, ticket);
    sender.await.unwrap().expect("sender completes");
}

// ---- rendezvous v2 --------------------------------------------------------

/// Drive a [`CodeHost`] in the background, recording every event and every state
/// change. Returns the join handle and the two recorders.
#[allow(clippy::type_complexity)]
fn spawn_host(
    host: CodeHost,
    payload: Vec<u8>,
    opts: HostOpts,
    state: HostState,
    cancel: CancellationToken,
) -> (
    tokio::task::JoinHandle<CloseReason>,
    Arc<Mutex<Vec<HostEvent>>>,
    Arc<Mutex<Vec<HostState>>>,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let states = Arc::new(Mutex::new(Vec::new()));
    let (ev, st) = (events.clone(), states.clone());
    let handle = tokio::spawn(async move {
        host.run(
            &payload,
            &opts,
            state,
            cancel,
            move |e| ev.lock().unwrap().push(e),
            move |s| st.lock().unwrap().push(s),
        )
        .await
        .expect("host loop")
    });
    (handle, events, states)
}

/// The same nameplate with the wrong words — what someone guessing types.
fn wrong_code(code: &str) -> String {
    let nameplate = code.split('-').next().unwrap();
    format!("{nameplate}-wrong-guess")
}

#[tokio::test]
async fn code_v2_roundtrip_delivers_ticket() {
    let relay = spawn_relay().await;
    let ticket = "arvcV2TICKETROUNDTRIP";

    let (code, host) = claim_code(&relay, false).await.expect("claim");
    assert!(!code.contains('@'));
    // The code grammar is untouched: an existing parser still reads it.
    assert_eq!(code.matches('-').count(), 2);

    let (handle, events, _) = spawn_host(
        host,
        ticket.as_bytes().to_vec(),
        HostOpts::default(),
        HostState::default(),
        CancellationToken::new(),
    );

    let got = resolve_code(&code, Some(&relay)).await.expect("resolve");
    assert_eq!(got, ticket);

    // Default policy is one-shot: having served its receiver, the code retires.
    assert_eq!(handle.await.unwrap(), CloseReason::MaxSessions);
    let events = events.lock().unwrap().clone();
    assert!(matches!(events.first(), Some(HostEvent::Listening)));
    assert!(events
        .iter()
        .any(|e| matches!(e, HostEvent::Paired { done: 1, .. })));

    // …and the nameplate is handed back rather than squatted.
    let (code2, host2) = claim_code(&relay, false).await.expect("reclaim space");
    drop((code2, host2));
}

#[tokio::test]
async fn code_v2_serves_several_receivers() {
    let relay = spawn_relay().await;
    let ticket = "arvcKEEPSERVINGTHISONE";

    let (code, host) = claim_code(&relay, false).await.expect("claim");
    let (handle, _, _) = spawn_host(
        host,
        ticket.as_bytes().to_vec(),
        HostOpts {
            max_sessions: Some(3),
            ..HostOpts::default()
        },
        HostState::default(),
        CancellationToken::new(),
    );

    // The v1 protocol could not do this at all: the first fetch destroyed the
    // slot. Here each receiver gets its own session, sealed to its own key.
    for i in 0..3 {
        let got = resolve_code(&code, Some(&relay))
            .await
            .unwrap_or_else(|e| panic!("receiver {i}: {e:#}"));
        assert_eq!(got, ticket, "receiver {i}");
    }
    assert_eq!(handle.await.unwrap(), CloseReason::MaxSessions);
}

#[tokio::test]
async fn code_v2_retires_itself_after_three_wrong_codes() {
    let relay = spawn_relay().await;
    let (code, host) = claim_code(&relay, false).await.expect("claim");
    let (handle, events, states) = spawn_host(
        host,
        b"arvcSECRET".to_vec(),
        HostOpts {
            max_sessions: None, // multi-use: the case the guess budget is for
            max_failures: 3,
            ..HostOpts::default()
        },
        HostState::default(),
        CancellationToken::new(),
    );

    // Three guesses at the two words. Each is refused *before* anything is
    // sealed — a wrong code never receives ciphertext at all.
    let bad = wrong_code(&code);
    for i in 0..3 {
        let err = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            resolve_code(&bad, Some(&relay)),
        )
        .await;
        assert!(
            matches!(err, Ok(Err(_)) | Err(_)),
            "guess {i} must not succeed"
        );
    }

    assert_eq!(handle.await.unwrap(), CloseReason::TooManyFailures);
    let events = events.lock().unwrap().clone();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, HostEvent::BadCode { .. }))
            .count(),
        3
    );
    assert!(events.iter().any(|e| matches!(
        e,
        HostEvent::Closed {
            reason: CloseReason::TooManyFailures
        }
    )));
    // Every failure was handed to the caller to persist as it happened.
    assert_eq!(states.lock().unwrap().last().unwrap().failures, 3);

    // The right code no longer works either: the slot is gone.
    let after = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        resolve_code(&code, Some(&relay)),
    )
    .await;
    assert!(matches!(after, Ok(Err(_)) | Err(_)), "the code is retired");
}

#[tokio::test]
async fn code_v2_reflection_and_abandonment_do_not_spend_the_guess_budget() {
    let relay = spawn_relay().await;
    let (code, host) = claim_code(&relay, false).await.expect("claim");
    let slot = host.slot.clone();
    let (handle, events, states) = spawn_host(
        host,
        b"arvcSECRET".to_vec(),
        HostOpts {
            max_sessions: Some(1),
            max_failures: 3,
            ..HostOpts::default()
        },
        HostState::default(),
        CancellationToken::new(),
    );
    let http = reqwest::Client::new();
    let rz = |key: &str| format!("{relay}/v1/rz/{slot}/{key}");

    // A session opened and then walked away from. Not an answer, so not a guess.
    let (_p, mr) = arvolo_core::pairing::start("some-other-code");
    http.post(rz("r.aaa")).body(mr).send().await.unwrap();

    // Wait for the sender's reply to that session, then reflect it back as a new
    // receiver's message. SPAKE2 is symmetric, so without a guard this makes the
    // sender derive a key nobody can use — and charges it as a wrong code.
    let ms = loop {
        let resp = http.get(rz("s.aaa")).send().await.unwrap();
        if resp.status().is_success() {
            break resp.bytes().await.unwrap().to_vec();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    http.post(rz("r.bbb")).body(ms).send().await.unwrap();

    // Wait until the host has actually answered the reflected session before
    // letting the honest receiver in. The reflection guard still posts a reply
    // (it answers so the session stops showing up as pending), so `s.bbb`
    // appearing is exactly "the host has seen and refused bbb".
    //
    // Without this the test raced its own setup: `max_sessions` is 1, so the
    // honest pairing takes the host straight to `Closed { MaxSessions }`, and if
    // that happened first the host retired without ever looking at bbb. The
    // security property held — nothing was charged — but the assertion below
    // demands the refusal be *observed*, which was never ordered.
    loop {
        if http
            .get(rz("s.bbb"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // The honest receiver still gets served, and the code was never charged.
    let got = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        resolve_code(&code, Some(&relay)),
    )
    .await
    .expect("honest receiver not starved")
    .expect("resolve");
    assert_eq!(got, "arvcSECRET");

    assert_eq!(handle.await.unwrap(), CloseReason::MaxSessions);
    let events = events.lock().unwrap().clone();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HostEvent::Rejected { sid } if sid == "bbb")),
        "the reflected session is refused: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, HostEvent::BadCode { .. })),
        "neither reflection nor abandonment counts as a guess: {events:?}"
    );
    assert_eq!(states.lock().unwrap().last().unwrap().failures, 0);
}

#[tokio::test]
async fn code_v2_survives_the_sender_restarting() {
    let relay = spawn_relay().await;
    let ticket = "arvcSURVIVESARESTART";
    let (code, host) = claim_code(&relay, false).await.expect("claim");

    // Round one: serve nobody, then lose the process. Cancelling drops the task
    // and everything it held — in v1 that included the SPAKE2 scalar, which is
    // precisely why a v1 code could not come back.
    let cancel = CancellationToken::new();
    let (handle, _, _) = spawn_host(
        host.clone(),
        ticket.as_bytes().to_vec(),
        HostOpts {
            max_sessions: Some(1),
            ..HostOpts::default()
        },
        HostState::default(),
        cancel.clone(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    cancel.cancel();
    let _ = handle.await;

    // Rebuild the sender from the four values a daemon would have on disk —
    // nothing else survives a restart.
    let restored = CodeHost {
        slot: host.slot.clone(),
        secret: host.secret.clone(),
        relay: host.relay.clone(),
        owner_token: host.owner_token,
    };
    // The slot was closed on cancel, so take the same nameplate back under the
    // same code: whoever wrote the code down still has a working one.
    assert_eq!(restored.reclaim().await.unwrap(), Reattach::Ok);
    assert_eq!(restored.reattach().await.unwrap(), Reattach::Ok);

    let (handle, _, _) = spawn_host(
        restored,
        ticket.as_bytes().to_vec(),
        HostOpts::default(),
        HostState::default(),
        CancellationToken::new(),
    );
    let got = resolve_code(&code, Some(&relay))
        .await
        .expect("the same code still pairs after a restart");
    assert_eq!(got, ticket);
    assert_eq!(handle.await.unwrap(), CloseReason::MaxSessions);
}

#[tokio::test]
async fn code_v2_failure_budget_persists_across_a_restart() {
    let relay = spawn_relay().await;
    let (code, host) = claim_code(&relay, false).await.expect("claim");
    let bad = wrong_code(&code);

    // Two guesses before the restart.
    let cancel = CancellationToken::new();
    let (handle, _, states) = spawn_host(
        host.clone(),
        b"arvcSECRET".to_vec(),
        HostOpts {
            max_sessions: None,
            max_failures: 3,
            ..HostOpts::default()
        },
        HostState::default(),
        cancel.clone(),
    );
    for _ in 0..2 {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            resolve_code(&bad, Some(&relay)),
        )
        .await;
    }
    let saved = *states
        .lock()
        .unwrap()
        .last()
        .expect("a state was persisted");
    assert_eq!(saved.failures, 2);
    cancel.cancel();
    let _ = handle.await;

    // Resume with the counter as it was persisted. Restarting must NOT hand the
    // guess budget back, or anyone able to provoke a restart gets unlimited
    // attempts at a two-word code.
    assert_eq!(host.reclaim().await.unwrap(), Reattach::Ok);
    let (handle, _, _) = spawn_host(
        host,
        b"arvcSECRET".to_vec(),
        HostOpts {
            max_sessions: None,
            max_failures: 3,
            ..HostOpts::default()
        },
        saved,
        CancellationToken::new(),
    );
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        resolve_code(&bad, Some(&relay)),
    )
    .await;
    assert_eq!(
        handle.await.unwrap(),
        CloseReason::TooManyFailures,
        "the third guess overall retires the code"
    );
}

#[tokio::test]
async fn publish_auto_picks_v2_and_the_receiver_follows() {
    let relay = spawn_relay().await;
    let ticket = b"arvcPUBLISHAUTO".to_vec();

    assert_eq!(relay_rz_version(&relay).await, RzVersion::V2);
    let (code, sender) = publish_auto(&ticket, &relay, true).await.expect("publish");
    assert!(code.contains('@'), "self-contained code embeds the relay");
    let CodeSender::V2(host) = sender else {
        panic!("a v2 relay must yield a v2 sender");
    };
    let (handle, _, _) = spawn_host(
        host,
        ticket.clone(),
        HostOpts::default(),
        HostState::default(),
        CancellationToken::new(),
    );

    // The receiver is told nothing about the protocol — it works it out from the
    // slot, which is why the code the user types never had to change.
    let got = resolve_bytes(&code, None).await.expect("resolve");
    assert_eq!(got, ticket);
    assert_eq!(handle.await.unwrap(), CloseReason::MaxSessions);
}

#[tokio::test]
async fn code_v2_cancel_frees_the_nameplate() {
    let relay = spawn_relay().await;
    let (_code, host) = claim_code(&relay, false).await.expect("claim");
    let slot = host.slot.clone();

    let cancel = CancellationToken::new();
    let (handle, events, _) = spawn_host(
        host,
        b"arvcX".to_vec(),
        HostOpts::default(),
        HostState::default(),
        cancel.clone(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    cancel.cancel();
    assert_eq!(handle.await.unwrap(), CloseReason::Cancelled);
    assert!(events.lock().unwrap().iter().any(|e| matches!(
        e,
        HostEvent::Closed {
            reason: CloseReason::Cancelled
        }
    )));

    // Cancelling a background code releases its slot instead of leaving it
    // squatted for the rest of the lease.
    let resp = reqwest::Client::new()
        .get(format!("{relay}/v1/rz/{slot}/own"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn device_pair_payload_roundtrips() {
    // Device pairing carries the shared identity secret + address-book snapshot as
    // an opaque byte payload over the same rendezvous the ticket flow uses.
    let relay = spawn_relay().await;
    let payload = PairPayload {
        identity_secret: [7u8; 32],
        snapshot: SyncSnapshot {
            lamport: 5,
            device: [1u8; 16],
            contacts: vec![],
            verified: vec![],
            trusted: vec![],
            blocked: vec![],
            seen: vec![],
            names: vec![],
        },
    };
    let bytes = payload.encode().unwrap();

    let (code, complete) = publish_bytes(bytes.clone(), &relay, true)
        .await
        .expect("publish");
    let sender = tokio::spawn(complete.run());

    let got = resolve_bytes(&code, None).await.expect("resolve");
    let recovered = PairPayload::decode(&got).expect("decode");
    assert_eq!(
        recovered, payload,
        "new device recovers the exact pair payload"
    );
    sender.await.unwrap().expect("sender completes");
}
