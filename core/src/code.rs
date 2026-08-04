//! Short-code pairing (magic-wormhole / croc style).
//!
//! Instead of copying a ~1000-char `arvc` ticket, the sender shows a short human
//! code like `4821-crater-mango`; the receiver types it. The ticket is exchanged
//! over a relay **rendezvous** and protected by a **SPAKE2** PAKE keyed on the
//! code, so the relay stays zero-knowledge (it only sees PAKE messages and the
//! **encrypted** ticket) and a short code is safe (no offline dictionary attack).
//!
//! The code may embed the sender's relay (`code@https://relay…`) so it works even
//! when the two sides use different relays; without it, the receiver's configured
//! default relay is used.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use rand::Rng;
use tokio_util::sync::CancellationToken;

use crate::crypto::{open_chunk, seal_chunk, CHUNK_KEY_LEN};
use crate::pairing;
use crate::wordlist::WORDS;

/// Sender's SPAKE2 message / slot-claim key.
const K_MS: &str = "ms";
/// Receiver's SPAKE2 message key.
const K_MR: &str = "mr";
/// Encrypted-ticket key (fetching it burns the slot).
const K_TKT: &str = "tkt";
/// Total time to wait for the peer at each rendezvous step.
const POLL_TIMEOUT: Duration = Duration::from_secs(120);

/// Parse a code into `(nameplate/slot, pake_secret, optional_relay_url)`.
/// Accepts `N-word-word` and `N-word-word@relay-url`.
pub fn parse_code(code: &str) -> Result<(String, String, Option<String>)> {
    let code = code.trim();
    let (secret, relay) = match code.split_once('@') {
        Some((s, r)) if !r.is_empty() => (s.to_string(), Some(r.to_string())),
        _ => (code.to_string(), None),
    };
    let nameplate = secret.split('-').next().unwrap_or("").to_string();
    if nameplate.is_empty() || secret.matches('-').count() < 2 {
        bail!("invalid code (expected N-word-word[@relay])");
    }
    Ok((nameplate, secret, relay))
}

/// Ensure a relay base URL carries a scheme. A bare host (`relay.example.com`)
/// gets `https://` — or `http://` when `use_http` is set (LAN / dev / plaintext).
/// An address that already has an explicit scheme is left untouched.
pub fn normalize_relay(raw: &str, use_http: bool) -> String {
    let r = raw.trim();
    if r.contains("://") {
        return r.to_string();
    }
    let scheme = if use_http { "http" } else { "https" };
    format!("{scheme}://{r}")
}

/// The compact form embedded in a pairing code: `https://` (the default) is
/// dropped so the code reads `code@host`; a non-default `http://` is kept
/// explicit so the receiver still knows to skip TLS. Inverse of `normalize_relay`.
fn compact_relay(relay: &str) -> String {
    relay
        .strip_prefix("https://")
        .map(str::to_string)
        .unwrap_or_else(|| relay.to_string())
}

/// `true` if `s` looks like a pairing code (vs. an `arvc…`/`arvm…` ticket).
pub fn looks_like_code(s: &str) -> bool {
    let head = s.split_once('@').map(|(l, _)| l).unwrap_or(s);
    let parts: Vec<&str> = head.split('-').collect();
    parts.len() >= 3 && parts[0].chars().all(|c| c.is_ascii_digit()) && !parts[0].is_empty()
}

/// A fresh `(nameplate, secret)`, e.g. `("4821", "4821-crater-mango")`.
fn gen_secret() -> (String, String) {
    let mut rng = rand::rng();
    let nameplate = rng.random_range(0u32..10_000).to_string();
    let w1 = WORDS[rng.random_range(0..WORDS.len())];
    let w2 = WORDS[rng.random_range(0..WORDS.len())];
    let secret = format!("{nameplate}-{w1}-{w2}");
    (nameplate, secret)
}

/// Derive the 32-byte ticket-encryption key from the SPAKE2 shared secret via a
/// KDF (BLAKE3 in key-derivation mode) with a domain-separated context.
///
/// This replaces a raw truncate-to-32-bytes of the PAKE output: truncation would
/// silently **zero-pad** if the group ever yielded fewer than 32 bytes (reducing
/// key entropy) and skips domain separation. `derive_key` always produces a full
/// 32-byte key bound to this context regardless of the input length.
fn key32(pake_key: &[u8]) -> [u8; CHUNK_KEY_LEN] {
    blake3::derive_key("arvolo/code/ticket-key/v1", pake_key)
}

fn rz_url(relay: &str, slot: &str, key: &str) -> String {
    format!("{}/v1/rz/{slot}/{key}", relay.trim_end_matches('/'))
}

/// Poll a rendezvous key until it's posted (or time out).
async fn poll_get(client: &reqwest::Client, url: &str, what: &str) -> Result<Vec<u8>> {
    poll_get_for(client, url, what, None, POLL_TIMEOUT).await
}

