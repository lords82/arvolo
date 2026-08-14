//! The axum HTTP layer: route handlers and the [`router`] assembly.

use arvolo_core::chunked::{SeedRequest, CHUNK_SIZE};
use arvolo_core::swarm::{AnnounceReq, AnnounceResp, PeerInfo};
use axum::{
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::limits::{
    rz_claims_per_hour, rz_posts_per_min, rz_rate_limit, rz_slots_per_min, write_rate_limit,
    writes_per_min, ClientIp, RzAction,
};
use crate::mailbox::{InboxStatus, MailboxError};
use crate::state::{
    max_blob_bytes, max_inbox_rows, max_presence_rows, max_rz_rows, max_seeded_rows,
    max_session_relay_bytes, max_total_blob_bytes, AppState, SwarmPeer, CONTROL_PLANE_BODY_LIMIT,
    INBOX_MAX_TTL_SECS, INBOX_TTL_SECS, MAX_INBOX_PER_SLOT, MAX_INBOX_VALUE_BYTES,
    MAX_RZ_VALUE_BYTES, MAX_SEED_CHUNKS_PER_REQ, MAX_SWARMS, MAX_SWARM_CHUNKS, MAX_SWARM_PEERS,
    PRESENCE_TTL_SECS, SWARM_PEER_TTL_SECS,
};
use crate::util::{constant_time_eq, now_unix, random_claim};

const ENCAPPED_KEY_HEADER: &str = "x-arvolo-encapped-key";
/// Base32 BLAKE3 hash of the revoke token, sent at deposit (optional).
const REVOKE_HASH_HEADER: &str = "x-arvolo-revoke-hash";
/// The revoke token itself, sent on a DELETE to authorize revocation.
const REVOKE_TOKEN_HEADER: &str = "x-arvolo-revoke-token";
/// Base32 BLAKE3 hash of an inbox offer's retract token, sent at inbox POST.
const INBOX_POSTER_HASH_HEADER: &str = "x-arvolo-poster-hash";
/// The inbox offer's retract token, sent on a DELETE to retract one's own offer.
const INBOX_POSTER_TOKEN_HEADER: &str = "x-arvolo-poster-token";
/// Seconds the relay actually granted a deposit, answered on the deposit response.
///
/// The request's `?ttl=` is a wish: [`Mailbox::commit_deposit`] clamps it to this
/// relay's `ARVOLO_MAX_TTL`. Until this header existed the clamp was invisible, and
/// the sender kept building on the number it asked for — a 7-day inbox offer over a
/// 24-hour blob, which the recipient discovers as a 404 on the second day. A client
/// too old to read it is no worse off than before.
const GRANTED_TTL_HEADER: &str = "x-arvolo-ttl";

// ---- HTTP layer -----------------------------------------------------------

#[derive(Deserialize)]
struct DepositQuery {
    #[serde(default = "default_ttl")]
    ttl: u64,
    #[serde(default = "default_max")]
    max: u32,
}

fn default_ttl() -> u64 {
    7 * 24 * 3600
}
fn default_max() -> u32 {
    1
}

/// The browser secure-download page, its script, and the streaming service
/// worker. Decryption happens entirely client-side; the relay only serves
/// ciphertext.
const DL_HTML: &str = include_str!("web/dl.html");
const DL_JS: &str = include_str!("web/dl.js");
const DL_SW: &str = include_str!("web/arvolo-sw.js");

/// A strict Content-Security-Policy for the download page: no external
/// resources at all, only same-origin script/worker/frame and same-origin fetch
/// (to `/v1/fetch/{claim}`). `worker-src`/`frame-src` are for the streaming
/// service worker and the hidden download iframe it drives. No inline script.
const DL_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; \
     connect-src 'self'; img-src 'self' data:; worker-src 'self'; frame-src 'self'; \
     frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// Shown (with 403) when the administrator has disabled download links. Its own
/// file rather than a string literal here, so it can carry the same styling as
/// the download page it stands in for.
const DL_DISABLED_HTML: &str = include_str!("web/disabled.html");

const LINKS_DISABLED_MSG: &str = "public download links are disabled by this relay's administrator";

/// The languages the browser pages are translated into — the same four the
/// desktop app speaks (`gui/src/i18n`). English is the fallback.
const PAGE_LANGS: [&str; 4] = ["en", "it", "fr", "de"];

/// Pick a page language from `Accept-Language`.
///
/// Only the 403 page needs this. The download page itself translates in the
/// browser, off `navigator.languages`, because it is a static file and must
/// stay cacheable — and because its running commentary ("Decrypting…", the
/// failure messages) lives in the script anyway.
///
/// Quality values are honoured and the region is dropped (`de-AT` → `de`), so a
/// reader whose header is `es, de;q=0.5` gets German rather than English. Ties
/// go to the earlier entry, which is the reader's own order of preference.
fn negotiate_lang(headers: &HeaderMap) -> &'static str {
    let Some(header) = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
    else {
        return "en";
    };
    let mut best: Option<(f32, &'static str)> = None;
    for part in header.split(',') {
        let mut bits = part.split(';');
        let tag = bits.next().unwrap_or("").trim().to_ascii_lowercase();
        let base = tag.split('-').next().unwrap_or("");
        let Some(lang) = PAGE_LANGS.iter().find(|l| **l == base) else {
            continue;
        };
        let q = bits
            .find_map(|b| b.trim().strip_prefix("q=").map(str::trim).map(str::parse))
            .unwrap_or(Ok(1.0))
            .unwrap_or(0.0);
        if q > 0.0 && best.is_none_or(|(best_q, _)| q > best_q) {
            best = Some((q, lang));
        }
    }
    best.map_or("en", |(_, lang)| lang)
}

fn links_disabled_page(headers: &HeaderMap) -> Response {
    (
        StatusCode::FORBIDDEN,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        DL_DISABLED_HTML.replace("{{lang}}", negotiate_lang(headers)),
    )
        .into_response()
}

async fn dl_page_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.links_enabled {
        return links_disabled_page(&headers);
    }
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (axum::http::header::CONTENT_SECURITY_POLICY, DL_CSP),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        DL_HTML,
    )
        .into_response()
}

async fn dl_js_handler(State(state): State<AppState>) -> Response {
    if !state.links_enabled {
        return (StatusCode::FORBIDDEN, LINKS_DISABLED_MSG).into_response();
    }
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (axum::http::header::CONTENT_SECURITY_POLICY, DL_CSP),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        DL_JS,
    )
        .into_response()
}

/// The streaming download service worker. Served from the root path so its
/// default scope (`/`) covers the `/dl/stream/{id}` requests it must intercept.
async fn dl_sw_handler(State(state): State<AppState>) -> Response {
    if !state.links_enabled {
        return (StatusCode::FORBIDDEN, LINKS_DISABLED_MSG).into_response();
    }
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (
                axum::http::HeaderName::from_static("service-worker-allowed"),
                "/",
            ),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        DL_SW,
    )
        .into_response()
}

/// Advertise this relay's optional features so a client can fail fast: `links`
/// (public browser download links) and `rz2` (long-lived, multi-session pairing
/// slots — [`RZ_OWN_KEY`] and friends).
///
/// The two are read with opposite defaults on purpose. A client that can't reach
/// this endpoint assumes links are allowed (worst case: a deposit is refused
/// later), but assumes `rz2` is absent — announcing a v2 code no relay can host
/// would strand a receiver, so the missing field must mean "no".
async fn features_handler(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        format!("{{\"links\":{},\"rz2\":true}}", state.links_enabled),
    )
}

