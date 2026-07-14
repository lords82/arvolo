//! Presence / inbox: reaching an **online** peer to propose a transfer.
//!
//! The ticket path ([`crate::flow`]) is capability-based: the receiver must
//! already hold a ticket. That can't express "someone wants to send *me* a file
//! right now". This module adds a signalling layer on top of the zero-knowledge
//! relay: a persistent client subscribes to its own **inbox** (a slot derived
//! from its public id) and others deposit an [`Offer`] there.
//!
//! An offer carries an `arvc…` ticket already **sealed to the recipient** (`--to`),
//! plus display metadata (name/size). The offer is HPKE-sealed in auth-mode to
//! the recipient, and that authenticated envelope (sender key included) is then
//! wrapped in an **anonymous outer seal** to the same recipient — a sealed-sender
//! construction, so the relay learns only *which slot* receives, never *who*
//! sends (no public id ever appears on the wire in the clear). On accept, the
//! recipient drives a normal [`crate::flow::recv_chunked`] with the embedded
//! ticket — the ticket is never shown to the user.
//!
//! Slot = `base32(blake3_derive_key("arvolo/inbox/slot/v1", pubkey))` — an opaque
//! stand-in for the raw public key (still linkable by anyone who knows the key).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::crypto::{open, open_anon, seal, seal_anon, Identity, PublicId, Sealed};
use crate::sync::SyncNote;

/// Domain separator for the inbox slot derivation.
const SLOT_CONTEXT: &str = "arvolo/inbox/slot/v1";
/// Domain separator for the presence-beacon slot derivation (distinct from the
/// inbox slot so the two can't be correlated by the derived value alone).
const PRESENCE_SLOT_CONTEXT: &str = "arvolo/presence/slot/v1";
/// AAD binding a sealed offer to its purpose (rejects cross-protocol reuse).
const OFFER_AAD: &[u8] = b"arvolo/offer/v1";
/// AAD for the anonymous **outer** envelope seal (the sealed-sender layer).
const ENVELOPE_AAD: &[u8] = b"arvolo/offer/env/v1";

/// The inbox slot for a public id: an opaque, domain-separated hash of its bytes.
pub fn slot_for(pubkey: &[u8]) -> String {
    let key = blake3::derive_key(SLOT_CONTEXT, pubkey);
    data_encoding::BASE32_NOPAD.encode(&key).to_lowercase()
}

/// The presence-beacon slot for a public id (distinct from its inbox slot).
pub fn presence_slot_for(pubkey: &[u8]) -> String {
    let key = blake3::derive_key(PRESENCE_SLOT_CONTEXT, pubkey);
    data_encoding::BASE32_NOPAD.encode(&key).to_lowercase()
}

/// A proposed transfer, sealed to the recipient inside an `Envelope`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Offer {
    /// Suggested file/bundle name (display + default output name).
    pub name: String,
    /// Plaintext total size in bytes (for the accept prompt / progress bar).
    pub size: u64,
    /// Number of chunks (informational).
    pub chunks: u64,
    /// The ticket to fetch the file, already sealed to the recipient (`--to`).
    /// Either a live `arvc…` ticket (driven by [`crate::flow::recv_chunked`] while
    /// the sender serves) or an offline `arvm…` mailbox ticket (driven by
    /// [`crate::flow::fetch_offline`] when the sender was offline). Never shown to
    /// the user; the accept path picks the fetcher by ticket type.
    pub ticket: String,
    /// An optional short note the sender attached (`send --to --note "…"`). Empty
    /// when none. It rides *inside* the HPKE-sealed offer, so it's E2E-encrypted and
    /// sender-authenticated exactly like the ticket — the relay never sees it.
    #[serde(default)]
    pub note: String,
    /// The sender's self-chosen display name (`arvolo name "…"`), or empty when
    /// unset. A *petname claim*: it rides inside the sealed, sender-authenticated
    /// offer (so it's bound to the sender's key and the relay never sees it), but it
    /// is attacker-controllable text — the receiver treats it as unverified and it
    /// never substitutes the fingerprint in any trust decision.
    #[serde(default)]
    pub sender_name: String,
}