/// Poll a rendezvous key until it's posted, giving up after `timeout`. `token`
/// authenticates the read for the owner-only keys of a v2 slot.
async fn poll_get_for(
    client: &reqwest::Client,
    url: &str,
    what: &str,
    token: Option<&str>,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let start = Instant::now();
    loop {
        let mut req = client.get(url);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await.context("rendezvous poll")?;
        if resp.status().is_success() {
            return Ok(resp
                .bytes()
                .await
                .context("read rendezvous value")?
                .to_vec());
        }
        if resp.status() != reqwest::StatusCode::NOT_FOUND {
            resp.error_for_status().context("rendezvous poll")?;
        }
        if start.elapsed() > timeout {
            bail!("timed out waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The sender's in-flight pairing: it has claimed a slot and posted its message;
/// [`PairComplete::run`] finishes the handshake and publishes the encrypted
/// ticket once the receiver shows up.
pub struct PairComplete {
    slot: String,
    relay: String,
    payload: Vec<u8>,
    pairing: pairing::Pairing,
    client: reqwest::Client,
}

/// Claim a rendezvous slot and post the sender's SPAKE2 message. Returns the code
/// to display (with `@relay` appended when `embed_relay`) and a handle to finish
/// the exchange. Retries on a slot collision.
pub async fn publish_ticket(
    ticket: &str,
    relay: &str,
    embed_relay: bool,
) -> Result<(String, PairComplete)> {
    publish_bytes(ticket.as_bytes().to_vec(), relay, embed_relay).await
}

/// Like [`publish_ticket`] but transfers arbitrary bytes (e.g. a postcard-encoded
/// device-pairing payload) instead of a UTF-8 ticket string.
pub async fn publish_bytes(
    payload: Vec<u8>,
    relay: &str,
    embed_relay: bool,
) -> Result<(String, PairComplete)> {
    let relay = relay.trim_end_matches('/').to_string();
    let client = reqwest::Client::new();

    let (slot, secret, pairing) = loop {
        let (slot, secret) = gen_secret();
        let (p, ms) = pairing::start(&secret);
        let resp = client
            .post(rz_url(&relay, &slot, K_MS))
            .body(ms)
            .send()
            .await
            .context("rendezvous claim")?;
        if resp.status() == reqwest::StatusCode::CONFLICT {
            continue; // slot taken, pick a new nameplate
        }
        resp.error_for_status().context("rendezvous claim")?;
        break (slot, secret, p);
    };

    let shown = if embed_relay {
        format!("{secret}@{}", compact_relay(&relay))
    } else {
        secret
    };
    Ok((
        shown,
        PairComplete {
            slot,
            relay,
            payload,
            pairing,
            client,
        },
    ))
}

impl PairComplete {
    /// Wait for the receiver, derive the shared key, and publish the encrypted
    /// payload. Completes once the receiver has shown up.
    pub async fn run(self) -> Result<()> {
        let mr = poll_get(
            &self.client,
            &rz_url(&self.relay, &self.slot, K_MR),
            "the receiver",
        )
        .await?;
        let key = key32(&self.pairing.finish(&mr)?);
        let ct = seal_chunk(&key, 0, 1, &self.payload)?;
        self.client
            .post(rz_url(&self.relay, &self.slot, K_TKT))
            .body(ct)
            .send()
            .await
            .context("publish ticket")?
            .error_for_status()
            .context("publish ticket")?;
        Ok(())
    }
}

/// Resolve a pairing code to its `arvc` ticket via the relay rendezvous. Uses the
/// relay embedded in the code, else `default_relay`.
pub async fn resolve_code(code: &str, default_relay: Option<&str>) -> Result<String> {
    let bytes = resolve_bytes(code, default_relay).await?;
    String::from_utf8(bytes).context("ticket is not valid UTF-8")
}

/// Like [`resolve_code`] but returns the raw transferred bytes (for a
/// device-pairing payload rather than a UTF-8 ticket).
pub async fn resolve_bytes(code: &str, default_relay: Option<&str>) -> Result<Vec<u8>> {
    resolve_bytes_with(code, default_relay, POLL_TIMEOUT).await
}

/// [`resolve_bytes`] with an explicit deadline. A v2 code can be alive for hours,
/// so "wait two minutes for the sender" stops being the only sensible answer;
/// callers that know they're facing a long-lived code can wait longer.
///
/// Works against either protocol. Which one a code speaks isn't in the code —
/// deliberately, so the grammar a user types never changed — so it is discovered
/// by looking at the slot: a v2 slot carries an `own` marker, a v1 slot carries
/// the sender's SPAKE2 message under `ms`. Whichever appears first wins, which
/// also covers a receiver who types the code before the sender has finished
/// claiming the slot.
pub async fn resolve_bytes_with(
    code: &str,
    default_relay: Option<&str>,
    timeout: Duration,
) -> Result<Vec<u8>> {
    exchange_bytes_with(code, default_relay, timeout, None)
        .await
        .map(|(payload, _)| payload)
}

/// Collect the sender's payload **and** seal one back to it.
///
/// The plain [`resolve_bytes`] is a handover: the sender puts something in the
/// slot and the receiver takes it. This is an *exchange* — used by
/// `arvolo contacts pair`, where both sides must come away knowing the other's
/// public id, and neither is more the sender than the other.
///
/// Returns the payload and whether the reply was accepted. `false` means the
/// relay refused it — only a v2 rendezvous has the key for it, and v1 reports
/// `false` without trying. Callers that need a *mutual* exchange should treat that
/// as a failure: a half-completed trade leaves the two sides disagreeing about
/// what happened.
pub async fn exchange_bytes_with(
    code: &str,
    default_relay: Option<&str>,
    timeout: Duration,
    reply: Option<&[u8]>,
) -> Result<(Vec<u8>, bool)> {
    let (slot, secret, relay_in_code) = parse_code(code)?;
    let relay = relay_in_code
        .or_else(|| default_relay.map(|s| s.to_string()))
        .ok_or_else(|| {
            anyhow!("no relay: the code has no @relay and no default relay is configured")
        })?;
    // A bare host embedded in the code (`code@host`) means https; `http://…` is
    // kept as-is (see `compact_relay`).
    let relay = normalize_relay(relay.trim_end_matches('/'), false);
    let client = reqwest::Client::new();

    match detect_version(&client, &relay, &slot, timeout).await? {
        RzVersion::V2 => resolve_v2(&client, &relay, &slot, &secret, timeout, reply).await,
        RzVersion::V1 => resolve_v1(&client, &relay, &slot, &secret, timeout)
            .await
            .map(|payload| (payload, false)),
    }
}

/// The v1 exchange: read the sender's message, post ours, fetch the sealed
/// payload. One receiver, one shot — fetching the ticket destroys the slot.
async fn resolve_v1(
    client: &reqwest::Client,
    relay: &str,
    slot: &str,
    secret: &str,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let ms = poll_get_for(
        client,
        &rz_url(relay, slot, K_MS),
        "the sender",
        None,
        timeout,
    )
    .await?;
    let (pairing, mr) = pairing::start(secret);
    client
        .post(rz_url(relay, slot, K_MR))
        .body(mr)
        .send()
        .await
        .context("post pairing message")?
        .error_for_status()
        .context("post pairing message")?;
    let key = key32(&pairing.finish(&ms)?);

    // Fetch and decrypt the payload (wrong code -> decrypt fails).
    let ct = poll_get_for(
        client,
        &rz_url(relay, slot, K_TKT),
        "the ticket",
        None,
        timeout,
    )
    .await?;
    open_chunk(&key, 0, 1, &ct).context("decrypt ticket (wrong code?)")
}

// ---- rendezvous v2: a long-lived slot, one sub-session per receiver --------
//
// v1 models a code as a single in-memory handshake: the sender holds a live
// SPAKE2 scalar, waits for exactly one receiver, and the relay destroys the slot
// when the ticket is fetched. Three consequences, all of them in the way of a
// code the daemon can serve: the code dies in minutes, it serves one receiver,
// and it cannot survive the sender restarting — the scalar was only ever in RAM.
//
// v2 keeps the code grammar and the PAKE, and moves the state. The slot becomes
// a mailbox the sender holds a capability token for; each receiver picks its own
// random session id and gets four private keys; the sender answers each with a
// FRESH SPAKE2 run. What the sender must remember between one receiver and the
// next is then only `(slot, secret, relay, owner_token)` — a [`CodeHost`], four
// values that fit in a file, which is exactly what makes a code restart-proof.
//
// The one thing v2 must buy back: a multi-use, long-lived code would otherwise
// give an attacker unlimited online guesses at the two words, where v1's
// burn-on-fetch allowed exactly one. So the receiver proves it derived the same
// key (a MAC over the transcript) *before* the sender seals anything, and the
// sender retires the code after a few wrong ones.

/// Slot-claim key: holds `blake3(owner_token)`; the token itself never travels.
const K_OWN: &str = "own";
/// Owner-only listing of receivers waiting for an answer.
const K_SESSIONS: &str = "sessions";
/// Per-session keys: receiver's PAKE message, sender's, receiver's key
/// confirmation, and the sealed payload.
const P_RECV: &str = "r.";
const P_SEND: &str = "s.";
const P_CONF: &str = "c.";
const P_TKT: &str = "t.";
/// The receiver's sealed reply, travelling back to the sender (see
/// [`reply_key32`]). Optional: a receiver that has nothing to say never writes it,
/// and a relay without the prefix rejects the write, which is how a client learns
/// this rendezvous cannot carry a reply.
const P_BACK: &str = "b.";
/// How long the sender waits for that reply once it has sealed its own half.
///
/// Short on purpose. The receiver writes its reply immediately after opening the
/// payload, so either it arrives within a round-trip or the other side is an older
/// client that will never send one — and in that case the exchange still succeeded
/// in the direction that matters, so there is nothing to be gained by waiting.
const REPLY_WAIT: Duration = Duration::from_secs(15);
/// How long the sessions listing may be held open by the relay.
const SESSIONS_WAIT_SECS: u64 = 30;
/// How long to wait for one receiver to finish its half once it has shown up.
/// Deliberately short: it is already online and mid-exchange, so an honest step
/// takes well under a second.
///
/// Sessions are answered one at a time, so this is also how long one abandoned
/// receiver can hold up the next — the reason it isn't minutes. Stalling this way
/// is a poor attack anyway: three wrong guesses retire the code outright, which
/// costs an attacker less and hurts more.
const SESSION_STEP_TIMEOUT: Duration = Duration::from_secs(10);
/// Nameplates to try before giving up on a collision.
const MAX_CLAIM_ATTEMPTS: usize = 32;
/// Longest a confirmed receiver waits for its sealed payload. Generous next to
/// the sub-second an honest sender takes, but bounded: sessions are answered one
/// at a time, so a few abandoned ones ahead in the queue can hold things up.
const TICKET_WAIT: Duration = Duration::from_secs(60);

/// Which rendezvous protocol a relay (or a slot) speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RzVersion {
    V1,
    V2,
}

/// Everything the sender of a v2 code has to remember. Four values, all
/// serializable — no PAKE scalar, no HTTP client, no running task — which is what
/// lets a daemon write a live code to disk and pick it up again after a restart.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CodeHost {
    /// Nameplate: the leading digits of the code, and the relay-side slot id.
    pub slot: String,
    /// The full code, which is also the PAKE password.
    pub secret: String,
    /// Rendezvous relay, normalized with a scheme.
    pub relay: String,
    /// Secret capability for this slot: authorizes answering sessions, renewing
    /// the lease, and retiring the code.
    pub owner_token: [u8; 32],
}

/// The counters that decide when a code stops working. The caller owns them
/// because they must be persisted: if `failures` reset on restart, an attacker
/// who can provoke a restart gets its guess budget back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostState {
    pub sessions_done: u32,
    pub failures: u32,
}

/// Policy for [`CodeHost::run`].
#[derive(Clone, Copy, Debug)]
pub struct HostOpts {
    /// Receivers to serve before retiring the code; `None` is "until cancelled".
    pub max_sessions: Option<u32>,
    /// Wrong-code attempts to tolerate before retiring it.
    pub max_failures: u32,
    /// Seconds to hold the sessions long-poll open.
    pub wait_secs: u64,
    /// Collect the receiver's sealed reply before counting the session done.
    ///
    /// Off by default, and deliberately: an ordinary `arvolo code` send has
    /// nothing to hear back, and waiting for a reply nobody will write would add
    /// a pause to every transfer. Only a mutual exchange turns it on.
    pub await_reply: bool,
}

impl Default for HostOpts {
    fn default() -> Self {
        Self {
            max_sessions: Some(1),
            max_failures: 3,
            wait_secs: SESSIONS_WAIT_SECS,
            await_reply: false,
        }
    }
}

/// What the host loop is doing, for a UI or a log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEvent {
    /// The slot is claimed and the loop is waiting for receivers.
    Listening,
    /// A receiver proved it knows the code and has been handed the payload.
    /// `reply` is what it sealed back, when [`HostOpts::await_reply`] asked for
    /// one and the receiver had something to say.
    Paired {
        sid: String,
        done: u32,
        reply: Option<Vec<u8>>,
    },
    /// A receiver failed key confirmation — a wrong code, or someone guessing.
    BadCode {
        sid: String,
        failures: u32,
        max: u32,
    },
    /// A session was refused without spending the guess budget (see the
    /// reflection guard in [`CodeHost::answer`]).
    Rejected { sid: String },
    /// The loop has stopped and the code is no longer usable.
    Closed { reason: CloseReason },
}