/// Swarm tracker: register/refresh this peer for `swarm_id` and return a sample of
/// the others. `POST /v1/swarm/{swarm_id}/announce`. Zero-knowledge: the relay
/// stores node addresses + bitfields with a TTL, never the key or plaintext.
async fn swarm_announce_handler(
    State(state): State<AppState>,
    AxumPath(swarm_id): AxumPath<String>,
    ip: ClientIp,
    Json(req): Json<AnnounceReq>,
) -> Response {
    if let Err((code, _)) =
        write_rate_limit(&state.write_limiter, ip.0, now_unix(), writes_per_min())
    {
        return code.into_response();
    }
    if req.n_chunks > MAX_SWARM_CHUNKS
        || req.node_addr.is_empty()
        || req.node_addr.len() > 4096
        || req.bitfield.len() > (req.n_chunks as usize).div_ceil(8).max(1)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let now = now_unix();
    let want = (req.want as usize).min(MAX_SWARM_PEERS);
    let mut tracker = state.swarm.lock().unwrap();

    // Reject a brand-new swarm once we're tracking too many (memory guard).
    if !tracker.contains_key(&swarm_id) && tracker.len() >= MAX_SWARMS && req.event != "stopped" {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    let peers_map = tracker.entry(swarm_id.clone()).or_default();
    peers_map.retain(|_, e| e.expires_at > now);

    if req.event == "stopped" {
        peers_map.remove(&req.node_addr);
    } else if peers_map.len() < MAX_SWARM_PEERS || peers_map.contains_key(&req.node_addr) {
        peers_map.insert(
            req.node_addr.clone(),
            SwarmPeer {
                node_addr: req.node_addr.clone(),
                bitfield: req.bitfield.clone(),
                expires_at: now + SWARM_PEER_TTL_SECS,
            },
        );
    }

    let peers: Vec<PeerInfo> = peers_map
        .values()
        .filter(|e| e.node_addr != req.node_addr)
        .take(want)
        .map(|e| PeerInfo {
            node_addr: e.node_addr.clone(),
            bitfield: e.bitfield.clone(),
        })
        .collect();

    if peers_map.is_empty() {
        tracker.remove(&swarm_id);
    }
    Json(AnnounceResp { peers }).into_response()
}

/// Swarm tracker: list current peers for `swarm_id` without announcing.
/// `GET /v1/swarm/{swarm_id}/peers`.
async fn swarm_peers_handler(
    State(state): State<AppState>,
    AxumPath(swarm_id): AxumPath<String>,
) -> Response {
    let now = now_unix();
    let mut tracker = state.swarm.lock().unwrap();
    let peers = tracker
        .get_mut(&swarm_id)
        .map(|m| {
            m.retain(|_, e| e.expires_at > now);
            m.values()
                .take(MAX_SWARM_PEERS)
                .map(|e| PeerInfo {
                    node_addr: e.node_addr.clone(),
                    bitfield: e.bitfield.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Json(AnnounceResp { peers }).into_response()
}

/// Build the relay HTTP router over the shared [`AppState`].
///
/// A fixed global request-body limit ([`CONTROL_PLANE_BODY_LIMIT`]) bounds every
/// small route. `/v1/deposit` is exempt: it streams the body straight to disk and
/// enforces [`max_blob_bytes`] itself, so it never buffers a whole file in memory.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/deposit", post(deposit_handler))
        .route("/v1/features", get(features_handler))
        .route("/v1/fetch/{claim}", get(fetch_handler))
        .route("/v1/entry/{claim}", axum::routing::delete(revoke_handler))
        .route("/v1/entry/{claim}/status", get(entry_status_handler))
        .route("/v1/addr", get(addr_handler))
        .route("/v1/seed", post(seed_handler))
        .route("/v1/release/{token}/{hash}", post(release_handler))
        .route(
            "/v1/rz/{slot}/{key}",
            post(rz_post_handler)
                .get(rz_get_handler)
                .delete(rz_delete_handler),
        )
        .route(
            "/v1/inbox/{slot}",
            post(inbox_post_handler).get(inbox_get_handler),
        )
        .route("/v1/inbox/{slot}/session", post(inbox_session_handler))
        .route(
            "/v1/inbox/{slot}/{id}",
            axum::routing::delete(inbox_delete_handler),
        )
        .route("/v1/inbox/{slot}/{id}/status", get(inbox_status_handler))
        .route(
            "/v1/presence/{slot}",
            post(presence_post_handler).get(presence_get_handler),
        )
        // Swarm tracker (peer rendezvous for a shared arvc… ticket).
        .route(
            "/v1/swarm/{swarm_id}/announce",
            post(swarm_announce_handler),
        )
        .route("/v1/swarm/{swarm_id}/peers", get(swarm_peers_handler))
        // Browser secure-download page (E2E: decrypts client-side).
        .route("/dl/{claim}", get(dl_page_handler))
        .route("/dl.js", get(dl_js_handler))
        .route("/arvolo-sw.js", get(dl_sw_handler))
        .route("/healthz", get(|| async { "ok" }))
        // Applied after the routes so it wraps them all: bounds the small
        // control-plane bodies. `/v1/deposit` reads a raw `Body` (not a
        // length-limited extractor), so it is unaffected and self-enforces.
        .layer(axum::extract::DefaultBodyLimit::max(
            CONTROL_PLANE_BODY_LIMIT,
        ))
        .with_state(state)
}

/// TTL (seconds) for a rendezvous slot: long enough for a human to type the code,
/// short enough that abandoned slots vanish quickly.
const RZ_TTL: u64 = 600;
/// Key under which the sender claims a slot (its SPAKE2 message).
const RZ_CLAIM_KEY: &str = "ms";
/// Key holding the encrypted ticket; fetching it burns the whole slot.
const RZ_TICKET_KEY: &str = "tkt";
/// Most unexpired rows one slot may hold. Bounds what guessing a 4-digit
/// nameplate is worth to an attacker once keys are no longer a fixed set of three.
const MAX_RZ_KEYS_PER_SLOT: i64 = 32;

// ---- rendezvous v2: a long-lived slot with one sub-session per receiver ----
//
// v1 is one in-memory handshake over three fixed keys, and the first `tkt` fetch
// destroys the slot. That makes a code short-lived, single-use, and impossible to
// resume after the sender restarts — its SPAKE2 scalar lived only in RAM.
//
// v2 keeps the same 4-digit nameplate and the same code grammar, but models the
// slot as a mailbox the owner holds a capability for. Each receiver picks its own
// random `sid` and gets four private keys; the owner answers each with a FRESH
// SPAKE2 run. The owner's entire state is then `(slot, code, token)` — three
// things that fit on disk — which is what makes a code survive a restart.

/// Claim key of a v2 slot. Value is `blake3(owner_token) || created_at_le` —
/// the token itself never reaches the relay, and the creation stamp rides along
/// so the absolute lifetime cap needs no extra column.
const RZ_OWN_KEY: &str = "own";
/// Owner-only listing of receivers waiting for an answer; renews the slot.
const RZ_SESSIONS_KEY: &str = "sessions";
/// Receiver's SPAKE2 message (public write, owner read).
const RZ_P_RECV: &str = "r.";
/// Sender's SPAKE2 message (owner write, public read).
const RZ_P_SEND: &str = "s.";
/// Receiver's key confirmation (public write, owner read).
const RZ_P_CONF: &str = "c.";
/// Sealed payload for one session (owner write, public read — and reading it
/// burns that session's keys, never the slot).
const RZ_P_TKT: &str = "t.";
/// Sealed payload travelling the other way, receiver → sender (public write,
/// owner read). The rest of the rendezvous is one-directional: the sender puts
/// something in a slot and the receiver takes it. This is what lets the two ends
/// *exchange* rather than hand over — a mutual `arvolo contacts pair`, where each
/// side ends up knowing the other's public id.
///
/// Public write, like `c.`: the receiver holds no owner token, and cannot be
/// asked for one. That is safe because the value is sealed under a key derived
/// from the completed PAKE, so only a party that proved it knew the code can
/// produce one the sender will open — and the relay itself never sees inside.
const RZ_P_BACK: &str = "b.";
/// Body a v2 `own` GET returns. A fixed marker: the stored hash is a verifier and
/// must never be echoed, or holding it would be as good as holding the token.
const RZ_V2_MARKER: &[u8] = b"2";
/// Bytes of the `own` row: 32-byte token hash + 8-byte creation stamp.
const RZ_OWN_ROW_LEN: usize = 40;
/// How far each owner touch pushes the slot's expiry out.
const RZ_V2_LEASE_SECS: u64 = 3600;
/// Hard ceiling on a slot's life no amount of renewing can pass. Without it a
/// renewable slot is a permanent nameplate squat, and there are only 10k.
const RZ_MAX_SLOT_LIFETIME: u64 = 24 * 3600;
/// Long-poll bounds for the sessions listing, mirroring the inbox's.
const RZ_SESSIONS_MAX_WAIT: u64 = 30;
const RZ_SESSIONS_POLL_MS: u64 = 500;

/// The `Authorization: Bearer <base32>` token, decoded. Same header the inbox
/// uses; deliberately not a query parameter, which would land in access logs.
fn rz_bearer(headers: &HeaderMap) -> Option<Vec<u8>> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?;
    let token = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))?;
    data_encoding::BASE32_NOPAD
        .decode(token.trim().to_ascii_uppercase().as_bytes())
        .ok()
}

/// The `own` row of `slot`, if it is a live v2 slot: `(token_hash, created_at)`.
fn rz_own_row(state: &AppState, slot: &str) -> Option<([u8; 32], u64)> {
    let row = state.mailbox.rz_get(slot, RZ_OWN_KEY, now_unix())?;
    if row.len() != RZ_OWN_ROW_LEN {
        return None;
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&row[..32]);
    let mut stamp = [0u8; 8];
    stamp.copy_from_slice(&row[32..]);
    Some((hash, u64::from_le_bytes(stamp)))
}

/// Verify the request holds this slot's owner token; yields its creation stamp.
/// 404 when the slot isn't a live v2 slot, 403 when the token is absent or wrong —
/// the owner needs to tell "my slot expired, re-claim it" from "someone else has
/// this nameplate now, the code is dead".
fn rz_owner(
    state: &AppState,
    headers: &HeaderMap,
    slot: &str,
) -> Result<u64, (StatusCode, String)> {
    let Some((want, created)) = rz_own_row(state, slot) else {
        return Err((StatusCode::NOT_FOUND, "no such rendezvous slot".into()));
    };
    let Some(token) = rz_bearer(headers) else {
        return Err((StatusCode::FORBIDDEN, "slot owner token required".into()));
    };
    if !constant_time_eq(blake3::hash(&token).as_bytes(), &want) {
        return Err((StatusCode::FORBIDDEN, "not the owner of this slot".into()));
    }
    Ok(created)
}

/// When a v2 row written now should expire: one lease out, but never past the
/// slot's absolute ceiling.
fn rz_v2_expiry(created: u64) -> u64 {
    let now = now_unix();
    now.saturating_add(RZ_V2_LEASE_SECS)
        .min(created.saturating_add(RZ_MAX_SLOT_LIFETIME))
}

/// `?wait=` on a rendezvous GET: how many seconds the sessions listing may be
/// held open. Ignored by every other key.
#[derive(Deserialize)]
struct RzWait {
    #[serde(default)]
    wait: u64,
}

/// Whether `key` belongs to a v2 sub-session, and which side may write it.
fn rz_session_prefix(key: &str) -> Option<&'static str> {
    [RZ_P_RECV, RZ_P_SEND, RZ_P_CONF, RZ_P_TKT, RZ_P_BACK]
        .into_iter()
        .find(|p| key.starts_with(p) && key.len() > p.len())
}