/// The authenticated **inner** envelope: the sender's public key plus the
/// auth-mode HPKE seal of the offer (`open` needs the sender key to verify it).
/// It never travels bare: [`SealedEnvelope`] wraps it in an anonymous outer seal
/// so the relay never sees the sender's public id (sealed sender). Earlier
/// releases deposited this struct in the clear; [`decode_offer`] still accepts
/// that legacy form for offers already queued on a relay.
#[derive(Serialize, Deserialize)]
struct Envelope {
    sender: Vec<u8>,
    encapped_key: Vec<u8>,
    ciphertext: Vec<u8>,
}

/// The on-relay wire form of a deposited offer: an **anonymous** (base-mode)
/// HPKE seal, to the recipient, of the postcard-encoded [`Envelope`]. The relay
/// sees only the recipient slot and this opaque blob — who is sending stays
/// end-to-end encrypted, and the sender is still cryptographically verified by
/// the inner auth-mode layer once the recipient unwraps it.
#[derive(Serialize, Deserialize)]
struct SealedEnvelope {
    encapped_key: Vec<u8>,
    ciphertext: Vec<u8>,
}

/// One inbox row as returned by the relay's GET: the relay-assigned id (used to
/// ack/delete) and the opaque `Envelope` bytes.
#[derive(Serialize, Deserialize)]
pub struct InboxItem {
    pub id: String,
    pub blob: Vec<u8>,
}

/// Serialize inbox rows for the relay's GET response (the relay owns no wire
/// format of its own — this keeps it in `core` so both ends agree).
pub fn encode_inbox_items(items: &[InboxItem]) -> Result<Vec<u8>> {
    postcard::to_allocvec(items).context("encode inbox items")
}

// ---- read authentication (proof of possession) ----------------------------
//
// Anyone can *deposit* an offer (you must be reachable), but only the inbox
// owner may *read* or *delete* it — otherwise a stranger who knows your public
// id could drain your offers or map your presence. X25519 keys can't sign, so
// instead of a signature we prove possession of the slot's private key: the
// relay seals a random nonce to the owner's public key (base-mode HPKE); only
// the owner can open it and echo the nonce back. The relay binds that nonce to
// the slot with a keyed MAC and hands back a short-lived bearer token, so the
// handshake happens once per session, not per poll.

/// AAD binding a session challenge to its purpose.
const SESSION_AAD: &[u8] = b"arvolo/inbox/session/v1";

/// What `POST /v1/inbox/{slot}/session` returns: a nonce sealed to the owner,
/// plus the relay's MAC over `(slot, nonce, exp)` and the expiry. The client
/// opens the seal to recover the nonce and assembles a [`SessionToken`].
#[derive(Serialize, Deserialize)]
pub struct SessionChallenge {
    pub encapped_key: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub exp: u64,
    pub mac: Vec<u8>,
}

/// The bearer credential the client presents on authenticated inbox requests.
/// `nonce` proves it opened the seal; `mac`/`exp` let the relay verify it issued
/// this nonce for this slot — statelessly, without storing sessions.
#[derive(Serialize, Deserialize)]
pub struct SessionToken {
    pub nonce: Vec<u8>,
    pub exp: u64,
    pub mac: Vec<u8>,
}

/// Seal a proof-of-possession `nonce` to `recipient` for a session challenge
/// (base-mode HPKE with the session AAD). Called by the relay; keeps the AAD
/// encapsulated so both ends can't drift.
pub fn seal_session_nonce(recipient: &PublicId, nonce: &[u8]) -> Result<Sealed> {
    seal_anon(nonce, recipient, SESSION_AAD)
}

/// Encode a [`SessionChallenge`] for the relay's `/session` response.
pub fn encode_session_challenge(c: &SessionChallenge) -> Result<Vec<u8>> {
    postcard::to_allocvec(c).context("encode session challenge")
}

/// Decode a bearer token string (base32 of a postcard [`SessionToken`]) back into
/// its parts. `None` on any malformed input.
pub fn decode_session_token(bearer: &str) -> Option<(Vec<u8>, u64, Vec<u8>)> {
    let raw = data_encoding::BASE32_NOPAD
        .decode(bearer.trim().to_uppercase().as_bytes())
        .ok()?;
    let t: SessionToken = postcard::from_bytes(&raw).ok()?;
    Some((t.nonce, t.exp, t.mac))
}