/// Why a code stopped working.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// Served every receiver it was allowed to.
    MaxSessions,
    /// Retired itself after too many wrong-code attempts.
    TooManyFailures,
    /// The relay no longer holds the slot (lease elapsed, or the 24h ceiling).
    Expired,
    /// The nameplate now belongs to someone else — this code is dead.
    Taken,
    /// The caller cancelled.
    Cancelled,
}

/// Outcome of re-asserting ownership of a slot after a restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reattach {
    /// Still ours, and the lease has been renewed.
    Ok,
    /// The slot is gone; the same nameplate may be re-claimed.
    Expired,
    /// Someone else holds the nameplate now.
    Taken,
}

/// Ask a relay whether it speaks rendezvous v2.
///
/// Fails **closed**, unlike [`crate::link::relay_allows_links`]: an unreachable
/// relay, a 404, or a features document without the field all mean v1. Guessing
/// wrong in the optimistic direction would mint a code no relay can host and
/// strand whoever types it; guessing wrong the other way just falls back.
pub async fn relay_rz_version(relay: &str) -> RzVersion {
    let url = format!("{}/v1/features", relay.trim_end_matches('/'));
    let Ok(resp) = reqwest::Client::new().get(&url).send().await else {
        return RzVersion::V1;
    };
    if !resp.status().is_success() {
        return RzVersion::V1;
    }
    let body = resp.text().await.unwrap_or_default();
    if body
        .split_whitespace()
        .collect::<String>()
        .contains("\"rz2\":true")
    {
        RzVersion::V2
    } else {
        RzVersion::V1
    }
}

