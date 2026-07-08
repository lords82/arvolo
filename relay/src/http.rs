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
    rz_posts_per_min, rz_rate_limit, rz_slots_per_min, write_rate_limit, writes_per_min, ClientIp,
    RzAction,
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

/// Shown (with 403) when the administrator has disabled download links.
const DL_DISABLED_HTML: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>arvolo</title></head>\
<body style=\"font-family:system-ui,sans-serif;background:#0b0d10;color:#e7ebf0;display:grid;\
place-items:center;min-height:100vh;margin:0;padding:24px\"><main style=\"max-width:420px;text-align:center\">\
<h1 style=\"font-size:17px;margin:0 0 8px\">Download links are turned off</h1>\
<p style=\"color:#8b95a3;font-size:13px;line-height:1.5\">The administrator of this relay has disabled \
public browser download links. Ask the sender to share the file another way.</p></main></body></html>";

const LINKS_DISABLED_MSG: &str = "public download links are disabled by this relay's administrator";

fn links_disabled_page() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        DL_DISABLED_HTML,
    )
        .into_response()
}

async fn dl_page_handler(State(state): State<AppState>) -> Response {
    if !state.links_enabled {
        return links_disabled_page();
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

/// Advertise this relay's optional features so a client can fail fast. Currently
/// just `links` (public browser download links).
async fn features_handler(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        format!("{{\"links\":{}}}", state.links_enabled),
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
            post(rz_post_handler).get(rz_get_handler),
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

/// Store a rendezvous value. The claim key (`ms`) fails with 409 if the slot is
/// already taken, so the sender can pick a fresh nameplate.
async fn rz_post_handler(
    State(state): State<AppState>,
    AxumPath((slot, key)): AxumPath<(String, String)>,
    ip: ClientIp,
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
    let exp = now_unix().saturating_add(RZ_TTL);
    // First-writer-wins for EVERY rendezvous key, not just the slot claim (`ms`).
    // Each key of a pairing (`ms` sender msg, `mr` receiver msg, `tkt` encrypted
    // ticket) is legitimately written exactly once; allowing overwrite (the old
    // INSERT-OR-REPLACE on `mr`/`tkt`) let anyone who guesses the slot (a 4-digit
    // nameplate, only 10k values) clobber an in-flight ticket/message and grief the
    // pairing. Claiming on first write closes that without affecting the honest flow.
    let claimed = state
        .mailbox
        .rz_claim(&slot, &key, &body, exp)
        .map_err(err_response)?;
    if !claimed {
        let msg = if key == RZ_CLAIM_KEY {
            "slot already taken"
        } else {
            "rendezvous key already written"
        };
        return Err((StatusCode::CONFLICT, msg.into()));
    }
    Ok("ok".into())
}

/// Read a rendezvous value (404 until posted). Reading the ticket burns the slot.
async fn rz_get_handler(
    State(state): State<AppState>,
    AxumPath((slot, key)): AxumPath<(String, String)>,
    ip: ClientIp,
) -> Result<Bytes, (StatusCode, String)> {
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
    // authenticate for it.
    if arvolo_core::presence::slot_for(&body) != slot {
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
            // A live recipient is polling: mark these offers seen so their posters
            // can tell "delivered to an online client" from "stale presence".
            let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
            state.mailbox.inbox_mark_fetched(&slot, &ids, now_unix());
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
    // Recipient ack: owner of the slot, proven by a session token.
    if !inbox_authorized(&headers, &slot, &state.auth_secret) {
        return StatusCode::UNAUTHORIZED;
    }
    state.mailbox.inbox_delete(&slot, &id);
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
        InboxStatus::Fetched => Ok("fetched"),
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
) -> Result<String, (StatusCode, String)> {
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
        Ok(()) => Ok(claim),
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
async fn addr_handler(State(state): State<AppState>) -> Result<String, (StatusCode, String)> {
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