/// Whether `k` is an acceptable rendezvous key: `^[a-z0-9][a-z0-9._-]{0,63}$`.
///
/// Any path segment used to be a valid key, which was harmless while the whole
/// vocabulary was `ms`/`mr`/`tkt`. It stops being harmless as soon as part of a
/// key comes from the peer: an unvalidated key is unbounded in length, lands
/// verbatim in the access log, and — the sharp edge — would carry SQL `LIKE`
/// wildcards (`%`, `_`) into any prefix-matched delete, letting one session erase
/// another's rows. Validating here means deletes can always name exact keys.
fn valid_rz_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && k.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// Store a rendezvous value. The claim key (`ms` for v1, `own` for v2) fails with
/// 409 if the slot is already taken, so the sender can pick a fresh nameplate.
///
/// Write authorization by key, so that knowing a session id is not the same as
/// being able to answer it: `own` and the receiver's `r.`/`c.` are open (first
/// write wins), while the sender's `s.`/`t.` require the owner token. Without
/// that split a hostile receiver could pre-write the answer to its own session
/// and poison it with two POSTs.
async fn rz_post_handler(
    State(state): State<AppState>,
    AxumPath((slot, key)): AxumPath<(String, String)>,
    ip: ClientIp,
    headers: HeaderMap,
    body: Bytes,
) -> Result<String, (StatusCode, String)> {
    rz_rate_limit(
        &state.rz_limiter,
        ip.0,
        RzAction::Post,
        now_unix(),
        rz_posts_per_min(),
        rz_slots_per_min(),
    )?;
    if !valid_rz_key(&key) {
        return Err((StatusCode::BAD_REQUEST, "invalid rendezvous key".into()));
    }
    if key == RZ_SESSIONS_KEY {
        return Err((
            StatusCode::METHOD_NOT_ALLOWED,
            "sessions is read-only".into(),
        ));
    }
    if body.len() > MAX_RZ_VALUE_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "rendezvous value too large".into(),
        ));
    }
    // Disk-fill guard on the (unauthenticated) rendezvous table.
    if state.mailbox.rz_count() >= max_rz_rows() {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "relay at capacity".into()));
    }
    // Same guard, per slot: the global cap alone would let one guessed nameplate
    // soak up the whole table.
    if state.mailbox.rz_slot_row_count(&slot, now_unix()) >= MAX_RZ_KEYS_PER_SLOT {
        return Err((
            StatusCode::INSUFFICIENT_STORAGE,
            "rendezvous slot is full".into(),
        ));
    }
    // Which protocol this write belongs to, and what it is allowed to do.
    let own = rz_own_row(&state, &slot);
    let exp = if key == RZ_OWN_KEY {
        // A v2 claim. Costs a per-IP claim budget on top of the POST budget:
        // renewal being free is what would otherwise make squatting all 10k
        // nameplates a one-off purchase.
        rz_rate_limit(
            &state.rz_limiter,
            ip.0,
            RzAction::Claim,
            now_unix(),
            rz_claims_per_hour(),
            rz_slots_per_min(),
        )?;
        if body.len() != 32 {
            return Err((
                StatusCode::BAD_REQUEST,
                "own takes a 32-byte token hash".into(),
            ));
        }
        // Don't let a v2 claim graft itself onto a live v1 pairing (or vice
        // versa): a mixed slot has no coherent authorization story. 409 is what
        // both clients already retry with a fresh nameplate on.
        if state
            .mailbox
            .rz_get(&slot, RZ_CLAIM_KEY, now_unix())
            .is_some()
        {
            return Err((StatusCode::CONFLICT, "slot already taken".into()));
        }
        now_unix().saturating_add(RZ_V2_LEASE_SECS)
    } else if let Some(prefix) = rz_session_prefix(&key) {
        let Some((_, created)) = own else {
            return Err((StatusCode::NOT_FOUND, "no such rendezvous slot".into()));
        };
        // Only the slot owner may speak for the sender.
        if prefix == RZ_P_SEND || prefix == RZ_P_TKT {
            rz_owner(&state, &headers, &slot)?;
        }
        let exp = rz_v2_expiry(created);
        if exp <= now_unix() {
            return Err((StatusCode::GONE, "rendezvous slot has expired".into()));
        }
        exp
    } else {
        // Legacy v1 key. A slot already claimed under v2 is taken, full stop.
        if own.is_some() {
            return Err((StatusCode::CONFLICT, "slot already taken".into()));
        }
        now_unix().saturating_add(RZ_TTL)
    };
    // First-writer-wins for EVERY rendezvous key, not just the slot claim (`ms`).
    // Each key of a pairing (`ms` sender msg, `mr` receiver msg, `tkt` encrypted
    // ticket) is legitimately written exactly once; allowing overwrite (the old
    // INSERT-OR-REPLACE on `mr`/`tkt`) let anyone who guesses the slot (a 4-digit
    // nameplate, only 10k values) clobber an in-flight ticket/message and grief the
    // pairing. Claiming on first write closes that without affecting the honest flow.
    // The `own` row carries its creation stamp so the absolute lifetime cap can be
    // enforced later without a schema change.
    let value = if key == RZ_OWN_KEY {
        let mut v = body.to_vec();
        v.extend_from_slice(&now_unix().to_le_bytes());
        v
    } else {
        body.to_vec()
    };
    let claimed = state
        .mailbox
        .rz_claim(&slot, &key, &value, exp)
        .map_err(err_response)?;
    if !claimed {
        let msg = if key == RZ_CLAIM_KEY || key == RZ_OWN_KEY {
            "slot already taken"
        } else {
            "rendezvous key already written"
        };
        return Err((StatusCode::CONFLICT, msg.into()));
    }
    Ok("ok".into())
}