/// An offer received and decrypted from the inbox, with its authenticated sender.
#[derive(Clone, Debug)]
pub struct ReceivedOffer {
    /// Relay-assigned id, used to [`InboxSubscription::ack`] the offer.
    pub id: String,
    /// The HPKE-authenticated sender (whoever holds this key's secret sealed it).
    pub sender: PublicId,
    /// The decrypted offer.
    pub offer: Offer,
}

/// The inbox path on a relay for a given slot.
fn inbox_url(relay: &str, slot: &str) -> String {
    format!("{}/v1/inbox/{slot}", relay.trim_end_matches('/'))
}

// ---- multi-device sync notes ----------------------------------------------
//
// A user's devices share one identity, hence one inbox slot. Address-book
// synchronization rides that slot as encrypted CRDT snapshots. A sync note is
// tagged with a magic prefix so the offer path can tell it apart from an
// [`Envelope`] (postcard is not self-describing) and leave it alone.

/// Magic prefix identifying an inbox blob as a sync note (Arvolo Sync Note v1).
const SYNC_NOTE_MAGIC: &[u8; 4] = b"ASN1";

/// Encode a [`SyncNote`] for deposit into an inbox slot (magic prefix + postcard).
pub fn encode_sync_note(note: &SyncNote) -> Result<Vec<u8>> {
    let mut out = SYNC_NOTE_MAGIC.to_vec();
    out.extend_from_slice(&postcard::to_allocvec(note).context("encode sync note")?);
    Ok(out)
}

/// Decode an inbox blob as a [`SyncNote`], or `None` if it isn't one (missing
/// magic prefix or malformed). Never confuses an offer for a sync note.
pub fn decode_sync_note(blob: &[u8]) -> Option<SyncNote> {
    let rest = blob.strip_prefix(&SYNC_NOTE_MAGIC[..])?;
    postcard::from_bytes(rest).ok()
}

/// Header carrying the base32 BLAKE3 hash of an offer's retract token (POST).
const POSTER_HASH_HEADER: &str = "x-arvolo-poster-hash";
/// Header carrying an offer's retract token itself (DELETE).
const POSTER_TOKEN_HEADER: &str = "x-arvolo-poster-token";

/// Handle to a deposited offer: its relay id and the secret token that lets the
/// poster later [`retract_offer`] it.
pub struct PostedOffer {
    pub id: String,
    pub poster_token: String,
}

/// A random 16-byte capability token, base32-encoded.
fn random_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    data_encoding::BASE32_NOPAD.encode(&b).to_lowercase()
}

/// Seal `offer` to `recipient` and deposit it in the recipient's inbox on `relay`.
/// `ttl_secs` sets how long the offer lives on the relay: `None` uses the relay's
/// short default (fine for a live `arvc` offer whose sender is actively serving);
/// an offline (`arvm`) offer passes the mailbox blob's TTL so the notification
/// survives until the recipient returns.
pub async fn post_offer(
    client: &reqwest::Client,
    relay: &str,
    recipient: &PublicId,
    me: &Identity,
    offer: &Offer,
    ttl_secs: Option<u64>,
) -> Result<PostedOffer> {
    let body = encode_offer_blob(offer, recipient, me)?;
    let slot = slot_for(&recipient.to_bytes());
    let mut url = inbox_url(relay, &slot);
    if let Some(ttl) = ttl_secs {
        url = format!("{url}?ttl={ttl}");
    }
    // A retract capability: keep the token, hand the relay only its hash.
    let poster_token = random_token();
    let poster_hash = data_encoding::BASE32_NOPAD
        .encode(blake3::hash(poster_token.as_bytes()).as_bytes())
        .to_lowercase();
    let resp = client
        .post(url)
        .header(POSTER_HASH_HEADER, poster_hash)
        .body(body)
        .send()
        .await
        .context("post offer to relay")?;
    if !resp.status().is_success() {
        anyhow::bail!("relay rejected offer: {}", resp.status());
    }
    let id = resp
        .text()
        .await
        .context("read offer id")?
        .trim()
        .to_string();
    Ok(PostedOffer { id, poster_token })
}

/// Retract an offer this client posted (by its id + poster token), e.g. a live
/// offer being superseded by an offline fallback. Best-effort: an already-gone
/// offer is not an error.
pub async fn retract_offer(
    client: &reqwest::Client,
    relay: &str,
    recipient: &PublicId,
    id: &str,
    poster_token: &str,
) -> Result<()> {
    let slot = slot_for(&recipient.to_bytes());
    let url = format!("{}/{id}", inbox_url(relay, &slot));
    client
        .delete(url)
        .header(POSTER_TOKEN_HEADER, poster_token)
        .send()
        .await
        .context("retract offer")?;
    Ok(())
}

