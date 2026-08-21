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
//! An inbox slot is not a hash of the public id but a **per-epoch blinded public
//! key** derived from it (see [`crate::crypto::blinded`]): a sender computes it from
//! the contact id, the owner can sign for it, and the relay — which authenticates
//! that signature against the slot itself — never learns whose inbox it is holding.
//! Slots rotate; see [`INBOX_EPOCH_SECS`].

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::crypto::{open, open_anon, seal, seal_anon, Identity, PublicId, Sealed};
use crate::sync::SyncNote;

/// Domain separator for the presence-beacon slot derivation. Only the beacon hashes
/// its way to a slot now — an inbox slot is a blinded key (see [`slot_for`]) — so
/// there is no inbox context to be distinct from any more.
const PRESENCE_SLOT_CONTEXT: &str = "arvolo/presence/slot/v2";
/// AAD binding a sealed offer to its purpose (rejects cross-protocol reuse).
const OFFER_AAD: &[u8] = b"arvolo/offer/v1";
/// AAD for the anonymous **outer** envelope seal (the sealed-sender layer).
const ENVELOPE_AAD: &[u8] = b"arvolo/offer/env/v1";

// ---- slots: what they are, and why they rotate ----------------------------
//
// An inbox slot is a **per-epoch blinded public key** derived from the owner's
// signing half (`crate::crypto::blinded`). Two properties make the whole scheme
// work, and they pull in opposite directions until blinding reconciles them:
//
// - a *sender* must be able to compute it, knowing only the contact id, or nobody
//   could address you;
// - the *relay* must not be able to tie one epoch's slot to another's, or rotating
//   them would be decoration.
//
// Blinding gives both: `A' = h·A` is computable by anyone holding `A`, and reveals
// nothing about `A` to someone who does not.
//
// The rotation is what keeps a stable identifier out of the request path — and so
// out of the relay's access log, and out of the log of any reverse proxy in front of
// it. Before it, one unchanging string named you there for the life of your
// identity: enough to group your whole history without knowing who you were, and it
// outlived any change of IP.
//
// The earlier version of this hashed the public id instead, and had a hole it could
// not close: reading an inbox meant POSTing your public key to the relay so it could
// seal a challenge to it. The relay therefore learned the long-term identity of
// every reader, and from it could recompute every slot that identity would ever have
// — so the rotation protected the logs and nothing else. The slot carrying its own
// key is what closed that: see the proof-of-possession section below.

/// How long one inbox slot lasts before it rotates.
///
/// The floor on this is how long an offer must stay findable: a reader looks in the
/// current epoch's slot and the previous one, so an offer is reachable for at least
/// one full epoch after it is posted. [`post_offer`] clamps its TTL to exactly that,
/// which makes the invariant total — a *live* offer is never in a slot nobody reads
/// — and it is also why this matches the sync cell's own 7-day TTL, the longest
/// thing the inbox is asked to hold.
pub const INBOX_EPOCH_SECS: u64 = 7 * 24 * 3600;

/// How long one presence slot lasts. Far shorter than the inbox's, because a beacon
/// lives 30 seconds: nothing needs to be findable across a boundary, so the value
/// can turn over hourly and the reader's fallback (below) stays cheap.
pub const PRESENCE_EPOCH_SECS: u64 = 3600;

/// How long after a presence-epoch boundary the previous slot may still hold a live
/// beacon. The relay expires beacons after 30s; two minutes is slack for clock skew
/// between the publisher and the asker, and it is the *only* window in which
/// [`check_online`] spends a second request.
const PRESENCE_GRACE_SECS: u64 = 120;

fn derive_slot(context: &str, pubkey: &[u8], epoch: u64) -> String {
    let mut input = Vec::with_capacity(pubkey.len() + 8);
    input.extend_from_slice(pubkey);
    input.extend_from_slice(&epoch.to_le_bytes());
    let key = blake3::derive_key(context, &input);
    data_encoding::BASE32_NOPAD.encode(&key).to_lowercase()
}

/// Encode a slot: the base32 of a 32-byte blinded key. The same shape the
/// hash-derived slots had, so nothing downstream notices the change of meaning.
fn encode_slot(key: &[u8; crate::crypto::blinded::KEY_LEN]) -> String {
    data_encoding::BASE32_NOPAD.encode(key).to_lowercase()
}

/// The key a slot names, or `None` if it is not a slot at all.
///
/// This is the relay's way in, and the reason the handshake no longer carries a
/// public key: the slot *is* the key it authenticates against, so the relay can
/// check ownership without ever learning whose inbox it is.
pub fn slot_key(slot: &str) -> Option<Vec<u8>> {
    data_encoding::BASE32_NOPAD
        .decode(slot.trim().to_uppercase().as_bytes())
        .ok()
        .filter(|k| k.len() == crate::crypto::blinded::KEY_LEN)
}

/// The inbox slot of `id` for a given epoch: its signing half, blinded.
fn slot_for_epoch(id: &PublicId, epoch: u64) -> String {
    encode_slot(&crate::crypto::blinded::public_at(id, epoch))
}