/// Read a rendezvous value (404 until posted). Reading a v1 ticket burns the whole
/// slot; reading a v2 session's ticket burns only that session.
async fn rz_get_handler(
    State(state): State<AppState>,
    AxumPath((slot, key)): AxumPath<(String, String)>,
    Query(q): Query<RzWait>,
    ip: ClientIp,
    headers: HeaderMap,
) -> Result<Bytes, (StatusCode, String)> {
    if !valid_rz_key(&key) {
        return Err((StatusCode::BAD_REQUEST, "invalid rendezvous key".into()));
    }
    // Polling one slot is never throttled; touching many *distinct* slots (a
    // nameplate sweep hunting for in-flight pairings) is.
    rz_rate_limit(
        &state.rz_limiter,
        ip.0,
        RzAction::GetSlot(&slot),
        now_unix(),
        rz_posts_per_min(),
        rz_slots_per_min(),
    )?;

    // How a receiver learns which protocol a code speaks. Answers a fixed marker,
    // never the stored verifier.
    if key == RZ_OWN_KEY {
        return match rz_own_row(&state, &slot) {
            Some(_) => Ok(Bytes::from_static(RZ_V2_MARKER)),
            None => Err((StatusCode::NOT_FOUND, "not yet".into())),
        };
    }
    if key == RZ_SESSIONS_KEY {
        return rz_sessions(&state, &headers, &slot, q.wait).await;
    }
    if let Some(prefix) = rz_session_prefix(&key) {
        // The receiver's half is addressed to the owner alone: leaking the list of
        // live session ids would undo the point of an unguessable one. `b.` is the
        // receiver's reply, so it belongs to the owner for the same reason.
        if prefix == RZ_P_RECV || prefix == RZ_P_CONF || prefix == RZ_P_BACK {
            rz_owner(&state, &headers, &slot)?;
        }
        let Some(v) = state.mailbox.rz_get(&slot, &key, now_unix()) else {
            return Err((StatusCode::NOT_FOUND, "not yet".into()));
        };
        if prefix == RZ_P_TKT {
            // Burn this session only — the slot goes on serving the next receiver.
            // `b.` is deliberately absent: the receiver writes its reply *after*
            // collecting the payload, so it does not exist yet and would only be
            // deleted before it could arrive.
            let sid = &key[RZ_P_TKT.len()..];
            let keys: Vec<String> = [RZ_P_RECV, RZ_P_SEND, RZ_P_CONF, RZ_P_TKT]
                .iter()
                .map(|p| format!("{p}{sid}"))
                .collect();
            state.mailbox.rz_delete_keys(&slot, &keys);
        }
        if prefix == RZ_P_BACK {
            // Reading the reply burns it, mirroring `t.`: it has reached the one
            // party entitled to it, and leaving it behind would keep a row alive
            // for the rest of the slot's lease for nobody's benefit.
            state
                .mailbox
                .rz_delete_keys(&slot, std::slice::from_ref(&key));
        }
        return Ok(Bytes::from(v));
    }

    // A v1 receiver polling a slot that is now a v2 rendezvous would otherwise
    // sit on 404 for its full two-minute timeout. Say so immediately: `poll_get`
    // treats any non-404 error as fatal, so an old client fails legibly.
    if key == RZ_CLAIM_KEY && rz_own_row(&state, &slot).is_some() {
        return Err((
            StatusCode::GONE,
            "this code needs a newer arvolo (rendezvous v2)".into(),
        ));
    }
    match state.mailbox.rz_get(&slot, &key, now_unix()) {
        Some(v) => {
            if key == RZ_TICKET_KEY {
                state.mailbox.rz_delete_slot(&slot);
            }
            Ok(Bytes::from(v))
        }
        None => Err((StatusCode::NOT_FOUND, "not yet".into())),
    }
}