/// Whether an offer we posted has been seen by (or handed off to) the recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferStatus {
    /// Still queued, not yet seen by a live recipient poll.
    Pending,
    /// A live recipient polled the inbox and received it.
    Fetched,
    /// No longer on the relay — the recipient acked/accepted it (or it expired).
    Gone,
}

/// Ask the relay whether an offer we posted has been seen yet (poster-authed).
/// Lets a live send tell "recipient is really online" from "stale presence".
pub async fn offer_status(
    client: &reqwest::Client,
    relay: &str,
    recipient: &PublicId,
    id: &str,
    poster_token: &str,
) -> Result<OfferStatus> {
    let slot = slot_for(&recipient.to_bytes());
    let url = format!("{}/{id}/status", inbox_url(relay, &slot));
    let resp = client
        .get(url)
        .header(POSTER_TOKEN_HEADER, poster_token)
        .send()
        .await
        .context("offer status")?
        .error_for_status()
        .context("relay rejected offer status")?;
    let body = resp.text().await.unwrap_or_default();
    Ok(match body.trim() {
        "fetched" => OfferStatus::Fetched,
        "gone" => OfferStatus::Gone,
        _ => OfferStatus::Pending,
    })
}

/// Publish (refresh) a presence beacon so contacts see `me` as online. A listening
/// client calls this periodically; the relay expires it after `PRESENCE_TTL`.
pub async fn publish_beacon(client: &reqwest::Client, relay: &str, me: &Identity) -> Result<()> {
    let slot = presence_slot_for(&me.public().to_bytes());
    let url = format!("{}/v1/presence/{slot}", relay.trim_end_matches('/'));
    let resp = client
        .post(url)
        .send()
        .await
        .context("publish presence beacon")?;
    if !resp.status().is_success() {
        anyhow::bail!("relay rejected beacon: {}", resp.status());
    }
    Ok(())
}

/// Is `contact` currently online (a live presence beacon on `relay`)? A network
/// error is reported as an error; a plain "no beacon" is `Ok(false)`.
pub async fn check_online(
    client: &reqwest::Client,
    relay: &str,
    contact: &PublicId,
) -> Result<bool> {
    let slot = presence_slot_for(&contact.to_bytes());
    let url = format!("{}/v1/presence/{slot}", relay.trim_end_matches('/'));
    let resp = client.get(url).send().await.context("check presence")?;
    Ok(resp.status().is_success())
}

/// A live subscription to *my* inbox on a relay. Long-polls for offers and
/// decrypts them with my identity; reading and acking are authenticated with a
/// proof-of-possession bearer token, cached and refreshed transparently.
pub struct InboxSubscription {
    client: reqwest::Client,
    relay: String,
    slot: String,
    /// Owner's identity secret — needed to open the session challenge. Held here
    /// (not passed per call) so `poll`/`ack`/`run` can authenticate on their own.
    me_secret: Vec<u8>,
    session: std::sync::Mutex<Option<CachedSession>>,
}

#[derive(Clone)]
struct CachedSession {
    bearer: String,
    exp: u64,
}

/// Seconds the relay may hold a GET open waiting for an offer (long-poll).
const LONG_POLL_SECS: u64 = 25;