/// The inbox slot for an identity at a given time.
pub fn slot_for_at(id: &PublicId, unix: u64) -> String {
    slot_for_epoch(id, unix / INBOX_EPOCH_SECS)
}

/// The inbox slot for an identity **now** — the one a depositor posts to.
pub fn slot_for(id: &PublicId) -> String {
    slot_for_at(id, now_unix())
}

/// Every inbox slot that can still hold a live row for `id` at `unix`, current
/// epoch first: what a reader must look in.
///
/// There is no matching "does this slot belong to this key" for the relay any more,
/// and its absence is the point of the change. The relay used to be handed a public
/// key and asked to check it hashed to the slot — which is how it came to know every
/// reader's long-term identity, and with it every slot that identity would ever
/// have. Now the slot carries its own key and a signature settles ownership, so the
/// relay has nothing to compare and no epoch of its own to be right about. The
/// clock-skew tolerance that check needed went with it.
pub fn inbox_slots_at(id: &PublicId, unix: u64) -> Vec<String> {
    let epoch = unix / INBOX_EPOCH_SECS;
    let mut out = Vec::with_capacity(2);
    for n in 0..=1u64 {
        let s = slot_for_epoch(id, epoch.saturating_sub(n));
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// The presence-beacon slot for a public id at a given time (distinct from its
/// inbox slot).
pub fn presence_slot_for_at(pubkey: &[u8], unix: u64) -> String {
    derive_slot(PRESENCE_SLOT_CONTEXT, pubkey, unix / PRESENCE_EPOCH_SECS)
}

/// The presence-beacon slot for a public id **now** — the one a beacon publishes to.
pub fn presence_slot_for(pubkey: &[u8]) -> String {
    presence_slot_for_at(pubkey, now_unix())
}

/// Presence slots worth asking about at `unix`: the current one, plus a neighbour
/// near an epoch boundary.
///
/// Which neighbour depends on which side of the boundary we are on, and both cases
/// are about the *publisher's* clock, not ours. Just after a boundary, a beacon
/// published seconds ago is still in the epoch that just ended. Just before one, a
/// publisher whose clock runs slightly fast has already moved to the next. Either
/// way it is one extra request, only inside a two-minute window, and only if the
/// first slot came up empty.
pub fn presence_slots_at(pubkey: &[u8], unix: u64) -> Vec<String> {
    let epoch = unix / PRESENCE_EPOCH_SECS;
    let into = unix % PRESENCE_EPOCH_SECS;
    let mut out = vec![derive_slot(PRESENCE_SLOT_CONTEXT, pubkey, epoch)];
    if into < PRESENCE_GRACE_SECS {
        out.push(derive_slot(
            PRESENCE_SLOT_CONTEXT,
            pubkey,
            epoch.saturating_sub(1),
        ));
    } else if PRESENCE_EPOCH_SECS - into <= PRESENCE_GRACE_SECS {
        out.push(derive_slot(PRESENCE_SLOT_CONTEXT, pubkey, epoch + 1));
    }
    out.dedup();
    out
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
    /// Which **device** posted this offer, when it knows its own id.
    ///
    /// Devices paired with `device pair` share one identity, so they also share a
    /// public key, an inbox slot, and therefore a [`Envelope::sender`]: nothing in
    /// an offer distinguishes "I posted this" from "my other device posted this".
    /// That is fine between two people and fatal between two of your own machines
    /// — the sender would find its own offer waiting in its own inbox and, since
    /// an offer from your own identity is auto-accepted, download the file over
    /// itself. This is the one field that tells them apart.
    ///
    /// A local tiebreak, not an identity: it is the same random
    /// [`DeviceId`](crate::sync::DeviceId) the CRDT clock uses, it never leaves
    /// the HPKE seal (so the relay cannot correlate devices by it), and it is
    /// never shown to anyone. `None` from a client that predates it, which simply
    /// means the filter cannot help there.
    #[serde(default)]
    pub origin: Option<crate::sync::DeviceId>,
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
// Anyone can *deposit* an offer (you must be reachable), but only the inbox owner
// may *read* or *delete* it — otherwise a stranger who knows your public id could
// drain your offers or map your presence.
//
// The slot is a per-epoch blinded public key (see [`crate::crypto::blinded`]), so
// the proof is a signature: the relay decodes the slot, issues a random nonce, and
// checks the signature against the slot itself. Nothing identifies the owner, and no
// key is transmitted — the relay authenticates a reader it cannot name.
//
// It used to be an HPKE round trip instead: the client sent its public key, the
// relay sealed a nonce to it, and opening the seal was the proof. That worked, and
// it meant the relay learned every reader's long-term identity — and with it every
// slot that identity would ever have, in any epoch, which made rotating the slots
// pointless against the relay itself. The nonce still exists for freshness; only the
// part that named you is gone.
//
// The relay binds the nonce to the slot with a keyed MAC, so it can verify it issued
// this challenge without storing sessions, and the client presents the signed nonce
// as a short-lived bearer token — the handshake happens once per session, not per
// poll.

/// Domain separator for what a session signature covers.
const SESSION_SIG_CONTEXT: &[u8] = b"arvolo/inbox/session/v2";

/// What `POST /v1/inbox/{slot}/session` returns: a random nonce, the relay's MAC
/// over `(slot, nonce, exp)`, and the expiry. Public — the security is in the
/// signature the client must produce over it, not in hiding the challenge.
#[derive(Serialize, Deserialize)]
pub struct SessionChallenge {
    pub nonce: Vec<u8>,
    pub exp: u64,
    pub mac: Vec<u8>,
}

/// The bearer credential the client presents on authenticated inbox requests.
/// `sig` proves possession of the slot's blinded secret; `mac`/`exp` let the relay
/// verify it issued this nonce for this slot — statelessly, without storing
/// sessions.
#[derive(Serialize, Deserialize)]
pub struct SessionToken {
    pub nonce: Vec<u8>,
    pub exp: u64,
    pub mac: Vec<u8>,
    pub sig: Vec<u8>,
}

/// The exact bytes a session signature covers: the context, the slot, the nonce and
/// the expiry. Both ends build it here so they cannot drift, and every field is
/// bound — a signature cannot be moved to another slot, nonce or lifetime.
pub fn session_signing_input(slot: &str, nonce: &[u8], exp: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(SESSION_SIG_CONTEXT.len() + slot.len() + nonce.len() + 8);
    out.extend_from_slice(SESSION_SIG_CONTEXT);
    out.extend_from_slice(slot.as_bytes());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&exp.to_le_bytes());
    out
}

/// Encode a [`SessionChallenge`] for the relay's `/session` response.
pub fn encode_session_challenge(c: &SessionChallenge) -> Result<Vec<u8>> {
    postcard::to_allocvec(c).context("encode session challenge")
}

/// Decode a bearer token string (base32 of a postcard [`SessionToken`]) back into
/// its parts. `None` on any malformed input.
pub fn decode_session_token(bearer: &str) -> Option<SessionToken> {
    let raw = data_encoding::BASE32_NOPAD
        .decode(bearer.trim().to_uppercase().as_bytes())
        .ok()?;
    postcard::from_bytes(&raw).ok()
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

/// Clamp an inbox TTL to the slot's own lifetime.
///
/// A row that outlives the window its slot is read in is worse than no row: it sits
/// on the relay, counts against the slot's cap, and is invisible to the one client
/// entitled to it. So the notification expires with its slot. Nothing is lost that
/// the caller can't recover — an offline offer's blob keeps the TTL it was deposited
/// with, and the sender's `arvm…` ticket still fetches it by hand.
fn clamp_inbox_ttl(ttl_secs: Option<u64>) -> Option<u64> {
    ttl_secs.map(|t| t.min(INBOX_EPOCH_SECS))
}

/// Seal `offer` to `recipient` and deposit it in the recipient's inbox on `relay`.
/// `ttl_secs` sets how long the offer lives on the relay: `None` uses the relay's
/// short default (fine for a live `arvc` offer whose sender is actively serving);
/// an offline (`arvm`) offer passes the mailbox blob's TTL so the notification
/// survives until the recipient returns — clamped to [`INBOX_EPOCH_SECS`], since a
/// notification cannot usefully outlive the slot it was posted in.
pub async fn post_offer(
    client: &reqwest::Client,
    relay: &str,
    recipient: &PublicId,
    me: &Identity,
    offer: &Offer,
    ttl_secs: Option<u64>,
) -> Result<PostedOffer> {
    let body = encode_offer_blob(offer, recipient, me)?;
    let slot = slot_for(recipient);
    let mut url = inbox_url(relay, &slot);
    if let Some(ttl) = clamp_inbox_ttl(ttl_secs) {
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
///
/// The slot is not recomputed and trusted: an offer posted before an epoch boundary
/// lives in the *previous* epoch's slot, so this walks the readable window and stops
/// at the one that answers. The TTL clamp in [`clamp_inbox_ttl`] is what guarantees
/// the window still contains it while it is alive.
pub async fn retract_offer(
    client: &reqwest::Client,
    relay: &str,
    recipient: &PublicId,
    id: &str,
    poster_token: &str,
) -> Result<()> {
    let mut last = None;
    for slot in inbox_slots_at(recipient, now_unix()) {
        let url = format!("{}/{id}", inbox_url(relay, &slot));
        match client
            .delete(url)
            .header(POSTER_TOKEN_HEADER, poster_token)
            .send()
            .await
        {
            // A wrong slot fails the poster check and falls through to the session
            // check, which we don't satisfy: 401. Only a success means it was ours.
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(_) => {}
            Err(e) => last = Some(e),
        }
    }
    match last {
        Some(e) => Err(anyhow::Error::new(e).context("retract offer")),
        None => Ok(()),
    }
}

/// How far an offer we posted has got.
///
/// A ladder, not a pair of flags: `Pending` → `Arrived` → `Taken`, with `Gone` off
/// to the side for the offers that ended without ever being taken. The distinction
/// that matters is between the middle two. `Arrived` is a fact about a *machine* —
/// some client of theirs read the offer, which a `recv`/`status` listing does as
/// much as a daemon poll — and for a while it was the best news available, so it
/// got read as "they have the file". `Taken` is the fact about the *person*: they
/// fetched it and acked. Keeping them apart is what stops a glance at a list from
/// being reported as a delivery.
///
/// There is a fourth thing one might want — *did a human look at it?* — and it is
/// deliberately absent, because nothing here can answer it. The relay sees reads of
/// a slot, not eyes on a screen; only the recipient's own client could claim it,
/// and no such claim is made. So no name on this ladder may suggest a person: what
/// the middle state knows is that the offer arrived somewhere, and `Arrived` is the
/// most it can say. (`Seen` was the earlier name and said too much; `Received` says
/// worse — in a tool whose verb for taking a file is literally `recv`, it is the
/// one word guaranteed to be read as the state it isn't.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferStatus {
    /// Still queued: no client of theirs has read it.
    Pending,
    /// A recipient client read the offer — it reached one of their devices. Says
    /// nothing about whether anyone has looked at it, let alone decided.
    Arrived,
    /// The recipient took it: fetched and acked. The only state that reports a
    /// person acting.
    Taken,
    /// Gone without being taken: expired, or retracted by us. A relay older than
    /// the `taken` state also answers this for an offer that *was* taken — it has
    /// no way to tell us apart from an expiry, which is the gap `Taken` closes.
    Gone,
}

impl OfferStatus {
    /// The name a UI shows and a DTO carries.
    ///
    /// Not the same vocabulary as the HTTP body, where `Arrived` is still spelled
    /// `fetched` so that clients older than this state keep reading the relay
    /// correctly. That compatibility spelling has no business leaking upwards: the
    /// only place it exists is [`offer_status`]'s parse and the relay's own
    /// handler.
    pub fn as_str(self) -> &'static str {
        match self {
            OfferStatus::Pending => "pending",
            OfferStatus::Arrived => "arrived",
            OfferStatus::Taken => "taken",
            OfferStatus::Gone => "gone",
        }
    }
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
    // Like `retract_offer`, the offer may be in the previous epoch's slot. The relay
    // answers `gone` for a row it doesn't have, which is exactly what a wrong slot
    // looks like — so keep asking while the answer is `gone`, and only conclude the
    // offer is gone once every slot in the window says so.
    let mut first_err = None;
    for slot in inbox_slots_at(recipient, now_unix()) {
        let url = format!("{}/{id}/status", inbox_url(relay, &slot));
        let sent = client
            .get(url)
            .header(POSTER_TOKEN_HEADER, poster_token)
            .send()
            .await
            .context("offer status");
        let resp =
            match sent.and_then(|r| r.error_for_status().context("relay rejected offer status")) {
                Ok(r) => r,
                Err(e) => {
                    first_err.get_or_insert(e);
                    continue;
                }
            };
        let body = resp.text().await.unwrap_or_default();
        match body.trim() {
            // `fetched` is the wire spelling of `Arrived` — kept as-is so this client
            // and one older than the state it names read the same relay the same way.
            "fetched" => return Ok(OfferStatus::Arrived),
            "taken" => return Ok(OfferStatus::Taken),
            "gone" => continue,
            _ => return Ok(OfferStatus::Pending),
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(OfferStatus::Gone),
    }
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
///
/// Normally one request. Only just after a presence-epoch boundary — where a beacon
/// published seconds ago is still in the previous slot — is a second one spent, and
/// only if the first found nothing.
pub async fn check_online(
    client: &reqwest::Client,
    relay: &str,
    contact: &PublicId,
) -> Result<bool> {
    let mut first_err = None;
    for slot in presence_slots_at(&contact.to_bytes(), now_unix()) {
        let url = format!("{}/v1/presence/{slot}", relay.trim_end_matches('/'));
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(true),
            Ok(_) => {}
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    match first_err {
        Some(e) => Err(anyhow::Error::new(e).context("check presence")),
        None => Ok(false),
    }
}

/// A live subscription to *my* inbox on a relay. Long-polls for offers and
/// decrypts them with my identity; reading and acking are authenticated with a
/// proof-of-possession bearer token, cached and refreshed transparently.
pub struct InboxSubscription {
    client: reqwest::Client,
    relay: String,
    /// Owner's public id. Not a cached slot: slots rotate, so the slot to read is
    /// derived per call and the identity is what has to be kept.
    public: PublicId,
    /// Owner's identity secret — needed to open the session challenge. Held here
    /// (not passed per call) so `poll`/`ack`/`run` can authenticate on their own.
    me_secret: Vec<u8>,
    /// One cached session per slot: the relay's bearer token is MAC'd to the slot it
    /// was issued for, so reading two slots means two handshakes.
    sessions: std::sync::Mutex<HashMap<String, CachedSession>>,
    /// Which slot each row we have seen came from, so [`Self::ack`] deletes it where
    /// it actually lives. A DELETE naming the wrong slot answers 204 and removes
    /// nothing (the relay marks by `(slot, id)`), so this cannot be probed for —
    /// it has to be remembered.
    row_slots: std::sync::Mutex<HashMap<String, String>>,
    /// Rows that failed to decode in THIS subscription's lifetime — skipped (and
    /// not re-logged) on later rounds, but never acked: see `poll_wait` on why an
    /// undecodable row must survive us.
    undecodable: std::sync::Mutex<std::collections::HashSet<String>>,
    /// When the older slots were last drained, so the steady-state poll loop doesn't
    /// pay for them every round.
    last_backfill: std::sync::Mutex<u64>,
    /// This device's id, once its owner has said what it is
    /// ([`Self::set_origin`]). Offers stamped with it are ours and are skipped —
    /// see `poll_wait`. `None` leaves the poll exactly as it was.
    origin: std::sync::Mutex<Option<crate::sync::DeviceId>>,
}

#[derive(Clone)]
struct CachedSession {
    bearer: String,
    exp: u64,
}

/// Seconds the relay may hold a GET open waiting for an offer (long-poll).
const LONG_POLL_SECS: u64 = 25;

/// How often the previous epoch's slot is drained during a long-poll loop.
///
/// The current slot is long-polled every round, as before. A row in an older slot is
/// by definition at least one epoch old, so nothing is gained by asking for it every
/// 25 seconds — and asking would double a daemon's steady-state request count
/// against the relay for no latency anyone can perceive. A one-shot read (`wait=0`:
/// `arvolo status`, the receive listing, the sync engine) ignores this and always
/// looks in every slot, because there a missed row is a wrong answer to a person.
const BACKFILL_EVERY_SECS: u64 = 300;

/// Cap on remembered row→slot mappings. A slot holds at most 64 rows and the window
/// is two slots deep, so this is generous; it exists only so a session that runs for
/// months can't grow the map without bound.
const MAX_ROW_SLOTS: usize = 1024;

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
            client: crate::http::client(),
            relay: relay.into(),
            public: me.public(),
            me_secret: me.secret_bytes(),
            sessions: std::sync::Mutex::new(HashMap::new()),
            row_slots: std::sync::Mutex::new(HashMap::new()),
            undecodable: std::sync::Mutex::new(std::collections::HashSet::new()),
            last_backfill: std::sync::Mutex::new(0),
            origin: std::sync::Mutex::new(None),
        }
    }

    /// Tell this subscription which device it is polling for, so it can recognise
    /// the offers this same device posted. Call it before polling; see
    /// [`Offer::origin`].
    pub fn set_origin(&self, device: crate::sync::DeviceId) {
        *self.origin.lock().unwrap() = Some(device);
    }

    /// Did *this* device post this offer? False whenever either side is unknown —
    /// an offer from a client that predates [`Offer::origin`], or a subscription
    /// nobody told which device it is. Unknown must mean "not ours": mistaking
    /// someone else's offer for our own would drop a real delivery on the floor,
    /// while the reverse merely restores the behaviour that came before.
    fn is_ours(&self, offer: &Offer) -> bool {
        match (*self.origin.lock().unwrap(), offer.origin) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => false,
        }
    }

    /// Rebuild the owner's identity from the stored secret.
    fn me(&self) -> Result<Identity> {
        Identity::from_secret_bytes(&self.me_secret)
    }

    /// The slot a deposit into our own inbox goes to, and the one the poll loop
    /// holds open: the current epoch's.
    fn current_slot(&self) -> String {
        slot_for(&self.public)
    }

    /// Every slot that can still hold a row for us, current epoch first.
    fn read_slots(&self) -> Vec<String> {
        inbox_slots_at(&self.public, now_unix())
    }

    /// Remember where a row lives, so acking it later hits the right slot.
    fn remember_rows(&self, slot: &str, ids: impl Iterator<Item = String>) {
        let mut map = self.row_slots.lock().unwrap();
        if map.len() > MAX_ROW_SLOTS {
            // Losing the map costs nothing worse than an ack falling back to the
            // current slot, which is where all but the oldest rows are anyway.
            map.clear();
        }
        for id in ids {
            map.insert(id, slot.to_string());
        }
    }

    /// Whether the older slots are due a drain, stamping the clock if so.
    fn backfill_due(&self) -> bool {
        let now = now_unix();
        let mut last = self.last_backfill.lock().unwrap();
        if now.saturating_sub(*last) < BACKFILL_EVERY_SECS {
            return false;
        }
        *last = now;
        true
    }

    /// Which of our epochs produced `slot`, if any.
    ///
    /// A reader holds two slots at once and signs for whichever it is talking to, so
    /// the epoch cannot be taken from the clock at signing time — the previous
    /// epoch's slot needs the previous epoch's key. Searching the same short window
    /// the reader uses keeps the two definitions from drifting apart.
    fn epoch_of(&self, slot: &str, me: &Identity) -> Option<u64> {
        let now = now_unix() / INBOX_EPOCH_SECS;
        (0..=1u64)
            .map(|n| now.saturating_sub(n))
            .find(|&e| slot_for_epoch(&me.public(), e) == slot)
    }

    /// Return a valid bearer token for `slot`, running the proof-of-possession
    /// handshake if the cache is empty or the token is within 60s of expiry. `force`
    /// skips the cache (used after a 401, in case the relay's session secret
    /// rotated).
    async fn ensure_session(&self, slot: &str, force: bool) -> Result<String> {
        if !force {
            if let Some(s) = self.sessions.lock().unwrap().get(slot).cloned() {
                if now_unix() + 60 < s.exp {
                    return Ok(s.bearer);
                }
            }
        }
        let me = self.me()?;
        let url = format!("{}/session", inbox_url(&self.relay, slot));
        // No body: the relay needs nothing from us to issue a challenge, and that is
        // the improvement — this request used to carry our public key.
        let resp = self
            .client
            .post(&url)
            .send()
            .await
            .context("request inbox session")?
            .error_for_status()
            .context("relay rejected inbox session")?;
        let bytes = resp.bytes().await.context("read session challenge")?;
        let challenge: SessionChallenge =
            postcard::from_bytes(&bytes).context("decode session challenge")?;
        // Sign the challenge with the blinded secret for the epoch this slot belongs
        // to. Which epoch that is has to be *found*, not assumed: the slot may be the
        // previous one (we read two), and only its own blinded key can sign for it.
        let epoch = self
            .epoch_of(slot, &me)
            .context("no epoch of ours produces this slot")?;
        let sig = crate::crypto::blinded::sign_at(
            &me,
            epoch,
            &session_signing_input(slot, &challenge.nonce, challenge.exp),
        );
        let token = SessionToken {
            nonce: challenge.nonce,
            exp: challenge.exp,
            mac: challenge.mac,
            sig: sig.to_vec(),
        };
        let raw = postcard::to_allocvec(&token).context("encode session token")?;
        let bearer = data_encoding::BASE32_NOPAD.encode(&raw).to_lowercase();
        self.sessions.lock().unwrap().insert(
            slot.to_string(),
            CachedSession {
                bearer: bearer.clone(),
                exp: challenge.exp,
            },
        );
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
        let items = self.items_across_slots(wait_secs).await?;
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
                // An offer this very device posted. Our paired devices share an
                // identity, so it decodes and authenticates perfectly — it is
                // addressed to us, by us. Skip it and, like a sync note, do NOT
                // ack: our *other* devices still have to see it. Leaving it on
                // the slot costs nothing, since the poll loop dedupes by row id
                // and already holds a floor on how often a round may complete.
                Some((_, offer)) if self.is_ours(&offer) => continue,
                Some((sender, offer)) => out.push(ReceivedOffer {
                    id: item.id,
                    sender,
                    offer,
                }),
                // A row we can't decode is NOT acked. Acking marks it "taken" for
                // the sender and destroys it — and a decode failure is not proof
                // of junk: a daemon mid-restart with its identity half-loaded
                // fails this same match on a perfectly good offer, and an ack
                // here buried it forever (measured, not hypothesized). Skipping
                // costs nothing: the in-run dedupe stops it from hammering, a
                // restart retries the decode with fresh state, and true junk
                // expires with its slot under the relay's row cap anyway.
                None => {
                    if self.undecodable.lock().unwrap().insert(item.id.clone()) {
                        tracing::warn!(
                            "inbox row {} does not decode as an offer for this identity — \
                             leaving it on the relay (it may be readable after a restart)",
                            item.id
                        );
                    }
                }
            }
        }
        Ok(out)
    }

    /// Fetch the raw inbox items (authenticated), holding the connection up to
    /// `wait_secs`. Used by the sync engine, which needs the relay-assigned ids to
    /// clean up superseded sync notes.
    pub async fn raw_items(&self, wait_secs: u64) -> Result<Vec<InboxItem>> {
        self.items_across_slots(wait_secs).await
    }

    /// Read the rows in every slot that can still hold one, remembering where each
    /// came from.
    ///
    /// Only the current slot is long-polled; the older ones are read with `wait=0`,
    /// since a row that has been sitting in one for an epoch is not waiting on
    /// latency. During a poll loop they are visited every [`BACKFILL_EVERY_SECS`],
    /// and on a one-shot read (`wait_secs == 0`) always.
    async fn items_across_slots(&self, wait_secs: u64) -> Result<Vec<InboxItem>> {
        let slots = self.read_slots();
        let mut out = Vec::new();
        let mut backfill = wait_secs == 0;
        for (i, slot) in slots.iter().enumerate() {
            let first = i == 0;
            if !first {
                // Decide once, on the first older slot, so a window that grows past
                // two slots costs one stamp rather than one per slot.
                if i == 1 && !backfill {
                    backfill = self.backfill_due();
                }
                if !backfill {
                    break;
                }
            }
            let wait = if first { wait_secs } else { 0 };
            let url = format!("{}?wait={}", inbox_url(&self.relay, slot), wait);
            let bytes = match self.authed(reqwest::Method::GET, slot, &url).await {
                Ok(b) => b,
                // A failure on the *current* slot is the caller's to see — it is the
                // live inbox. An older slot failing is not worth losing the round
                // over: it holds nothing time-critical by construction.
                Err(e) if first => return Err(e),
                Err(_) => continue,
            };
            if bytes.is_empty() {
                continue;
            }
            let items: Vec<InboxItem> =
                postcard::from_bytes(&bytes).context("decode inbox items")?;
            self.remember_rows(slot, items.iter().map(|row| row.id.clone()));
            out.extend(items);
        }
        Ok(out)
    }

    /// Deposit a raw blob into **our own** inbox slot (unauthenticated POST, like
    /// any depositor). `ttl_secs` sets how long the relay keeps it — the sync cell
    /// uses a long TTL and is refreshed on each publish, and is clamped to the
    /// slot's own lifetime like any other row. Returns the relay id.
    pub async fn post_raw(&self, body: Vec<u8>, ttl_secs: Option<u64>) -> Result<String> {
        let mut url = inbox_url(&self.relay, &self.current_slot());
        if let Some(ttl) = clamp_inbox_ttl(ttl_secs) {
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
    ///
    /// In whichever slot we read it from: the relay keys rows by `(slot, id)` and
    /// answers 204 either way, so naming the current slot for a row that lives in an
    /// older one would report success and delete nothing — the offer would come back
    /// on the next round, forever. An id we have never seen falls back to the current
    /// slot, which is where anything not read through this subscription would be.
    pub async fn ack(&self, id: &str) -> Result<()> {
        let slot = self
            .row_slots
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_else(|| self.current_slot());
        let url = format!("{}/{id}", inbox_url(&self.relay, &slot));
        self.authed(reqwest::Method::DELETE, &slot, &url).await?;
        self.row_slots.lock().unwrap().remove(id);
        Ok(())
    }

    /// Perform an authenticated inbox request against `slot`, attaching its bearer
    /// token and re-authenticating once on a 401 (e.g. the relay restarted with a new
    /// session secret). Returns the response body bytes.
    async fn authed(&self, method: reqwest::Method, slot: &str, url: &str) -> Result<Vec<u8>> {
        let mut forced = false;
        loop {
            let bearer = self.ensure_session(slot, forced).await?;
            let resp = self
                .client
                .request(method.clone(), url)
                .bearer_auth(&bearer)
                .send()
                .await
                .context("inbox request")?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && !forced {
                // Stale token — drop it and retry the handshake once.
                self.sessions.lock().unwrap().remove(slot);
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

    /// A fixed instant well inside an epoch, so the "same epoch" cases below are not
    /// accidentally straddling a boundary.
    const T: u64 = 100 * INBOX_EPOCH_SECS + INBOX_EPOCH_SECS / 2;

    #[test]
    fn a_slot_is_a_key_and_belongs_to_one_identity() {
        let (a, b) = (Identity::generate(), Identity::generate());
        let slot = slot_for_at(&a.public(), T);
        assert_ne!(
            slot,
            slot_for_at(&b.public(), T),
            "two identities, two slots"
        );
        // A slot decodes to the key the relay authenticates against, and it is *not*
        // the identity: that is what the relay is prevented from learning.
        let key = slot_key(&slot).expect("a slot is a key");
        assert_eq!(key.len(), 32);
        assert_ne!(key.as_slice(), &a.public().ed_bytes()[..]);
        // The presence slot stays a plain hash and stays separate from this one.
        assert_ne!(slot, presence_slot_for(&a.public().to_bytes()));
    }

    #[test]
    fn inbox_slot_is_stable_within_an_epoch_and_rotates_across_one() {
        let id = Identity::generate().public();
        assert_eq!(slot_for_at(&id, T), slot_for_at(&id, T + 60));
        assert_eq!(
            slot_for_at(&id, T),
            slot_for_at(&id, T + INBOX_EPOCH_SECS / 2 - 1),
            "still the same epoch right up to the boundary"
        );
        assert_ne!(
            slot_for_at(&id, T),
            slot_for_at(&id, T + INBOX_EPOCH_SECS),
            "the whole point: one epoch on, the relay sees a different key"
        );
    }

    /// Only the owner can authenticate for a slot, and only for *that* slot's epoch.
    /// This replaces the old check of "does this key hash to this slot", which the
    /// relay had to do because it was handed the key — the thing it no longer sees.
    #[test]
    fn a_slot_can_only_be_signed_for_by_its_owner_and_its_epoch() {
        let (me, other) = (Identity::generate(), Identity::generate());
        let epoch = T / INBOX_EPOCH_SECS;
        let slot = slot_for_at(&me.public(), T);
        let key = slot_key(&slot).unwrap();
        let msg = session_signing_input(&slot, b"nonce", 123);

        let sig = crate::crypto::blinded::sign_at(&me, epoch, &msg);
        assert!(crate::crypto::blinded::verify(&key, &msg, &sig));

        assert!(
            !crate::crypto::blinded::verify(
                &key,
                &msg,
                &crate::crypto::blinded::sign_at(&other, epoch, &msg)
            ),
            "somebody else's signature must not open this inbox"
        );
        assert!(
            !crate::crypto::blinded::verify(
                &key,
                &msg,
                &crate::crypto::blinded::sign_at(&me, epoch + 1, &msg)
            ),
            "nor our own from another epoch"
        );
    }

    /// Every field of the challenge is bound, so a signature cannot be lifted to
    /// another slot, nonce or expiry.
    #[test]
    fn a_session_signature_is_bound_to_its_whole_challenge() {
        let me = Identity::generate();
        let epoch = T / INBOX_EPOCH_SECS;
        let slot = slot_for_at(&me.public(), T);
        let key = slot_key(&slot).unwrap();
        let msg = session_signing_input(&slot, b"nonce", 123);
        let sig = crate::crypto::blinded::sign_at(&me, epoch, &msg);

        for other in [
            session_signing_input(&slot, b"nonce", 124),
            session_signing_input(&slot, b"other-nonce", 123),
            session_signing_input(
                &slot_for_at(&me.public(), T - INBOX_EPOCH_SECS),
                b"nonce",
                123,
            ),
        ] {
            assert!(
                !crate::crypto::blinded::verify(&key, &other, &sig),
                "the signature must not carry to a different challenge"
            );
        }
    }

    #[test]
    fn a_live_row_is_always_in_a_slot_the_reader_looks_in() {
        // The invariant that makes rotation safe: post at any moment, read at any
        // later moment while the row can still be alive (TTL is clamped to one
        // epoch), and the slot posted to is in the reader's window.
        let id = Identity::generate().public();
        for post_offset in [0, 1, INBOX_EPOCH_SECS / 3, INBOX_EPOCH_SECS - 1] {
            let posted_at = T + post_offset;
            let posted_slot = slot_for_at(&id, posted_at);
            let ttl = clamp_inbox_ttl(Some(u64::MAX)).unwrap();
            for age in [0, 1, ttl / 2, ttl] {
                let slots = inbox_slots_at(&id, posted_at + age);
                assert!(
                    slots.contains(&posted_slot),
                    "a row posted at +{post_offset} and read {age}s later fell outside the window"
                );
            }
        }
    }

    #[test]
    fn the_ttl_clamp_is_what_holds_that_invariant_up() {
        assert_eq!(
            clamp_inbox_ttl(Some(30 * 24 * 3600)),
            Some(INBOX_EPOCH_SECS)
        );
        assert_eq!(clamp_inbox_ttl(Some(60)), Some(60));
        // `None` still means "the relay's own short default", not "one epoch".
        assert_eq!(clamp_inbox_ttl(None), None);
    }

    #[test]
    fn the_read_window_is_two_slots_and_current_first() {
        let id = Identity::generate().public();
        let slots = inbox_slots_at(&id, T);
        assert_eq!(slots.len(), 2);
        assert_eq!(
            slots[0],
            slot_for_at(&id, T),
            "the current slot is read first — it's the one that gets long-polled"
        );
    }

    #[test]
    fn epoch_zero_yields_one_slot_not_the_same_slot_twice() {
        // A clock claiming 1970 (or a test fixture with a small timestamp) must not
        // make the reader ask the relay the same question twice.
        let id = Identity::generate();
        assert_eq!(inbox_slots_at(&id.public(), 0).len(), 1);
        assert_eq!(presence_slots_at(&id.public().to_bytes(), 0).len(), 1);
    }

    #[test]
    fn presence_asks_about_the_previous_slot_only_just_after_a_boundary() {
        let k = Identity::generate().public().to_bytes();
        let boundary = 1000 * PRESENCE_EPOCH_SECS;

        // Just after: a beacon published seconds ago is still in the old slot.
        let just_after = presence_slots_at(&k, boundary + 5);
        assert_eq!(just_after.len(), 2);
        assert_eq!(just_after[0], presence_slot_for_at(&k, boundary + 5));
        assert_eq!(
            just_after[1],
            presence_slot_for_at(&k, boundary - 1),
            "and the second one asked is the epoch that just ended"
        );

        // Well inside the epoch: nothing there could still be alive (beacons last
        // 30s), so the extra request isn't spent.
        assert_eq!(
            presence_slots_at(&k, boundary + PRESENCE_GRACE_SECS).len(),
            1
        );
        assert_eq!(
            presence_slots_at(&k, boundary + PRESENCE_EPOCH_SECS / 2).len(),
            1
        );

        // Just *before* the next boundary the neighbour is the other way round: a
        // publisher whose clock runs slightly fast has already moved on.
        let before = boundary + PRESENCE_EPOCH_SECS - 5;
        let just_before = presence_slots_at(&k, before);
        assert_eq!(just_before.len(), 2);
        assert_eq!(just_before[0], presence_slot_for_at(&k, before));
        assert_eq!(
            just_before[1],
            presence_slot_for_at(&k, boundary + PRESENCE_EPOCH_SECS),
            "the second one asked is the epoch about to start"
        );
    }

    #[test]
    fn presence_slots_rotate_too_and_stay_separated_from_inbox_slots() {
        let id = Identity::generate().public();
        let k = id.to_bytes();
        assert_ne!(
            presence_slot_for_at(&k, T),
            presence_slot_for_at(&k, T + PRESENCE_EPOCH_SECS)
        );
        // Same identity, same instant, the two derivations: unrelated values. And
        // they are not even the same *kind* of thing any more — a presence slot is a
        // hash of the id, an inbox slot is a blinded key — so a relay seeing both
        // learns nothing that links them.
        assert_ne!(presence_slot_for_at(&k, T), slot_for_at(&id, T));
    }

    fn test_offer() -> Offer {
        Offer {
            name: "photo.jpg".into(),
            size: 1234,
            chunks: 1,
            ticket: "arvc-fake".into(),
            note: "see page 4".into(),
            sender_name: "Lorenzo".into(),
            origin: None,
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
