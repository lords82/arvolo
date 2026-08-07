//! Pairing as a *session*, for clients that cannot sit in a blocking call.
//!
//! The CLI's two pairing commands ([`crate::commands::pair`] and
//! [`crate::sync`]) are written for a terminal: they print a code, block until
//! the other machine answers, and prompt for anything they still need. None of
//! that survives a request/reply socket — the reply would arrive minutes later,
//! if at all, and there is nobody at a prompt on the other end of a GUI.
//!
//! So a daemon client starts a session instead. [`start`] returns as soon as the
//! work is spawned; the code to read out arrives as
//! [`EventDto::PairingCode`], and the outcome as [`EventDto::PairingDone`] or
//! [`EventDto::PairingFailed`]. [`Sessions::cancel`] stops one, which is what a
//! UI sends when its pairing sheet closes — an unattended `device pair` would
//! otherwise keep offering this device's identity secret for the whole window.
//!
//! The two kinds share this machinery and nothing else. Contact pairing trades
//! **public** ids between two people; device pairing hands over this device's
//! **identity secret**. See [`PairKind`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use arvolo_core::code;
use arvolo_core::crypto::Identity;
use arvolo_ipc::protocol::{EventDto, PairKind};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::book;

/// How long a *joining* side waits for the host to show up. The hosting side has
/// no deadline of its own — it waits until cancelled — because the person reading
/// the code out is right there deciding when to give up.
const JOIN_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// The rendezvous value cap on the relay is 64 KiB; keep the inline device-pair
/// payload (identity secret + full book snapshot) comfortably under it. Same
/// bound as [`crate::sync`]'s.
const MAX_PAIR_PAYLOAD: usize = 60 * 1024;

/// Live pairing sessions, keyed by the handle handed to the client.
#[derive(Clone, Default)]
pub struct Sessions {
    inner: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

impl Sessions {
    fn insert(&self, token: CancellationToken) -> String {
        let id = format!("pair-{}", NEXT_SESSION.fetch_add(1, Ordering::Relaxed));
        self.inner.lock().unwrap().insert(id.clone(), token);
        id
    }

    fn forget(&self, session: &str) {
        self.inner.lock().unwrap().remove(session);
    }