/// Owner-only: retire a v2 slot early (`DELETE /v1/rz/{slot}/own`). Cancelling a
/// background code should actually free its nameplate, not leave it squatted for
/// the rest of its lease.
async fn rz_delete_handler(
    State(state): State<AppState>,
    AxumPath((slot, key)): AxumPath<(String, String)>,
    ip: ClientIp,
    headers: HeaderMap,
) -> Result<String, (StatusCode, String)> {
    rz_rate_limit(
        &state.rz_limiter,
        ip.0,
        RzAction::GetSlot(&slot),
        now_unix(),
        rz_posts_per_min(),
        rz_slots_per_min(),
    )?;
    if key != RZ_OWN_KEY {
        return Err((
            StatusCode::METHOD_NOT_ALLOWED,
            "only the slot claim can be deleted".into(),
        ));
    }
    rz_owner(&state, &headers, &slot)?;
    state.mailbox.rz_delete_slot(&slot);
    Ok("ok".into())
}

/// Owner-only: the session ids waiting for an answer (`r.{sid}` posted, `s.{sid}`
/// not yet), one per line, long-polling up to `wait` seconds for the first.
///
/// This doubles as the slot's renewal — the owner asking "who is waiting?" is
/// exactly the signal that the code is still wanted, so there is no separate
/// keepalive to spend a POST on, and a code idling for hours costs one
/// distinct-slot GET per rate-limit window.
async fn rz_sessions(
    state: &AppState,
    headers: &HeaderMap,
    slot: &str,
    wait: u64,
) -> Result<Bytes, (StatusCode, String)> {
    let created = rz_owner(state, headers, slot)?;
    let exp = rz_v2_expiry(created);
    if exp <= now_unix() {
        return Err((StatusCode::GONE, "rendezvous slot has expired".into()));
    }
    state.mailbox.rz_touch_slot(slot, exp, now_unix());

    let deadline = now_unix().saturating_add(wait.min(RZ_SESSIONS_MAX_WAIT));
    loop {
        let answered: std::collections::HashSet<String> = state
            .mailbox
            .rz_slot_keys_prefixed(slot, RZ_P_SEND, now_unix())
            .into_iter()
            .map(|k| k[RZ_P_SEND.len()..].to_string())
            .collect();
        let pending: Vec<String> = state
            .mailbox
            .rz_slot_keys_prefixed(slot, RZ_P_RECV, now_unix())
            .into_iter()
            .map(|k| k[RZ_P_RECV.len()..].to_string())
            .filter(|sid| !answered.contains(sid))
            .collect();
        if !pending.is_empty() {
            return Ok(Bytes::from(pending.join("\n")));
        }
        if now_unix() >= deadline {
            return Ok(Bytes::new());
        }
        tokio::time::sleep(std::time::Duration::from_millis(RZ_SESSIONS_POLL_MS)).await;
    }
}

/// How long a client may ask the relay to hold a GET open (long-poll), and the
/// poll granularity while it waits.
const INBOX_MAX_WAIT_SECS: u64 = 30;
const INBOX_POLL_MS: u64 = 500;
/// Lifetime of an inbox read/delete session token.
const INBOX_SESSION_TTL: u64 = 3600;
/// Length of the proof-of-possession nonce.
const INBOX_NONCE_LEN: usize = 16;

#[derive(Deserialize)]
struct InboxWait {
    #[serde(default)]
    wait: u64,
}

/// MAC binding a session nonce to its slot and expiry, keyed by the relay's
/// per-process secret. BLAKE3 keyed hash → 32 bytes.
fn inbox_mac(secret: &[u8; 32], slot: &str, nonce: &[u8], exp: u64) -> [u8; 32] {
    let mut input = Vec::with_capacity(slot.len() + nonce.len() + 8);
    input.extend_from_slice(slot.as_bytes());
    input.extend_from_slice(nonce);
    input.extend_from_slice(&exp.to_le_bytes());
    *blake3::keyed_hash(secret, &input).as_bytes()
}

/// Verify the `Authorization: Bearer <token>` proves ownership of `slot`:
/// the token's MAC must match one we issued for this slot, and be unexpired.
fn inbox_authorized(headers: &HeaderMap, slot: &str, secret: &[u8; 32]) -> bool {
    let Some(auth) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(token) = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
    else {
        return false;
    };
    let Some((nonce, exp, mac)) = arvolo_core::presence::decode_session_token(token.trim()) else {
        return false;
    };
    if now_unix() >= exp {
        return false;
    }
    let expected = inbox_mac(secret, slot, &nonce, exp);
    mac.len() == expected.len() && constant_time_eq(&mac, &expected)
}

/// Issue a proof-of-possession session: seal a random nonce to the presented
/// public key (only its owner can open it) and return the sealed nonce plus a
/// MAC binding it to this slot. The client echoes the opened nonce back as a
/// bearer token on subsequent read/delete requests.
async fn inbox_session_handler(
    State(state): State<AppState>,
    AxumPath(slot): AxumPath<String>,
    body: Bytes,
) -> Result<Bytes, (StatusCode, String)> {
    let pubid = arvolo_core::crypto::PublicId::from_bytes(&body)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid public id".into()))?;
    // Bind the request to the slot: only the key whose hash *is* this slot may
    // authenticate for it. Slots rotate per epoch, so any slot in the readable
    // window counts — a client polling across a boundary still has rows in the
    // previous epoch's slot and must be able to authenticate for it.
    if !arvolo_core::presence::inbox_slot_matches(&body, &slot, now_unix()) {
        return Err((
            StatusCode::FORBIDDEN,
            "public id does not match slot".into(),
        ));
    }
    let nonce: [u8; INBOX_NONCE_LEN] = rand::random();
    let sealed = arvolo_core::presence::seal_session_nonce(&pubid, &nonce)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let exp = now_unix().saturating_add(INBOX_SESSION_TTL);
    let mac = inbox_mac(&state.auth_secret, &slot, &nonce, exp);
    let challenge = arvolo_core::presence::SessionChallenge {
        encapped_key: sealed.encapped_key,
        ciphertext: sealed.ciphertext,
        exp,
        mac: mac.to_vec(),
    };
    let bytes = arvolo_core::presence::encode_session_challenge(&challenge)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Bytes::from(bytes))
}

/// Optional `?ttl=` on an inbox deposit: an offline offer (pointing at a mailbox
/// blob) must outlive the short live-offer default, up to the blob TTL.
#[derive(Deserialize)]
struct InboxDepositQuery {
    ttl: Option<u64>,
}

/// Deposit a sealed offer in a recipient's inbox slot. Returns the relay-assigned
/// id (the recipient deletes by it after handling). An optional
/// `x-arvolo-poster-hash` header (base32 BLAKE3 of a retract token) lets the
/// poster later delete this offer.
async fn inbox_post_handler(
    State(state): State<AppState>,
    AxumPath(slot): AxumPath<String>,
    Query(q): Query<InboxDepositQuery>,
    ip: ClientIp,
    headers: HeaderMap,
    body: Bytes,
) -> Result<String, (StatusCode, String)> {
    let now = now_unix();
    write_rate_limit(&state.write_limiter, ip.0, now, writes_per_min())?;
    if body.len() > MAX_INBOX_VALUE_BYTES {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "offer too large".into()));
    }
    // Global disk-fill guard, then per-slot flood guard (one victim's inbox can't
    // be filled without bound, and the sender can't drown real offers).
    if state.mailbox.inbox_count() >= max_inbox_rows() {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "relay at capacity".into()));
    }
    if state.mailbox.inbox_count_slot(&slot, now) >= MAX_INBOX_PER_SLOT {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "inbox full".into()));
    }
    let poster_hash = headers
        .get(INBOX_POSTER_HASH_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            // The base32 alphabet is uppercase; accept a lowercased header too.
            data_encoding::BASE32_NOPAD
                .decode(s.trim().to_uppercase().as_bytes())
                .ok()
        })
        .unwrap_or_default();
    let id = random_claim();
    // Clamp a caller-requested TTL to the cap; default to the short live-offer TTL.
    let ttl = q.ttl.unwrap_or(INBOX_TTL_SECS).min(INBOX_MAX_TTL_SECS);
    let exp = now.saturating_add(ttl);
    state
        .mailbox
        .inbox_put(&slot, &id, &body, exp, &poster_hash)
        .map_err(err_response)?;
    Ok(id)
}

