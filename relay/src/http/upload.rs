//! `/v1/upload`: the resumable form of `/v1/deposit`.
//!
//! ```text
//!   POST   /v1/upload?ttl=&max=      open    → 201, body = upload id
//!          x-arvolo-upload-length: N           (plus the deposit's own headers)
//!   GET    /v1/upload/{id}           status  → 204, x-arvolo-upload-offset / -length
//!   PATCH  /v1/upload/{id}           append  → 204, x-arvolo-upload-offset (new end)
//!          x-arvolo-upload-offset: o           (must equal the current end)
//!   POST   /v1/upload/{id}/commit    commit  → 200, body = claim, x-arvolo-ttl
//!   DELETE /v1/upload/{id}           abandon → 204
//! ```
//!
//! An append that names the wrong offset is answered `409` with the real one
//! in `x-arvolo-upload-offset`, so a client that lost track — a request that
//! timed out after the bytes had landed, say — learns where to go on from in
//! the same round trip. A commit of an incomplete upload is a `409` of the
//! same shape. See `docs/PROTOCOL.md` for the client's side.

use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use super::{
    err_response, DepositQuery, ENCAPPED_KEY_HEADER, GRANTED_TTL_HEADER, LINKS_DISABLED_MSG,
    MAX_BLOB_HEADER, REVOKE_HASH_HEADER,
};
use crate::limits::{write_rate_limit, writes_per_min, ClientIp};
use crate::mailbox::{MailboxError, UploadOpen};
use crate::state::{max_blob_bytes, max_total_blob_bytes, AppState};
use crate::util::now_unix;

/// The full ciphertext length, declared when an upload is opened.
pub(crate) const UPLOAD_LENGTH_HEADER: &str = "x-arvolo-upload-length";
/// Where an append starts (request) / where the upload now ends (response).
pub(crate) const UPLOAD_OFFSET_HEADER: &str = "x-arvolo-upload-offset";

type HandlerResult = Result<Response, (StatusCode, String)>;

fn base32_header(headers: &HeaderMap, name: &str) -> Option<Vec<u8>> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            data_encoding::BASE32_NOPAD
                .decode(s.to_uppercase().as_bytes())
                .ok()
        })
}

fn u64_header(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok())
}

/// "Not there" for every reason a client should start a new session.
fn gone() -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, "no such upload".into())
}

fn offset_conflict(at: u64, why: &str) -> Response {
    (
        StatusCode::CONFLICT,
        [(UPLOAD_OFFSET_HEADER, at.to_string())],
        why.to_string(),
    )
        .into_response()
}

/// `POST /v1/upload` — open a session.
///
/// Every refusal a single-request deposit can meet is met here, before any
/// ciphertext is sent: links turned off, the per-file cap (413, naming the
/// cap), and the disk budget (507). The budget is checked against the declared
/// length and that length is then *reserved*, which a streaming deposit cannot
/// do — it only finds out it ran over once it has.
pub(super) async fn open_handler(
    State(state): State<AppState>,
    Query(q): Query<DepositQuery>,
    ip: ClientIp,
    headers: HeaderMap,
) -> HandlerResult {
    if !state.uploads_enabled {
        return Err(gone());
    }
    write_rate_limit(&state.write_limiter, ip.0, now_unix(), writes_per_min())?;
    let mb = &state.mailbox;
    let encapped_key = base32_header(&headers, ENCAPPED_KEY_HEADER).ok_or((
        StatusCode::BAD_REQUEST,
        format!("missing/invalid {ENCAPPED_KEY_HEADER} header (base32)"),
    ))?;
    if !state.links_enabled && encapped_key.is_empty() {
        return Err((StatusCode::FORBIDDEN, LINKS_DISABLED_MSG.to_string()));
    }
    let revoke_hash = base32_header(&headers, REVOKE_HASH_HEADER).unwrap_or_default();
    let length = u64_header(&headers, UPLOAD_LENGTH_HEADER).ok_or((
        StatusCode::BAD_REQUEST,
        format!("missing/invalid {UPLOAD_LENGTH_HEADER} header"),
    ))?;

    let per_blob = max_blob_bytes() as u64;
    if per_blob != 0 && length > per_blob {
        return Ok((
            StatusCode::PAYLOAD_TOO_LARGE,
            [(MAX_BLOB_HEADER, per_blob.to_string())],
            format!("blob too large (this relay accepts up to {per_blob} bytes)"),
        )
            .into_response());
    }
    if mb.at_capacity() {
        return Err((StatusCode::INSUFFICIENT_STORAGE, "relay at capacity".into()));
    }
    let total_cap = max_total_blob_bytes();
    if total_cap != 0 && mb.disk_bytes().saturating_add(length) > total_cap {
        return Err((
            StatusCode::INSUFFICIENT_STORAGE,
            "relay at storage capacity".into(),
        ));
    }

    let id = mb
        .open_upload(
            UploadOpen {
                encapped_key,
                revoke_hash,
                ttl_secs: q.ttl,
                max_downloads: q.max,
                length,
            },
            now_unix(),
        )
        .map_err(err_response)?;
    Ok((StatusCode::CREATED, id).into_response())
}