/// Current unix time in seconds.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl InboxSubscription {
    /// Subscribe to the inbox of `me` on `relay`.
    pub fn new(relay: impl Into<String>, me: &Identity) -> Self {
        Self {
            client: reqwest::Client::new(),
            relay: relay.into(),
            slot: slot_for(&me.public().to_bytes()),
            me_secret: me.secret_bytes(),
            session: std::sync::Mutex::new(None),
        }
    }

    /// Rebuild the owner's identity from the stored secret.
    fn me(&self) -> Result<Identity> {
        Identity::from_secret_bytes(&self.me_secret)
    }

    /// Return a valid bearer token, running the proof-of-possession handshake if
    /// the cache is empty or the token is within 60s of expiry. `force` skips the
    /// cache (used after a 401, in case the relay's session secret rotated).
    async fn ensure_session(&self, force: bool) -> Result<String> {
        if !force {
            if let Some(s) = self.session.lock().unwrap().clone() {
                if now_unix() + 60 < s.exp {
                    return Ok(s.bearer);
                }
            }
        }
        let me = self.me()?;
        let url = format!("{}/session", inbox_url(&self.relay, &self.slot));
        let body = me.public().to_bytes();
        let resp = self
            .client
            .post(&url)
            .body(body)
            .send()
            .await
            .context("request inbox session")?
            .error_for_status()
            .context("relay rejected inbox session")?;
        let bytes = resp.bytes().await.context("read session challenge")?;
        let challenge: SessionChallenge =
            postcard::from_bytes(&bytes).context("decode session challenge")?;
        // Open the sealed nonce — only our private key can, which is the proof.
        let sealed = Sealed {
            encapped_key: challenge.encapped_key,
            ciphertext: challenge.ciphertext,
        };
        let nonce = open_anon(&sealed, &me, SESSION_AAD).context("open session challenge")?;
        let token = SessionToken {
            nonce,
            exp: challenge.exp,
            mac: challenge.mac,
        };
        let raw = postcard::to_allocvec(&token).context("encode session token")?;
        let bearer = data_encoding::BASE32_NOPAD.encode(&raw).to_lowercase();
        *self.session.lock().unwrap() = Some(CachedSession {
            bearer: bearer.clone(),
            exp: challenge.exp,
        });
        Ok(bearer)
    }

    /// One long-poll round with the default `LONG_POLL_SECS` hold time.
    pub async fn poll(&self) -> Result<Vec<ReceivedOffer>> {
        self.poll_wait(LONG_POLL_SECS).await
    }

    /// One poll round holding the connection up to `wait_secs` (0 = return
    /// immediately). Returns any offers currently queued, decrypted and
    /// authenticated with our identity; an empty vec on a quiet inbox. Undecodable
    /// or unauthenticated rows are dropped from our view and acked so they stop
    /// coming back, rather than failing the whole poll.
    pub async fn poll_wait(&self, wait_secs: u64) -> Result<Vec<ReceivedOffer>> {
        let url = format!("{}?wait={}", inbox_url(&self.relay, &self.slot), wait_secs);
        let bytes = self.authed(reqwest::Method::GET, &url).await?;
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        let items: Vec<InboxItem> = postcard::from_bytes(&bytes).context("decode inbox items")?;
        let me = self.me()?;
        let mut out = Vec::new();
        for item in items {
            // A sync note shares our slot but is not an offer — leave it on the
            // relay for the sync engine and never ack it here (only the sync
            // writer clears these; acking would strip other devices' updates).
            if decode_sync_note(&item.blob).is_some() {
                continue;
            }
            match decode_offer(&item.blob, &me) {
                Some((sender, offer)) => out.push(ReceivedOffer {
                    id: item.id,
                    sender,
                    offer,
                }),
                // Junk or an offer we can't authenticate: drop it from our view and
                // ack it so it stops coming back.
                None => {
                    let _ = self.ack(&item.id).await;
                }
            }
        }
        Ok(out)
    }

    /// Fetch the raw inbox items (authenticated), holding the connection up to
    /// `wait_secs`. Used by the sync engine, which needs the relay-assigned ids to
    /// clean up superseded sync notes.
    pub async fn raw_items(&self, wait_secs: u64) -> Result<Vec<InboxItem>> {
        let url = format!("{}?wait={}", inbox_url(&self.relay, &self.slot), wait_secs);
        let bytes = self.authed(reqwest::Method::GET, &url).await?;
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        postcard::from_bytes(&bytes).context("decode inbox items")
    }

    /// Deposit a raw blob into **our own** inbox slot (unauthenticated POST, like
    /// any depositor). `ttl_secs` sets how long the relay keeps it — the sync cell
    /// uses a long TTL and is refreshed on each publish. Returns the relay id.
    pub async fn post_raw(&self, body: Vec<u8>, ttl_secs: Option<u64>) -> Result<String> {
        let mut url = inbox_url(&self.relay, &self.slot);
        if let Some(ttl) = ttl_secs {
            url = format!("{url}?ttl={ttl}");
        }
        let resp = self
            .client
            .post(&url)
            .body(body)
            .send()
            .await
            .context("post to inbox")?;
        if !resp.status().is_success() {
            anyhow::bail!("relay rejected inbox post: {}", resp.status());
        }
        Ok(resp
            .text()
            .await
            .context("read post id")?
            .trim()
            .to_string())
    }

    /// Delete a handled offer from the inbox by its id.
    pub async fn ack(&self, id: &str) -> Result<()> {
        let url = format!("{}/{id}", inbox_url(&self.relay, &self.slot));
        self.authed(reqwest::Method::DELETE, &url).await?;
        Ok(())
    }

    /// Perform an authenticated inbox request, attaching the bearer token and
    /// re-authenticating once on a 401 (e.g. the relay restarted with a new
    /// session secret). Returns the response body bytes.
    async fn authed(&self, method: reqwest::Method, url: &str) -> Result<Vec<u8>> {
        let mut forced = false;
        loop {
            let bearer = self.ensure_session(forced).await?;
            let resp = self
                .client
                .request(method.clone(), url)
                .bearer_auth(&bearer)
                .send()
                .await
                .context("inbox request")?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && !forced {
                // Stale token — drop it and retry the handshake once.
                *self.session.lock().unwrap() = None;
                forced = true;
                continue;
            }
            let resp = resp
                .error_for_status()
                .context("relay rejected inbox request")?;
            let bytes = resp.bytes().await.context("read inbox response")?;
            return Ok(bytes.to_vec());
        }
    }

    /// Long-poll until `cancel` fires, invoking `on` for each received offer.
    /// Transient poll errors are swallowed and retried after a short backoff so a
    /// flaky relay doesn't tear the subscription down.
    pub async fn run(&self, cancel: CancellationToken, on: impl Fn(ReceivedOffer) + Send + Sync) {
        // Offers stay on the relay until the caller acks them (on accept/reject),
        // so a client restart re-surfaces anything unhandled. But within one run,
        // a polled-yet-unacked offer would be returned again on the next round —
        // dedupe by id so each offer is surfaced to the caller exactly once.
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Floor on how often a round may complete. The relay's long-poll returns
        // *immediately* whenever the slot is non-empty — and a slot holding a
        // durable non-offer blob (the contact-sync cell) is non-empty forever, so
        // without this floor the loop degenerates into a zero-delay GET storm
        // (observed: >700 req/s per client, pegging the relay and its access
        // logs). A quiet inbox still long-polls the full hold time and never
        // waits here.
        const MIN_ROUND: std::time::Duration = std::time::Duration::from_secs(2);
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let round_started = std::time::Instant::now();
            tokio::select! {
                _ = cancel.cancelled() => return,
                res = self.poll() => match res {
                    Ok(offers) => {
                        // Acked offers never return, so `seen` only needs to cover
                        // live ones; bound it so a long-lived session can't grow it
                        // without limit (a rare reset just risks re-surfacing an
                        // offer, which the caller already dedupes by pending id).
                        if seen.len() > 4096 {
                            seen.clear();
                        }
                        for o in offers {
                            if seen.insert(o.id.clone()) {
                                on(o);
                            }
                        }
                    }
                    Err(_) => {
                        tokio::select! {
                            _ = cancel.cancelled() => return,
                            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
                        }
                    }
                },
            }
            let elapsed = round_started.elapsed();
            if elapsed < MIN_ROUND {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(MIN_ROUND - elapsed) => {}
                }
            }
        }
    }
}