/// The owner token as it travels: base32, lowercase, in an `Authorization`
/// header (never a query parameter — those get logged verbatim by proxies).
fn token_string(token: &[u8; 32]) -> String {
    data_encoding::BASE32_NOPAD.encode(token).to_lowercase()
}

/// A fresh session id: 128 bits, so the sender's answer to one receiver is not
/// addressable by another. Lowercase base32 keeps it inside the relay's key
/// grammar.
fn gen_sid() -> String {
    let raw: [u8; 16] = rand::random();
    data_encoding::BASE32_NOPAD.encode(&raw).to_lowercase()
}

/// Derive the payload key for one v2 session. Distinct from [`key32`] by context
/// *and* by mixing in the transcript, so no two exchanges — across protocol
/// versions or across sessions of one code — can ever share a key.
fn ticket_key32(pake_key: &[u8], slot: &str, sid: &str) -> [u8; CHUNK_KEY_LEN] {
    let mut input = Vec::with_capacity(pake_key.len() + slot.len() + sid.len() + 2);
    input.extend_from_slice(pake_key);
    input.push(0);
    input.extend_from_slice(slot.as_bytes());
    input.push(0);
    input.extend_from_slice(sid.as_bytes());
    blake3::derive_key("arvolo/code/ticket-key/v2", &input)
}

/// Key for the receiver→sender reply, the other direction of a mutual exchange.
///
/// Domain-separated from [`ticket_key32`] over the same input, so the two
/// directions can never be confused: a payload sealed for one will not open under
/// the other, and a relay that swapped `t.` and `b.` rows would produce two AEAD
/// failures rather than a silent crossover.
fn reply_key32(pake_key: &[u8], slot: &str, sid: &str) -> [u8; CHUNK_KEY_LEN] {
    let mut input = Vec::with_capacity(pake_key.len() + slot.len() + sid.len() + 2);
    input.extend_from_slice(pake_key);
    input.push(0);
    input.extend_from_slice(slot.as_bytes());
    input.push(0);
    input.extend_from_slice(sid.as_bytes());
    blake3::derive_key("arvolo/code/reply-key/v2", &input)
}

/// The receiver's proof that it derived the same PAKE key, bound to this slot and
/// session so it can't be replayed into another.
///
/// Only this direction is explicit. The sender's own confirmation is already
/// implied by the sealed payload: a wrong key fails the AEAD tag in `open_chunk`.
/// The receiver's, though, is what lets the sender *count* wrong codes — without
/// it the sender hands ciphertext to every guesser and can never tell.
fn confirm_mac(pake_key: &[u8], slot: &str, sid: &str) -> blake3::Hash {
    let key = blake3::derive_key("arvolo/code/confirm-recv/v2", pake_key);
    let mut input = Vec::with_capacity(slot.len() + sid.len() + 1);
    input.extend_from_slice(slot.as_bytes());
    input.push(0);
    input.extend_from_slice(sid.as_bytes());
    blake3::keyed_hash(&key, &input)
}