/// Refresh a presence beacon: mark this slot's owner online for `PRESENCE_TTL_SECS`.
/// Unauthenticated by design — a spoofed "online" only triggers the sender's
/// offline fallback, and no one can force another slot offline (there is no delete).
async fn presence_post_handler(
    State(state): State<AppState>,
    AxumPath(slot): AxumPath<String>,
    ip: ClientIp,
) -> Result<StatusCode, (StatusCode, String)> {
    write_rate_limit(&state.write_limiter, ip.0, now_unix(), writes_per_min())?;
    if state.mailbox.beacon_count() >= max_presence_rows()
        && !state.mailbox.beacon_alive(&slot, now_unix())
    {
        // At capacity and this is a new slot — refuse (refreshing an existing
        // beacon is always allowed so live clients don't drop offline).
        return Err((StatusCode::INSUFFICIENT_STORAGE, "relay at capacity".into()));
    }
    let exp = now_unix().saturating_add(PRESENCE_TTL_SECS);
    state.mailbox.beacon_put(&slot, exp).map_err(err_response)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Is the owner of this slot currently online? 200 if a live beacon exists, else 404.
async fn presence_get_handler(
    State(state): State<AppState>,
    AxumPath(slot): AxumPath<String>,
) -> StatusCode {
    if state.mailbox.beacon_alive(&slot, now_unix()) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

/// Long-poll a recipient's inbox. Returns immediately with any queued offers;
/// otherwise holds the connection up to `?wait=<secs>` (capped) before returning
/// an empty body. Reading does **not** burn — the recipient acks with DELETE.
async fn inbox_get_handler(
    State(state): State<AppState>,
    AxumPath(slot): AxumPath<String>,
    Query(q): Query<InboxWait>,
    headers: HeaderMap,
) -> Result<Bytes, (StatusCode, String)> {
    if !inbox_authorized(&headers, &slot, &state.auth_secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "inbox read requires a session".into(),
        ));
    }
    let deadline = now_unix().saturating_add(q.wait.min(INBOX_MAX_WAIT_SECS));
    loop {
        let rows = state.mailbox.inbox_list(&slot, now_unix());
        if !rows.is_empty() {
            // A recipient client is reading: mark these offers arrived, so their
            // posters can tell a client that is really there from stale presence.
            // Arrival only — a listing lands here exactly as a daemon poll does,
            // and neither is anyone looking, let alone deciding. The `taken` stamp
            // comes later, on the ack, if the file is actually taken.
            let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
            state.mailbox.inbox_mark_arrived(&slot, &ids, now_unix());
            let items: Vec<arvolo_core::presence::InboxItem> = rows
                .into_iter()
                .map(|(id, blob)| arvolo_core::presence::InboxItem { id, blob })
                .collect();
            let bytes = arvolo_core::presence::encode_inbox_items(&items)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            return Ok(Bytes::from(bytes));
        }
        if now_unix() >= deadline {
            return Ok(Bytes::new());
        }
        tokio::time::sleep(std::time::Duration::from_millis(INBOX_POLL_MS)).await;
    }
}