    /// Stop a session. Returns false for a handle this daemon doesn't know —
    /// already finished, or never started.
    pub fn cancel(&self, session: &str) -> bool {
        match self.inner.lock().unwrap().remove(session) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Stop every session. Called on daemon shutdown so a hosted device-pair code
    /// does not outlive the process that was offering the identity behind it.
    pub fn cancel_all(&self) {
        for (_, token) in self.inner.lock().unwrap().drain() {
            token.cancel();
        }
    }
}

/// Spawn a pairing session and return its handle. Everything else about it
/// arrives on `events`.
pub fn start(
    sessions: &Sessions,
    events: broadcast::Sender<EventDto>,
    kind: PairKind,
    relay: Option<String>,
    code_str: Option<String>,
    name: Option<String>,
) -> String {
    let token = CancellationToken::new();
    let session = sessions.insert(token.clone());

    let (sessions, id) = (sessions.clone(), session.clone());
    tokio::spawn(async move {
        let outcome = run(kind, relay, code_str, name, &id, &events, &token).await;
        sessions.forget(&id);
        let ev = match outcome {
            Ok(done) => EventDto::PairingDone {
                session: id,
                kind,
                summary: done.summary,
                needs_restart: done.needs_restart,
            },
            // A cancelled session is not a failure the user needs told about: they
            // are the one who closed the sheet. It still has to be *reported*, or a
            // UI waiting on an outcome would wait forever.
            Err(e) => EventDto::PairingFailed {
                session: id,
                kind,
                error: format!("{e:#}"),
                cancelled: token.is_cancelled(),
            },
        };
        let _ = events.send(ev);
    });

    session
}

struct Done {
    summary: String,
    needs_restart: bool,
}

async fn run(
    kind: PairKind,
    relay: Option<String>,
    code_str: Option<String>,
    name: Option<String>,
    session: &str,
    events: &broadcast::Sender<EventDto>,
    token: &CancellationToken,
) -> Result<Done> {
    match kind {
        PairKind::ContactHost => contact_host(relay, name, session, events, token).await,
        PairKind::ContactJoin => {
            let code = code_str.context("a pairing code is required to join")?;
            contact_join(code, name, token).await
        }
        PairKind::DeviceHost => device_host(relay, session, events, token).await,
        PairKind::DeviceJoin => {
            let code = code_str.context("a pairing code is required to join")?;
            device_join(code, token).await
        }
    }
}

fn resolve_relay(relay: Option<String>) -> Result<String> {
    match relay.map(|r| book::normalize_relay(&r, false)) {
        Some(r) => Ok(r),
        None => book::default_relay_or_builtin().context(
            "no relay configured — set one in settings, or pass one with the pairing request",
        ),
    }
}

// ---- contact pairing ------------------------------------------------------

/// Show a code and save whoever answers it, verified. The mutual exchange needs
/// rendezvous v2 (it is what carries the other side's id back), so a relay too old
/// for it is refused *before* a code is shown rather than half-way through.
async fn contact_host(
    relay: Option<String>,
    name: Option<String>,
    session: &str,
    events: &broadcast::Sender<EventDto>,
    token: &CancellationToken,
) -> Result<Done> {
    let relay = resolve_relay(relay)?;
    let me = crate::my_identity()?;
    let my_id = crate::encode_id(&me.public());

    if code::relay_rz_version(&relay).await != code::RzVersion::V2 {
        bail!(
            "{relay} is too old for a mutual exchange — it needs rendezvous v2, which is what \
             carries the other side's id back to you. Update the relay, or add them by id."
        );
    }

    let (shown, sender) = code::publish_auto(my_id.as_bytes(), &relay, true)
        .await
        .context("start contact pairing")?;
    let _ = events.send(EventDto::PairingCode {
        session: session.to_string(),
        code: shown,
    });

    let their_id = match sender {
        // Unreachable: the version check above already refused a v1 relay. Kept as
        // a refusal rather than an `unreachable!` so a future change to
        // `publish_auto` degrades into an error instead of a panic.
        code::CodeSender::V1(_) => bail!("this relay cannot carry a mutual exchange"),
        code::CodeSender::V2(host) => {
            let opts = code::HostOpts {
                max_sessions: Some(1),
                await_reply: true,
                ..code::HostOpts::default()
            };
            let mut got: Option<Vec<u8>> = None;
            host.run(
                my_id.as_bytes(),
                &opts,
                code::HostState::default(),
                token.clone(),
                |ev| {
                    if let code::HostEvent::Paired { reply, .. } = ev {
                        got = reply;
                    }
                },
                |_| {},
            )
            .await
            .context("contact pairing")?;
            got
        }
    };

    let Some(bytes) = their_id else {
        if token.is_cancelled() {
            bail!("pairing cancelled — nothing was saved");
        }
        bail!("the other side took your id but never sent theirs — nothing was saved here");
    };
    save_the_other_side(&String::from_utf8_lossy(&bytes), name)
}

/// Answer someone's code. Bounded by [`JOIN_WAIT`] and by cancellation, whichever
/// comes first: without the timeout a mistyped code would leave a session pending
/// until the daemon stopped.
async fn contact_join(
    code_str: String,
    name: Option<String>,
    token: &CancellationToken,
) -> Result<Done> {
    let me = crate::my_identity()?;
    let my_id = crate::encode_id(&me.public());
    let default_relay = book::default_relay_or_builtin();

    let exchange = code::exchange_bytes_with(
        &code_str,
        default_relay.as_deref(),
        JOIN_WAIT,
        Some(my_id.as_bytes()),
    );
    let (their_bytes, replied) = tokio::select! {
        _ = token.cancelled() => bail!("pairing cancelled — nothing was saved"),
        r = exchange => r.context("contact pairing")?,
    };

    // Saving only our half would leave the two of you disagreeing about what just
    // happened — you have them, they don't have you — which is worse than a clean
    // failure you can retry.
    if !replied {
        bail!(
            "the relay refused to carry your id back, so only half the exchange would have \
             happened — nothing was saved. That relay needs rendezvous v2."
        );
    }
    save_the_other_side(&String::from_utf8_lossy(&their_bytes), name)
}

/// Save the id just traded for, and mark it verified — the pairing *is* the
/// verification, since the SPAKE2 channel only forms between two parties that knew
/// the same code.
fn save_the_other_side(their_id: &str, name: Option<String>) -> Result<Done> {
    let their_id = their_id.trim();
    book::decode_id(their_id).context("the other side sent something that isn't a public id")?;

    if let Some(existing) = book::resolve_name(their_id) {
        book::mark_verified(their_id)?;
        return Ok(Done {
            summary: format!("'{existing}' è ora verificato — l'avevi già in rubrica."),
            needs_restart: false,
        });
    }

    let name = match name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
        Some(n) => n,
        // There is no prompt to fall back to here, and refusing at this point would
        // throw away an exchange that already succeeded — the other side has our id
        // and believes we have theirs. Name them after their own fingerprint: it is
        // unmistakable, it is theirs, and it is trivially renamed afterwards.
        None => unique_name_from_fingerprint(their_id),
    };
    book::contact_add(&name, their_id)?;
    book::mark_verified(&name)?;
    let fp = book::fingerprint_of(their_id).unwrap_or_default();
    Ok(Done {
        summary: format!("Salvato '{name}' — verificato. Impronta: {fp}"),
        needs_restart: false,
    })
}

/// A contact name derived from the peer's own fingerprint, made unique against the
/// book. Never rebinds an existing name: that would be a key change nobody asked
/// about, the one thing the trust model tries to make impossible by accident.
fn unique_name_from_fingerprint(id: &str) -> String {
    let fp = book::fingerprint_of(id).unwrap_or_default();
    let base: String = fp
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join("-");
    let base = if base.is_empty() {
        id.chars().take(8).collect()
    } else {
        base
    };
    let taken: std::collections::BTreeSet<String> =
        book::contact_list().into_iter().map(|(n, _)| n).collect();
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c))
        .unwrap_or(base)
}