/// Claim a fresh v2 slot on `relay`, retrying on a nameplate collision exactly as
/// v1 does. Returns the code to show and the state to keep (and to persist).
pub async fn claim_code(relay: &str, embed_relay: bool) -> Result<(String, CodeHost)> {
    let relay = relay.trim_end_matches('/').to_string();
    let client = reqwest::Client::new();
    let owner_token: [u8; 32] = rand::random();
    // The relay stores only the hash, so a compromised relay database still
    // can't answer sessions in our name.
    let verifier = blake3::hash(&owner_token).as_bytes().to_vec();

    for _ in 0..MAX_CLAIM_ATTEMPTS {
        let (slot, secret) = gen_secret();
        let resp = client
            .post(rz_url(&relay, &slot, K_OWN))
            .body(verifier.clone())
            .send()
            .await
            .context("rendezvous claim")?;
        if resp.status() == reqwest::StatusCode::CONFLICT {
            continue; // nameplate taken (by either protocol), pick another
        }
        resp.error_for_status().context("rendezvous claim")?;
        let host = CodeHost {
            slot,
            secret,
            relay,
            owner_token,
        };
        let shown = host.shown(embed_relay);
        return Ok((shown, host));
    }
    bail!("could not find a free pairing code on {relay} — try again")
}

impl CodeHost {
    /// The code as the user sees it, with `@relay` appended when the receiver
    /// should not have to be configured.
    pub fn shown(&self, embed_relay: bool) -> String {
        if embed_relay {
            format!("{}@{}", self.secret, compact_relay(&self.relay))
        } else {
            self.secret.clone()
        }
    }

    fn token(&self) -> String {
        token_string(&self.owner_token)
    }

    /// Re-assert ownership — the first thing to do after loading a persisted
    /// host. Renews the lease as a side effect, and tells apart "my slot lapsed,
    /// I can take the nameplate back" from "someone else has it, this code is
    /// dead", which are the same 404-shaped silence otherwise.
    pub async fn reattach(&self) -> Result<Reattach> {
        let client = reqwest::Client::new();
        let resp = client
            .get(rz_url(&self.relay, &self.slot, K_SESSIONS))
            .bearer_auth(self.token())
            .send()
            .await
            .context("reattach to rendezvous slot")?;
        Ok(match resp.status() {
            s if s.is_success() => Reattach::Ok,
            reqwest::StatusCode::FORBIDDEN => Reattach::Taken,
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE => Reattach::Expired,
            s => bail!("rendezvous relay refused the slot: {s}"),
        })
    }

    /// Take an expired nameplate back under the same code and token, so a code
    /// already given out keeps working across an outage longer than its lease.
    pub async fn reclaim(&self) -> Result<Reattach> {
        let verifier = blake3::hash(&self.owner_token).as_bytes().to_vec();
        let resp = reqwest::Client::new()
            .post(rz_url(&self.relay, &self.slot, K_OWN))
            .body(verifier)
            .send()
            .await
            .context("reclaim rendezvous slot")?;
        Ok(match resp.status() {
            s if s.is_success() => Reattach::Ok,
            reqwest::StatusCode::CONFLICT => Reattach::Taken,
            s => bail!("rendezvous relay refused the claim: {s}"),
        })
    }

    /// Retire the code: free the nameplate instead of squatting it until the
    /// lease runs out. Best-effort — a failure here costs a nameplate, not
    /// correctness.
    pub async fn close(&self) -> Result<()> {
        reqwest::Client::new()
            .delete(rz_url(&self.relay, &self.slot, K_OWN))
            .bearer_auth(self.token())
            .send()
            .await
            .context("close rendezvous slot")?;
        Ok(())
    }