/// Delete an offer. Authorized either by the recipient's session token (their ack
/// after handling) or by the poster's retract token (the sender withdrawing its
/// own offer — e.g. a live offer superseded by an offline fallback).
async fn inbox_delete_handler(
    State(state): State<AppState>,
    AxumPath((slot, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> StatusCode {
    // Poster retract: present the token whose hash we stored at POST.
    if let Some(token) = headers
        .get(INBOX_POSTER_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        if state
            .mailbox
            .inbox_delete_by_poster(&slot, &id, token.trim())
        {
            return StatusCode::NO_CONTENT;
        }
    }
    // Recipient ack: owner of the slot, proven by a session token. Recorded as
    // taken rather than deleted, so the poster gets an answer that an expiry can't
    // also produce; the row is emptied and reaped at its original TTL.
    if !inbox_authorized(&headers, &slot, &state.auth_secret) {
        return StatusCode::UNAUTHORIZED;
    }
    state.mailbox.inbox_mark_taken(&slot, &id, now_unix());
    StatusCode::NO_CONTENT
}

/// Poster-only status of one offer: has a live recipient seen it? Authorized by
/// the retract token (only the poster holds it). Body is a plain word:
/// `pending` / `fetched` / `gone`; a wrong/missing token is 401.
async fn inbox_status_handler(
    State(state): State<AppState>,
    AxumPath((slot, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Result<&'static str, StatusCode> {
    let token = headers
        .get(INBOX_POSTER_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    match state
        .mailbox
        .inbox_status_by_poster(&slot, &id, token.trim())
    {
        InboxStatus::BadToken => Err(StatusCode::UNAUTHORIZED),
        InboxStatus::Gone => Ok("gone"),
        InboxStatus::Pending => Ok("pending"),
        // Still spelled `fetched` on the wire, though the state is now called what
        // it always meant. A client older than this relay maps the word it knows to
        // "a live client has it" and keeps working; renaming it would have made
        // every such client read an unknown word, fall through to `pending`, and
        // abandon live sends to recipients who were right there. `taken` is purely
        // additive — an old client falls through to `pending` for it, which costs
        // nothing: by then the recipient has the file.
        InboxStatus::Arrived => Ok("fetched"),
        InboxStatus::Taken => Ok("taken"),
    }
}

/// Status of a deposited offline blob for its depositor: `pending` if still on the
/// relay, `gone` (404) if fetched (burn-after-read) or expired. The `claim` is a
/// secret capability, so this needs no extra auth.
#[derive(serde::Serialize)]
struct EntryStatusResp {
    status: &'static str,
    downloads: u32,
    max_downloads: u32,
}

async fn entry_status_handler(
    State(state): State<AppState>,
    AxumPath(claim): AxumPath<String>,
) -> Result<axum::Json<EntryStatusResp>, StatusCode> {
    match state.mailbox.entry_counts(&claim, now_unix()) {
        // JSON body carries the download accounting; older clients that only check
        // the status code (present vs 404) keep working unchanged.
        Some((downloads, max_downloads)) => Ok(axum::Json(EntryStatusResp {
            status: "pending",
            downloads,
            max_downloads,
        })),
        None => Err(StatusCode::NOT_FOUND),
    }
}

fn status_for(e: &MailboxError) -> StatusCode {
    match e {
        MailboxError::NotFound => StatusCode::NOT_FOUND,
        MailboxError::Expired | MailboxError::Exhausted => StatusCode::GONE,
        MailboxError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        MailboxError::Capacity => StatusCode::INSUFFICIENT_STORAGE,
        MailboxError::Forbidden => StatusCode::FORBIDDEN,
        MailboxError::Backend(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Map a mailbox error to an HTTP response. Internal backend errors (SQL, IO)
/// are logged server-side but never echoed to the client, to avoid leaking
/// implementation detail; all other variants are safe, caller-facing states.
fn err_response(e: MailboxError) -> (StatusCode, String) {
    match e {
        MailboxError::Backend(detail) => {
            tracing::error!(%detail, "relay backend error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
        }
        other => (status_for(&other), other.to_string()),
    }
}

async fn deposit_handler(
    State(state): State<AppState>,
    Query(q): Query<DepositQuery>,
    ip: ClientIp,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, (StatusCode, String)> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    write_rate_limit(&state.write_limiter, ip.0, now_unix(), writes_per_min())?;
    let mb = &state.mailbox;
    let encapped_key = headers
        .get(ENCAPPED_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            data_encoding::BASE32_NOPAD
                .decode(s.to_uppercase().as_bytes())
                .ok()
        })
        .ok_or((
            StatusCode::BAD_REQUEST,
            format!("missing/invalid {ENCAPPED_KEY_HEADER} header (base32)"),
        ))?;

    // A link deposit carries no HPKE recipient (empty encapped key). If the
    // administrator disabled links, refuse it here too (defense in depth beside
    // the /dl page and /v1/features advertisement).
    if !state.links_enabled && encapped_key.is_empty() {
        return Err((StatusCode::FORBIDDEN, LINKS_DISABLED_MSG.to_string()));
    }

    // Optional revoke-hash: base32 BLAKE3 of the sender's revoke token.
    let revoke_hash = headers
        .get(REVOKE_HASH_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            data_encoding::BASE32_NOPAD
                .decode(s.to_uppercase().as_bytes())
                .ok()
        })
        .unwrap_or_default();

    // Disk-fill guard before we start writing anything.
    if mb.at_capacity() {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "relay at capacity".into()));
    }

    // Aggregate disk-budget guard: refuse when the store is already over budget,
    // and bound this deposit to the remaining budget while it streams (so one
    // in-flight deposit can't blow past the total either).
    let total_cap = max_total_blob_bytes();
    let remaining_total = if total_cap != 0 {
        let stored = mb.stored_bytes();
        if stored >= total_cap {
            return Err((
                StatusCode::INSUFFICIENT_STORAGE,
                "relay at storage capacity".into(),
            ));
        }
        total_cap - stored
    } else {
        u64::MAX
    };

    // Stream the ciphertext straight to disk — never buffer the whole file in
    // memory — enforcing the per-blob size cap as we go (`0` = unlimited) and the
    // remaining aggregate budget. On any error or overflow the partial file is
    // removed.
    let cap = match max_blob_bytes() as u64 {
        0 => remaining_total,
        per_blob => per_blob.min(remaining_total),
    };
    let cap = usize::try_from(cap).unwrap_or(usize::MAX);
    let claim = mb.new_claim();
    let path = mb.blob_path(&claim);
    let mut file = tokio::fs::File::create(&path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("create blob: {e}"),
        )
    })?;
    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    let abort = |path: &std::path::Path, err: (StatusCode, String)| {
        let path = path.to_path_buf();
        async move {
            let _ = tokio::fs::remove_file(&path).await;
            err
        }
    };
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                return Err(
                    abort(&path, (StatusCode::BAD_REQUEST, format!("body read: {e}"))).await,
                )
            }
        };
        written = written.saturating_add(chunk.len() as u64);
        if cap != 0 && written > cap as u64 {
            return Err(abort(
                &path,
                (StatusCode::PAYLOAD_TOO_LARGE, "blob too large".into()),
            )
            .await);
        }
        if let Err(e) = file.write_all(&chunk).await {
            return Err(abort(
                &path,
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("write blob: {e}"),
                ),
            )
            .await);
        }
    }
    if let Err(e) = file.flush().await {
        return Err(abort(
            &path,
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("flush blob: {e}"),
            ),
        )
        .await);
    }
    drop(file);

    match mb.commit_deposit(
        &claim,
        encapped_key,
        q.ttl,
        q.max,
        revoke_hash,
        written,
        now_unix(),
    ) {
        // The body is still the bare claim, as every released client expects; the
        // granted TTL rides alongside it in a header, which an older client ignores.
        Ok(granted) => Ok(([(GRANTED_TTL_HEADER, granted.to_string())], claim).into_response()),
        Err(e) => {
            let _ = tokio::fs::remove_file(&path).await;
            Err(err_response(e))
        }
    }
}

/// Revoke (delete) an entry, authorized by the sender's revoke token supplied in
/// the `x-arvolo-revoke-token` header.
async fn revoke_handler(
    State(state): State<AppState>,
    AxumPath(claim): AxumPath<String>,
    headers: HeaderMap,
) -> Result<String, (StatusCode, String)> {
    let token = headers
        .get(REVOKE_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or((
            StatusCode::BAD_REQUEST,
            format!("missing {REVOKE_TOKEN_HEADER} header"),
        ))?;
    state.mailbox.revoke(&claim, token).map_err(err_response)?;
    Ok("revoked".into())
}

async fn fetch_handler(
    State(state): State<AppState>,
    AxumPath(claim): AxumPath<String>,
) -> Result<Response, (StatusCode, String)> {
    // Metadata decision under the DB lock (fast); the blob file is then read
    // off-lock and off the async worker via tokio::fs, so one large/slow download
    // can't hold the SQLite mutex and stall every other relay request.
    let plan = state
        .mailbox
        .fetch_plan(&claim, now_unix())
        .map_err(err_response)?;
    // Stream the blob straight off disk — never buffer a whole (possibly multi-GB)
    // file in memory. Open first, then, for a burn-after-read claim, unlink the
    // path immediately: the open handle keeps the bytes alive until this response
    // finishes, so they are still served while the file is already gone.
    let file = tokio::fs::File::open(&plan.blob_path).await.map_err(|e| {
        tracing::error!(error = %e, "open blob file");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?;
    let len = file.metadata().await.ok().map(|m| m.len());
    if plan.burn {
        let _ = tokio::fs::remove_file(&plan.blob_path).await;
    }
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let mut resp = Response::new(body);
    let encoded = data_encoding::BASE32_NOPAD.encode(&plan.encapped_key);
    if let Ok(val) = encoded.parse() {
        resp.headers_mut().insert(ENCAPPED_KEY_HEADER, val);
    }
    if let Some(len) = len {
        if let Ok(val) = len.to_string().parse() {
            resp.headers_mut()
                .insert(axum::http::header::CONTENT_LENGTH, val);
        }
    }
    Ok(resp)
}

/// Seed (backfill) a P2P blob into the relay's store. Body = the sender's blob
/// ticket; returns the relay's provider address (base32) so the sender can
/// advertise the relay as a fallback provider.
async fn seed_handler(
    State(state): State<AppState>,
    ip: ClientIp,
    body: String,
) -> Result<String, (StatusCode, String)> {
    write_rate_limit(&state.write_limiter, ip.0, now_unix(), writes_per_min())?;
    let req = SeedRequest::decode(body.trim())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad seed request: {e}")))?;
    if req.chunks.len() > MAX_SEED_CHUNKS_PER_REQ {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("too many chunks (max {MAX_SEED_CHUNKS_PER_REQ})"),
        ));
    }
    // Aggregate disk-footprint guard for the unauthenticated seed path: refuse if
    // storing these chunks would exceed the total pending-seed cap.
    if state
        .mailbox
        .seeded_count()
        .saturating_add(req.chunks.len() as i64)
        > max_seeded_rows()
    {
        return Err((
            StatusCode::INSUFFICIENT_STORAGE,
            "relay at seed capacity".into(),
        ));
    }
    // Free-tier per-session offload cap: bound how many bytes any one transfer may
    // lean on this relay (keyed on the content-derived swarm id, so it is durable
    // across the sender suspending/resuming/restarting). `0` = unlimited. Meter by
    // nominal chunk size (16 MiB); the last chunk is smaller, so this slightly
    // over-counts, which only makes the cap marginally stricter. On PAYMENT_REQUIRED
    // the sender falls back to direct P2P (or a private, uncapped relay).
    let cap = max_session_relay_bytes();
    let add_bytes = (req.chunks.len() as u64).saturating_mul(CHUNK_SIZE as u64);
    if cap > 0
        && state
            .mailbox
            .session_bytes(&req.swarm_id, now_unix())
            .saturating_add(add_bytes)
            > cap
    {
        return Err((StatusCode::PAYMENT_REQUIRED, cap.to_string()));
    }
    state
        .blobs
        .seed_chunks(req.sender, &req.chunks)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("seed failed: {e}")))?;
    let exp = now_unix().saturating_add(seed_ttl());
    for hash in &req.chunks {
        state
            .mailbox
            .record_seed(&req.token, &hash.to_string(), exp)
            .map_err(err_response)?;
    }
    if cap > 0 {
        let _ = state
            .mailbox
            .add_session_bytes(&req.swarm_id, add_bytes, exp);
    }
    Ok("ok".into())
}