/// Seal `offer` to `recipient` as `me` in its on-relay wire form: inner
/// auth-mode seal (sender-verifying), then the anonymous outer envelope so the
/// relay never sees the sender's public id.
fn encode_offer_blob(offer: &Offer, recipient: &PublicId, me: &Identity) -> Result<Vec<u8>> {
    let plaintext = postcard::to_allocvec(offer).context("encode offer")?;
    let sealed = seal(&plaintext, recipient, me, OFFER_AAD).context("seal offer")?;
    let env = Envelope {
        sender: me.public().to_bytes(),
        encapped_key: sealed.encapped_key,
        ciphertext: sealed.ciphertext,
    };
    let env_bytes = postcard::to_allocvec(&env).context("encode envelope")?;
    let outer = seal_anon(&env_bytes, recipient, ENVELOPE_AAD).context("seal envelope")?;
    let wire = SealedEnvelope {
        encapped_key: outer.encapped_key,
        ciphertext: outer.ciphertext,
    };
    postcard::to_allocvec(&wire).context("encode sealed envelope")
}

/// Decode + open one inbox blob with `me`, returning the authenticated sender and
/// offer. `None` if the row is malformed or fails authentication/decryption.
fn decode_offer(blob: &[u8], me: &Identity) -> Option<(PublicId, Offer)> {
    // Sealed-sender wire form: anonymous outer seal, then the inner envelope.
    // Fall back to the legacy clear-sender envelope (pre-sealed-sender clients /
    // offers already queued on a relay); the inner auth-mode open is identical.
    let env: Envelope = match postcard::from_bytes::<SealedEnvelope>(blob)
        .ok()
        .and_then(|wire| {
            let outer = Sealed {
                encapped_key: wire.encapped_key,
                ciphertext: wire.ciphertext,
            };
            open_anon(&outer, me, ENVELOPE_AAD).ok()
        })
        .and_then(|env_bytes| postcard::from_bytes(&env_bytes).ok())
    {
        Some(env) => env,
        None => postcard::from_bytes(blob).ok()?,
    };
    let sender = PublicId::from_bytes(&env.sender).ok()?;
    let sealed = Sealed {
        encapped_key: env.encapped_key,
        ciphertext: env.ciphertext,
    };
    let plaintext = open(&sealed, me, &sender, OFFER_AAD).ok()?;
    let offer: Offer = postcard::from_bytes(&plaintext).ok()?;
    Some((sender, offer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_is_stable_and_key_specific() {
        let a = Identity::generate();
        let b = Identity::generate();
        assert_eq!(
            slot_for(&a.public().to_bytes()),
            slot_for(&a.public().to_bytes())
        );
        assert_ne!(
            slot_for(&a.public().to_bytes()),
            slot_for(&b.public().to_bytes())
        );
        // The presence slot is domain-separated from the inbox slot for the same key.
        assert_ne!(
            slot_for(&a.public().to_bytes()),
            presence_slot_for(&a.public().to_bytes())
        );
    }

    fn test_offer() -> Offer {
        Offer {
            name: "photo.jpg".into(),
            size: 1234,
            chunks: 1,
            ticket: "arvc-fake".into(),
            note: "see page 4".into(),
            sender_name: "Lorenzo".into(),
        }
    }

    #[test]
    fn offer_round_trips_through_sealed_envelope() {
        let sender = Identity::generate();
        let recipient = Identity::generate();
        let offer = test_offer();
        let blob = encode_offer_blob(&offer, &recipient.public(), &sender).unwrap();

        let (got_sender, got_offer) = decode_offer(&blob, &recipient).expect("decode");
        assert_eq!(got_sender.to_bytes(), sender.public().to_bytes());
        assert_eq!(got_offer, offer);

        // A different recipient can't open it.
        let other = Identity::generate();
        assert!(decode_offer(&blob, &other).is_none());
    }

    #[test]
    fn sealed_envelope_hides_the_sender_from_the_relay() {
        let sender = Identity::generate();
        let recipient = Identity::generate();
        let blob = encode_offer_blob(&test_offer(), &recipient.public(), &sender).unwrap();

        // The relay-visible blob must not contain the sender's public id anywhere
        // (that is the whole point of the sealed-sender outer layer).
        let pk = sender.public().to_bytes();
        assert!(
            !blob.windows(pk.len()).any(|w| w == &pk[..]),
            "sender public id must not appear in the on-relay blob"
        );
        // And it must never be mistaken for a sync note.
        assert!(decode_sync_note(&blob).is_none());
    }

    #[test]
    fn legacy_clear_sender_envelope_still_decodes() {
        // Offers deposited by pre-sealed-sender clients (bare `Envelope` on the
        // wire) must remain readable so queued offers survive an upgrade.
        let sender = Identity::generate();
        let recipient = Identity::generate();
        let offer = test_offer();
        let plaintext = postcard::to_allocvec(&offer).unwrap();
        let sealed = seal(&plaintext, &recipient.public(), &sender, OFFER_AAD).unwrap();
        let env = Envelope {
            sender: sender.public().to_bytes(),
            encapped_key: sealed.encapped_key,
            ciphertext: sealed.ciphertext,
        };
        let blob = postcard::to_allocvec(&env).unwrap();

        let (got_sender, got_offer) = decode_offer(&blob, &recipient).expect("legacy decode");
        assert_eq!(got_sender.to_bytes(), sender.public().to_bytes());
        assert_eq!(got_offer, offer);
    }
}