    /// Serve this code until it is used up, guessed at too often, cancelled, or
    /// the relay lets go of the slot.
    ///
    /// `state` is threaded through `on_state` on every change so the caller can
    /// persist it *before* the next network round trip: the failure counter is
    /// only a real limit if it survives a crash.
    pub async fn run(
        &self,
        payload: &[u8],
        opts: &HostOpts,
        mut state: HostState,
        cancel: CancellationToken,
        mut on_event: impl FnMut(HostEvent) + Send,
        mut on_state: impl FnMut(HostState) + Send,
    ) -> Result<CloseReason> {
        let client = reqwest::Client::new();
        // Every PAKE message we have put on the relay. `start_symmetric` uses
        // M=N, so an attacker can echo our own message back as a receiver's and
        // make us derive a key it cannot compute — harmless to the secret, but it
        // would burn a third of the guess budget for the price of two POSTs.
        let mut emitted: HashSet<Vec<u8>> = HashSet::new();
        // Sessions we have sealed a payload for but that haven't collected it yet.
        // Retiring the code deletes the slot, so it must not happen while one of
        // these is still on the relay — that would take the ticket back out of a
        // receiver's hands a moment after handing it over.
        let mut outstanding: HashSet<String> = HashSet::new();
        on_event(HostEvent::Listening);

        loop {
            if cancel.is_cancelled() {
                return Ok(self.finish(CloseReason::Cancelled, &mut on_event).await);
            }
            if let Some(max) = opts.max_sessions {
                if state.sessions_done >= max {
                    self.drain(&client, &mut outstanding).await;
                    return Ok(self.finish(CloseReason::MaxSessions, &mut on_event).await);
                }
            }
            // Checked before spending any request, so a burnt code stops costing
            // the relay (and us) anything at all. No draining here: a session that
            // failed confirmation was never sealed anything to collect.
            if state.failures >= opts.max_failures {
                return Ok(self
                    .finish(CloseReason::TooManyFailures, &mut on_event)
                    .await);
            }

            let pending = tokio::select! {
                _ = cancel.cancelled() => {
                    return Ok(self.finish(CloseReason::Cancelled, &mut on_event).await)
                }
                r = self.poll_sessions(&client, opts.wait_secs) => r?,
            };
            let sids = match pending {
                Ok(sids) => sids,
                // The relay let go of the slot, or handed the nameplate on.
                Err(reason) => {
                    on_event(HostEvent::Closed { reason });
                    return Ok(reason);
                }
            };

            for sid in sids {
                if cancel.is_cancelled() {
                    return Ok(self.finish(CloseReason::Cancelled, &mut on_event).await);
                }
                if state.failures >= opts.max_failures {
                    break;
                }
                if let Some(max) = opts.max_sessions {
                    if state.sessions_done >= max {
                        break;
                    }
                }
                match self
                    .answer(&client, &sid, payload, &mut emitted, opts.await_reply)
                    .await
                {
                    Ok(Answer::Paired { reply }) => {
                        state.sessions_done += 1;
                        outstanding.insert(sid.clone());
                        on_state(state);
                        on_event(HostEvent::Paired {
                            sid,
                            done: state.sessions_done,
                            reply,
                        });
                    }
                    Ok(Answer::BadCode) => {
                        state.failures += 1;
                        // Persist before doing anything else: this counter is the
                        // whole brute-force defence.
                        on_state(state);
                        on_event(HostEvent::BadCode {
                            sid,
                            failures: state.failures,
                            max: opts.max_failures,
                        });
                    }
                    Ok(Answer::Rejected) | Ok(Answer::Abandoned) => {
                        on_event(HostEvent::Rejected { sid })
                    }
                    // A transport hiccup on one session must not kill the code.
                    Err(e) => tracing::debug!("pairing session {sid} failed: {e:#}"),
                }
            }
        }
    }