/// The relay's iroh blob-node address plus a fresh transfer token, so the sender
/// can advertise the relay as a provider and use the token to seed/release.
///
/// Counted against the same per-IP write budget as the actual writes: this is
/// unauthenticated and mints a token per call, which made it the one remaining
/// endpoint an anonymous caller could hammer for free while every other
/// mutation on the relay was already limited.
async fn addr_handler(
    State(state): State<AppState>,
    ip: ClientIp,
) -> Result<String, (StatusCode, String)> {
    write_rate_limit(&state.write_limiter, ip.0, now_unix(), writes_per_min())?;
    let addr = state
        .blobs
        .addr_encoded()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("addr: {e}")))?;
    Ok(format!("{addr}\n{}", random_claim()))
}

/// TTL (seconds) for seeded chunks not yet released. Default 24h.
fn seed_ttl() -> u64 {
    std::env::var("ARVOLO_SEED_TTL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24 * 3600)
}

/// Incremental cleanup: the receiver calls this for each chunk as it gets it, so
/// the relay frees that chunk during the download (TTL is only a backstop).
async fn release_handler(
    State(state): State<AppState>,
    AxumPath((token, hash)): AxumPath<(String, String)>,
) -> Result<String, (StatusCode, String)> {
    if state.mailbox.seed_exists(&token, &hash) {
        state.blobs.release_hex(&hash).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("release failed: {e}"),
            )
        })?;
        let _ = state.mailbox.delete_seed_one(&token, &hash);
    }
    Ok("ok".into())
}

#[cfg(test)]
mod rz_v2_tests {
    use super::*;

    #[test]
    fn key_grammar() {
        for ok in ["ms", "own", "r.abc", "t.a-b_c", "x", "0"] {
            assert!(valid_rz_key(ok), "{ok:?} should be valid");
        }
        for bad in [
            "",
            "MS",
            "-a",
            ".a",
            "_a",
            "a b",
            "a/b",
            "a%b",
            &"x".repeat(65),
        ] {
            assert!(!valid_rz_key(bad), "{bad:?} should be invalid");
        }
        assert!(valid_rz_key(&"x".repeat(64)));
    }

    #[test]
    fn session_prefixes_need_a_session_id() {
        assert_eq!(rz_session_prefix("r.abc"), Some(RZ_P_RECV));
        assert_eq!(rz_session_prefix("s.abc"), Some(RZ_P_SEND));
        assert_eq!(rz_session_prefix("c.abc"), Some(RZ_P_CONF));
        assert_eq!(rz_session_prefix("t.abc"), Some(RZ_P_TKT));
        // A bare prefix is not a session, and neither is anything else.
        for not in ["r.", "s.", "own", "sessions", "ms", "mr", "tkt", "rabc"] {
            assert_eq!(rz_session_prefix(not), None, "{not:?}");
        }
    }

    #[test]
    fn renewal_never_outlives_the_absolute_cap() {
        let now = now_unix();

        // A fresh slot leases a full hour at a time.
        assert_eq!(rz_v2_expiry(now), now + RZ_V2_LEASE_SECS);

        // A slot near its ceiling gets only what is left of the 24 hours — this is
        // what stops a renewable slot from being a permanent nameplate squat.
        let old = now - (RZ_MAX_SLOT_LIFETIME - 100);
        assert_eq!(rz_v2_expiry(old), old + RZ_MAX_SLOT_LIFETIME);
        assert!(rz_v2_expiry(old) < now + RZ_V2_LEASE_SECS);

        // Past the ceiling the expiry is already behind us, which the handlers
        // turn into 410 Gone rather than a silently un-renewed slot.
        let ancient = now - (RZ_MAX_SLOT_LIFETIME + 10);
        assert!(rz_v2_expiry(ancient) <= now);
    }
}

#[cfg(test)]
mod page_lang_tests {
    use super::*;

    fn lang(accept: &str) -> &'static str {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::ACCEPT_LANGUAGE,
            accept.parse().expect("header value"),
        );
        negotiate_lang(&h)
    }

    #[test]
    fn plain_tags_and_regions_resolve_to_a_page_language() {
        assert_eq!(lang("it"), "it");
        assert_eq!(lang("de-AT"), "de");
        assert_eq!(lang("FR-ca"), "fr");
        assert_eq!(negotiate_lang(&HeaderMap::new()), "en");
    }

    #[test]
    fn quality_beats_position_and_unknown_tags_are_skipped() {
        // The reader's own order decides between equals...
        assert_eq!(lang("fr,it"), "fr");
        // ...but an explicit q wins over it, which is the whole point of the header.
        assert_eq!(lang("fr;q=0.4,it;q=0.9"), "it");
        // A language we don't speak must not consume the choice.
        assert_eq!(lang("es-ES,es;q=0.9,de;q=0.5"), "de");
        // `q=0` means "not this one"; with nothing else on offer we fall back.
        assert_eq!(lang("it;q=0"), "en");
        // Nothing recognisable at all, including the wildcard.
        assert_eq!(lang("*"), "en");
        assert_eq!(lang("zh-CN,ja;q=0.8"), "en");
        // A malformed q is untrusted rather than fatal: it loses to a real one.
        assert_eq!(lang("de;q=banana,it;q=0.3"), "it");
    }
}