/// `GET /v1/upload/{id}` — where the upload stands.
pub(super) async fn status_handler(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> HandlerResult {
    if !state.uploads_enabled {
        return Err(gone());
    }
    let info = match state.mailbox.upload_info(&id, now_unix()) {
        Ok(i) => i,
        Err(MailboxError::NotFound) => return Err(gone()),
        Err(e) => return Err(err_response(e)),
    };
    Ok((
        StatusCode::NO_CONTENT,
        [
            (UPLOAD_OFFSET_HEADER, info.received.to_string()),
            (UPLOAD_LENGTH_HEADER, info.length.to_string()),
        ],
    )
        .into_response())
}

/// `PATCH /v1/upload/{id}` — append the body at the offset the request names.
///
/// Whatever arrives is kept, even when the body breaks off half-way: the next
/// append starts from the end of what is on disk, which this answers with —
/// or, when the connection is already gone, which the next status asks for.
/// Bytes past the declared length are refused, and the file is cut back to it.
pub(super) async fn append_handler(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> HandlerResult {
    if !state.uploads_enabled {
        return Err(gone());
    }
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let mb = &state.mailbox;
    let offset = u64_header(&headers, UPLOAD_OFFSET_HEADER).ok_or((
        StatusCode::BAD_REQUEST,
        format!("missing/invalid {UPLOAD_OFFSET_HEADER} header"),
    ))?;
    let Some(_guard) = mb.begin_append(&id) else {
        // Another request is writing this upload. Most likely the client's own
        // previous attempt, still draining after the client gave up on it; the
        // client asks for the status and tries again.
        return Err((
            StatusCode::CONFLICT,
            "upload is busy with another request".into(),
        ));
    };
    let info = match mb.upload_info(&id, now_unix()) {
        Ok(i) => i,
        Err(MailboxError::NotFound) => return Err(gone()),
        Err(e) => return Err(err_response(e)),
    };
    if info.committed.is_some() {
        return Ok(offset_conflict(info.length, "upload already committed"));
    }
    if offset != info.received {
        return Ok(offset_conflict(
            info.received,
            "offset does not match the upload",
        ));
    }

    let path = mb.upload_path(&id);
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .map_err(|_| gone())?;
    let mut end = offset;
    let mut stream = body.into_data_stream();
    let mut outcome: Option<(StatusCode, String)> = None;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                outcome = Some((StatusCode::BAD_REQUEST, format!("body read: {e}")));
                break;
            }
        };
        if end.saturating_add(chunk.len() as u64) > info.length {
            outcome = Some((
                StatusCode::BAD_REQUEST,
                "append runs past the declared upload length".into(),
            ));
            break;
        }
        if let Err(e) = file.write_all(&chunk).await {
            tracing::error!(%e, "upload append write failed");
            outcome = Some((StatusCode::INTERNAL_SERVER_ERROR, "internal error".into()));
            break;
        }
        end += chunk.len() as u64;
    }
    // Keep what landed, and only that: a write that failed half-way can leave a
    // torn tail, and the file length is what the next append trusts.
    let _ = file.flush().await;
    drop(file);
    if outcome.is_some() {
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&path) {
            let _ = f.set_len(end);
        }
    }
    mb.touch_upload(&id, now_unix());
    match outcome {
        None => Ok((
            StatusCode::NO_CONTENT,
            [(UPLOAD_OFFSET_HEADER, end.to_string())],
        )
            .into_response()),
        Some((status, msg)) => {
            Ok((status, [(UPLOAD_OFFSET_HEADER, end.to_string())], msg).into_response())
        }
    }
}

/// `POST /v1/upload/{id}/commit` — the upload becomes a mailbox entry.
///
/// Answers exactly as `/v1/deposit` does on success — the claim as the body,
/// the granted TTL in a header — so everything after the upload is the same
/// code on the client whichever way the bytes went up.
pub(super) async fn commit_handler(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    ip: ClientIp,
) -> HandlerResult {
    if !state.uploads_enabled {
        return Err(gone());
    }
    write_rate_limit(&state.write_limiter, ip.0, now_unix(), writes_per_min())?;
    let mb = &state.mailbox;
    // Not while an append is still writing: its last bytes would land after
    // the file had been moved.
    let Some(_guard) = mb.begin_append(&id) else {
        return Err((
            StatusCode::CONFLICT,
            "upload is busy with another request".into(),
        ));
    };
    match mb.commit_upload(&id, now_unix()) {
        Ok(Ok((claim, granted))) => {
            Ok(([(GRANTED_TTL_HEADER, granted.to_string())], claim).into_response())
        }
        Ok(Err(received)) => Ok(offset_conflict(received, "upload is not complete")),
        Err(MailboxError::NotFound) => Err(gone()),
        Err(e) => Err(err_response(e)),
    }
}

/// `DELETE /v1/upload/{id}` — abandon it now rather than at the idle deadline.
pub(super) async fn abort_handler(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> HandlerResult {
    if !state.uploads_enabled {
        return Err(gone());
    }
    let Some(_guard) = state.mailbox.begin_append(&id) else {
        return Err((
            StatusCode::CONFLICT,
            "upload is busy with another request".into(),
        ));
    };
    match state.mailbox.abort_upload(&id) {
        Ok(()) | Err(MailboxError::NotFound) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => Err(err_response(e)),
    }
}