    /// Wait for the receivers we have already sealed a payload for to collect it.
    ///
    /// Fetching a session's ticket is what deletes it, so "collected" is simply
    /// "gone from the relay". Bounded: a receiver that never comes back must not
    /// keep the nameplate alive forever, and it has its ticket sealed and waiting
    /// until the lease runs out either way.
    async fn drain(&self, client: &reqwest::Client, outstanding: &mut HashSet<String>) {
        let start = Instant::now();
        while !outstanding.is_empty() && start.elapsed() < SESSION_STEP_TIMEOUT {
            let mut collected = Vec::new();
            for sid in outstanding.iter() {
                // Watch `r.{sid}`, never `t.{sid}`: reading the ticket is what
                // burns the session, so polling it here would mean the sender
                // collecting its own delivery and leaving the receiver on 404.
                let url = rz_url(&self.relay, &self.slot, &format!("{P_RECV}{sid}"));
                match client.get(&url).bearer_auth(self.token()).send().await {
                    // Still there: this receiver hasn't collected yet.
                    Ok(r) if r.status().is_success() => {}
                    // Gone (collected), or the slot is no longer ours to watch.
                    Ok(_) => collected.push(sid.clone()),
                    // A relay we can't reach isn't a reason to sit here; the
                    // ticket is posted and the lease outlives us anyway.
                    Err(_) => collected.push(sid.clone()),
                }
            }
            for sid in collected {
                outstanding.remove(&sid);
            }
            if outstanding.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Retire the slot and announce why.
    async fn finish(
        &self,
        reason: CloseReason,
        on_event: &mut (impl FnMut(HostEvent) + Send),
    ) -> CloseReason {
        let _ = self.close().await;
        on_event(HostEvent::Closed { reason });
        reason
    }

    /// One long-poll of the sessions listing. `Ok(Err(reason))` means the slot is
    /// no longer ours to serve.
    async fn poll_sessions(
        &self,
        client: &reqwest::Client,
        wait_secs: u64,
    ) -> Result<Result<Vec<String>, CloseReason>> {
        let resp = client
            .get(format!(
                "{}?wait={}",
                rz_url(&self.relay, &self.slot, K_SESSIONS),
                wait_secs
            ))
            .bearer_auth(self.token())
            .send()
            .await
            .context("poll rendezvous sessions")?;
        match resp.status() {
            s if s.is_success() => {
                let body = resp.text().await.unwrap_or_default();
                Ok(Ok(body
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()))
            }
            reqwest::StatusCode::FORBIDDEN => Ok(Err(CloseReason::Taken)),
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE => {
                Ok(Err(CloseReason::Expired))
            }
            s => bail!("rendezvous relay refused the sessions poll: {s}"),
        }
    }

    /// Run one receiver's exchange to its end.
    async fn answer(
        &self,
        client: &reqwest::Client,
        sid: &str,
        payload: &[u8],
        emitted: &mut HashSet<Vec<u8>>,
        await_reply: bool,
    ) -> Result<Answer> {
        let mr = poll_get_for(
            client,
            &rz_url(&self.relay, &self.slot, &format!("{P_RECV}{sid}")),
            "the receiver's message",
            Some(&self.token()),
            SESSION_STEP_TIMEOUT,
        )
        .await?;

        // A fresh run per receiver: reusing one scalar across sessions is not
        // what SPAKE2 is safe for.
        let (pairing, ms) = pairing::start(&self.secret);

        // Reflection: our own message handed back to us. Answer so the session
        // stops showing up as pending, but never seal anything, and never charge
        // it to the guess budget.
        if emitted.contains(&mr) {
            let _ = self.put(client, &format!("{P_SEND}{sid}"), ms).await;
            return Ok(Answer::Rejected);
        }
        emitted.insert(ms.clone());
        self.put(client, &format!("{P_SEND}{sid}"), ms).await?;

        let pake_key = pairing.finish(&mr)?;
        let want = confirm_mac(&pake_key, &self.slot, sid);

        // The receiver proves it derived the same key before we seal anything.
        // A receiver that simply walks away is not a wrong code — only a wrong
        // answer is — so a timeout here is `Abandoned`, not `BadCode`.
        let Ok(got) = poll_get_for(
            client,
            &rz_url(&self.relay, &self.slot, &format!("{P_CONF}{sid}")),
            "the receiver's confirmation",
            Some(&self.token()),
            SESSION_STEP_TIMEOUT,
        )
        .await
        else {
            return Ok(Answer::Abandoned);
        };
        // `blake3::Hash` compares in constant time; raw slices would not.
        if got.len() != 32 || blake3::Hash::from_bytes(to_array32(&got)) != want {
            // Answer a wrong code with noise rather than silence. The receiver
            // fails at once on a payload it cannot open ("wrong code?") instead of
            // waiting out its whole timeout wondering. It costs nothing away: a
            // guesser learns only that it guessed wrong, which the silence told it
            // too, just slower — and the third wrong guess retires the code either
            // way, so a faster "no" buys no extra attempts.
            let noise: [u8; 48] = rand::random();
            let _ = self
                .put(client, &format!("{P_TKT}{sid}"), noise.to_vec())
                .await;
            return Ok(Answer::BadCode);
        }

        let key = ticket_key32(&pake_key, &self.slot, sid);
        let sealed = seal_chunk(&key, 0, 1, payload)?;
        self.put(client, &format!("{P_TKT}{sid}"), sealed).await?;

        // The other direction, when this is an exchange rather than a handover.
        // A receiver that never writes one is not an error: it may be an older
        // client, or simply have nothing to send, so this degrades to the
        // one-directional pairing that has always worked.
        let reply = if await_reply {
            match poll_get_for(
                client,
                &rz_url(&self.relay, &self.slot, &format!("{P_BACK}{sid}")),
                "the other side's reply",
                Some(&self.token()),
                REPLY_WAIT,
            )
            .await
            {
                Ok(ct) => {
                    let rkey = reply_key32(&pake_key, &self.slot, sid);
                    open_chunk(&rkey, 0, 1, &ct).ok()
                }
                Err(_) => None,
            }
        } else {
            None
        };
        Ok(Answer::Paired { reply })
    }

    async fn put(&self, client: &reqwest::Client, key: &str, value: Vec<u8>) -> Result<()> {
        client
            .post(rz_url(&self.relay, &self.slot, key))
            .bearer_auth(self.token())
            .body(value)
            .send()
            .await
            .context("post rendezvous value")?
            .error_for_status()
            .context("post rendezvous value")?;
        Ok(())
    }
}

/// How one receiver's exchange ended.
enum Answer {
    /// Confirmed and handed the payload, with the receiver's sealed reply if one
    /// was asked for and arrived.
    Paired { reply: Option<Vec<u8>> },
    /// Confirmed wrong — a guess. Counts against the code.
    BadCode,
    /// Refused without charging the guess budget (reflection).
    Rejected,
    /// The receiver never finished. Not a guess, so not charged either.
    Abandoned,
}

fn to_array32(b: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&b[..32]);
    out
}

/// Either kind of live sender, so callers can hold one without caring which
/// protocol the relay turned out to speak.
pub enum CodeSender {
    /// One-shot, in-memory: call `run()` and it serves exactly one receiver.
    V1(PairComplete),
    /// Long-lived and persistable: see [`CodeHost::run`].
    V2(CodeHost),
}

/// Publish `payload` under a fresh code, using v2 where the relay supports it and
/// falling back to the v1 exchange where it does not.
pub async fn publish_auto(
    payload: &[u8],
    relay: &str,
    embed_relay: bool,
) -> Result<(String, CodeSender)> {
    match relay_rz_version(relay).await {
        RzVersion::V2 => {
            let (shown, host) = claim_code(relay, embed_relay).await?;
            Ok((shown, CodeSender::V2(host)))
        }
        RzVersion::V1 => {
            let (shown, pc) = publish_bytes(payload.to_vec(), relay, embed_relay).await?;
            Ok((shown, CodeSender::V1(pc)))
        }
    }
}

/// Which protocol the slot behind a code speaks. Polls for whichever marker
/// appears first, so it also covers a receiver who is faster than the sender.
async fn detect_version(
    client: &reqwest::Client,
    relay: &str,
    slot: &str,
    timeout: Duration,
) -> Result<RzVersion> {
    let start = Instant::now();
    loop {
        for (key, version) in [(K_OWN, RzVersion::V2), (K_MS, RzVersion::V1)] {
            let resp = client
                .get(rz_url(relay, slot, key))
                .send()
                .await
                .context("rendezvous probe")?;
            if resp.status().is_success() {
                return Ok(version);
            }
            // A v2 slot answers 410 on `ms` so an old client fails fast; a new
            // client reads it as the v2 signal it is.
            if resp.status() == reqwest::StatusCode::GONE {
                return Ok(RzVersion::V2);
            }
        }
        if start.elapsed() > timeout {
            bail!("timed out waiting for the sender");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The v2 exchange, from the receiver's side: open a session of our own, prove we
/// know the code, then collect the payload sealed to that session alone.
async fn resolve_v2(
    client: &reqwest::Client,
    relay: &str,
    slot: &str,
    secret: &str,
    timeout: Duration,
    reply: Option<&[u8]>,
) -> Result<(Vec<u8>, bool)> {
    let sid = gen_sid();
    let (pairing, mr) = pairing::start(secret);
    client
        .post(rz_url(relay, slot, &format!("{P_RECV}{sid}")))
        .body(mr)
        .send()
        .await
        .context("open pairing session")?
        .error_for_status()
        .context("open pairing session")?;

    let ms = poll_get_for(
        client,
        &rz_url(relay, slot, &format!("{P_SEND}{sid}")),
        "the sender",
        None,
        timeout,
    )
    .await?;
    let pake_key = pairing.finish(&ms)?;

    // Prove we derived the same key. The sender seals nothing until it has seen
    // this, and counts a wrong one against the code.
    client
        .post(rz_url(relay, slot, &format!("{P_CONF}{sid}")))
        .body(confirm_mac(&pake_key, slot, &sid).as_bytes().to_vec())
        .send()
        .await
        .context("post key confirmation")?
        .error_for_status()
        .context("post key confirmation")?;

    // Capped well below the caller's overall deadline: once we have confirmed,
    // the sender either seals within moments or is not going to. Waiting out a
    // long deadline here would only turn "that sender is gone" into a silence.
    let ct = poll_get_for(
        client,
        &rz_url(relay, slot, &format!("{P_TKT}{sid}")),
        "the ticket",
        None,
        timeout.min(TICKET_WAIT),
    )
    .await
    .context("the sender never handed over the file — wrong code, or it stopped serving")?;
    let key = ticket_key32(&pake_key, slot, &sid);
    let payload = open_chunk(&key, 0, 1, &ct).context("decrypt ticket (wrong code?)")?;

    // Seal our half back, if we have one. Posted *after* opening the payload, so a
    // wrong code never writes anything: only a side that could decrypt has proved
    // it belongs here. A relay without the reply key answers 4xx; that is reported
    // rather than raised, because whether a one-way outcome is acceptable is the
    // caller's call, not this function's.
    let mut replied = false;
    if let Some(body) = reply {
        let rkey = reply_key32(&pake_key, slot, &sid);
        let sealed = seal_chunk(&rkey, 0, 1, body)?;
        replied = client
            .post(rz_url(relay, slot, &format!("{P_BACK}{sid}")))
            .body(sealed)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
    }
    Ok((payload, replied))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_and_self_contained() {
        let (np, secret, relay) = parse_code("4821-crater-mango").unwrap();
        assert_eq!(np, "4821");
        assert_eq!(secret, "4821-crater-mango");
        assert_eq!(relay, None);

        let (np, secret, relay) = parse_code("7-fox-oak@https://relay.example.com:8787").unwrap();
        assert_eq!(np, "7");
        assert_eq!(secret, "7-fox-oak");
        assert_eq!(relay.as_deref(), Some("https://relay.example.com:8787"));
    }

    #[test]
    fn relay_scheme_roundtrips_through_a_code() {
        // Bare host defaults to https; the code embeds the compact form, and the
        // receiver normalizes it back to the same URL.
        let full = normalize_relay("relay.example.com", false);
        assert_eq!(full, "https://relay.example.com");
        assert_eq!(compact_relay(&full), "relay.example.com");
        assert_eq!(normalize_relay(&compact_relay(&full), false), full);

        // A plaintext relay keeps its scheme both in the code and after resolve.
        let http = normalize_relay("relay.local:8787", true);
        assert_eq!(http, "http://relay.local:8787");
        assert_eq!(compact_relay(&http), "http://relay.local:8787");
        assert_eq!(normalize_relay(&compact_relay(&http), false), http);

        // An explicit scheme is never rewritten by the flag.
        assert_eq!(
            normalize_relay("http://relay.local", false),
            "http://relay.local"
        );
    }

    #[test]
    fn parse_rejects_junk() {
        assert!(parse_code("nope").is_err());
        assert!(parse_code("12-onlyone").is_err());
    }

    #[test]
    fn discriminates_code_from_ticket() {
        assert!(looks_like_code("4821-crater-mango"));
        assert!(looks_like_code("7-fox-oak@http://127.0.0.1:8787"));
        assert!(!looks_like_code("arvcQCAIAEEAQCAAQAUZLBT2")); // ticket
        assert!(!looks_like_code("word-word-word")); // non-digit nameplate
    }

    #[test]
    fn gen_secret_shape() {
        let (np, secret) = gen_secret();
        assert!(np.chars().all(|c| c.is_ascii_digit()));
        assert!(secret.starts_with(&format!("{np}-")));
        assert_eq!(secret.matches('-').count(), 2);
    }

    #[test]
    fn matching_code_decrypts_wrong_code_does_not() {
        let ticket = b"arvc-the-real-ticket";
        // Both sides run SPAKE2; matching secret -> same key -> decrypt works.
        let (ps, ms) = pairing::start("4821-crater-mango");
        let (pr, mr) = pairing::start("4821-crater-mango");
        let ks = key32(&ps.finish(&mr).unwrap());
        let kr = key32(&pr.finish(&ms).unwrap());
        let ct = seal_chunk(&ks, 0, 1, ticket).unwrap();
        assert_eq!(open_chunk(&kr, 0, 1, &ct).unwrap(), ticket);

        // Wrong secret on the receiver -> different key -> cannot decrypt.
        let (ps, ms) = pairing::start("4821-crater-mango");
        let (pw, mw) = pairing::start("4821-wrong-word");
        let ks = key32(&ps.finish(&mw).unwrap());
        let kw = key32(&pw.finish(&ms).unwrap());
        let ct = seal_chunk(&ks, 0, 1, ticket).unwrap();
        assert!(open_chunk(&kw, 0, 1, &ct).is_err());
    }
}