// ---- device pairing -------------------------------------------------------

/// Hand this device's identity + address book to a new device. Strictly one-shot:
/// this is the identity secret itself going over the wire.
async fn device_host(
    relay: Option<String>,
    session: &str,
    events: &broadcast::Sender<EventDto>,
    token: &CancellationToken,
) -> Result<Done> {
    let relay = resolve_relay(relay)?;
    let me = crate::my_identity()?;
    let identity_secret: [u8; 32] = me
        .secret_bytes()
        .try_into()
        .map_err(|_| anyhow!("identity secret is not 32 bytes"))?;
    let snapshot = book::build_local_snapshot()?;
    let payload = arvolo_core::sync::PairPayload {
        identity_secret,
        snapshot,
    };
    let bytes = payload.encode()?;
    if bytes.len() > MAX_PAIR_PAYLOAD {
        bail!(
            "address book is too large to pair inline ({} KiB); the deposit-based fallback \
             is not implemented yet",
            bytes.len() / 1024
        );
    }

    let (shown, sender) = code::publish_auto(&bytes, &relay, true)
        .await
        .context("start device pairing")?;
    let _ = events.send(EventDto::PairingCode {
        session: session.to_string(),
        code: shown,
    });

    match sender {
        // `PairComplete::run` polls with its own two-minute timeout and takes no
        // cancellation token, so it has to be raced against ours. Without this,
        // closing the sheet reported success while the code — and the identity
        // secret behind it — stayed claimable for the rest of that window.
        code::CodeSender::V1(complete) => tokio::select! {
            _ = token.cancelled() => bail!("pairing cancelled — no device was linked"),
            r = complete.run() => r.context("device pairing")?,
        },
        code::CodeSender::V2(host) => {
            let opts = code::HostOpts {
                max_sessions: Some(1),
                ..code::HostOpts::default()
            };
            let reason = host
                .run(
                    &bytes,
                    &opts,
                    code::HostState::default(),
                    token.clone(),
                    |_| {},
                    |_| {},
                )
                .await
                .context("device pairing")?;
            if reason != code::CloseReason::MaxSessions {
                if token.is_cancelled() {
                    bail!("pairing cancelled — no device was linked");
                }
                bail!("device pairing did not complete ({reason:?})");
            }
        }
    }
    Ok(Done {
        summary: format!(
            "Nuovo dispositivo collegato — condivide la tua identità {} e la rubrica.",
            me.public().fingerprint()
        ),
        needs_restart: false,
    })
}

/// Adopt a shared identity from another device, *replacing* this one's.
///
/// Unlike the CLI's `device join` there is no confirmation prompt here: a GUI has
/// already asked before sending the request, and there is nobody at a terminal to
/// answer one. The destructive part is unchanged — files still sealed to the old
/// identity stop being openable here — so the caller is responsible for having
/// asked. What this *does* add is the restart flag: the running daemon is still
/// the old identity until it comes back up.
async fn device_join(code_str: String, token: &CancellationToken) -> Result<Done> {
    let default_relay = book::default_relay_or_builtin();
    let resolve = code::resolve_bytes(&code_str, default_relay.as_deref());
    let bytes = tokio::select! {
        _ = token.cancelled() => bail!("pairing cancelled — this device was not linked"),
        r = tokio::time::timeout(JOIN_WAIT, resolve) => match r {
            Ok(v) => v.context("device pairing")?,
            Err(_) => bail!(
                "nobody answered that code in time — check it, and that the other device is \
                 still showing it"
            ),
        },
    };

    let payload = arvolo_core::sync::PairPayload::decode(&bytes)?;
    let new_id = Identity::from_secret_bytes(&payload.identity_secret)
        .context("received identity is invalid")?;
    let path = crate::identity_path();

    // Idempotent re-join: same identity, nothing destroyed, nothing to restart for.
    let same = Identity::load(&path)
        .ok()
        .is_some_and(|e| e.public().to_bytes() == new_id.public().to_bytes());

    new_id.save(&path).context("save shared identity")?;
    book::apply_merged_state(&payload.snapshot).context("import address book")?;

    Ok(Done {
        summary: format!(
            "Collegato. Questo dispositivo condivide l'identità {} e {} contatti.",
            new_id.public().fingerprint(),
            book::contact_list().len()
        ),
        needs_restart: !same,
    })
}
