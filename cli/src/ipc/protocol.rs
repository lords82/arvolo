//! IPC wire contract shared by the daemon (`server`) and its clients (`client`).
//!
//! The transport is a local socket carrying **newline-delimited JSON**: one JSON
//! value per line. A client sends a [`RequestEnvelope`] and the daemon answers
//! with a [`ServerMessage::Reply`] correlated by `id`; a [`Request::Subscribe`]
//! instead switches the connection to a stream of [`ServerMessage::Event`] lines.
//!
//! The core engine types (`Transfer`, `ManagerEvent`, `PublicId`, `PathBuf`) are
//! not `serde`-serializable, so this module defines parallel DTOs with base32
//! string peer ids and string paths, plus `From` conversions. The core types are
//! never derived onto the wire.

use arvolo_core::crypto::PublicId;
use arvolo_core::manager::{Direction, ManagerEvent, Transfer, TransferStatus};
use serde::{Deserialize, Serialize};

/// Base32 (no-pad, lowercase) encoding of a public id — the canonical wire form,
/// matching the CLI's `encode_id`.
fn encode_id(p: &PublicId) -> String {
    data_encoding::BASE32_NOPAD
        .encode(&p.to_bytes())
        .to_lowercase()
}

// ---- request/response -----------------------------------------------------

/// A client request, wrapped in a [`RequestEnvelope`] on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    /// Liveness check → [`Response::Pong`].
    Ping,
    /// Daemon status snapshot → [`Response::Status`].
    Status,
    /// All known transfers (any status) → [`Response::Transfers`].
    ListTransfers,
    /// Offers parked awaiting the user's approval → [`Response::Pending`].
    ListPending,
    /// Send files/folders to a contact; paths are on the *daemon's* filesystem.
    /// → [`Response::TransferId`].
    Push { to: String, paths: Vec<String> },
    /// Cancel a transfer by id → [`Response::Ok`].
    Cancel { id: u64 },
    /// Accept a parked offer, optionally to a specific path → [`Response::TransferId`].
    AcceptOffer {
        offer_id: String,
        out: Option<String>,
    },
    /// Reject a parked offer → [`Response::Ok`].
    RejectOffer { offer_id: String },
    /// Turn this connection into an event stream (no further requests on it).
    Subscribe,
}

/// The daemon's answer to a [`Request`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Pong,
    Status(StatusDto),
    TransferId(u64),
    Transfers(Vec<TransferDto>),
    Pending(Vec<OfferDto>),
    Ok,
    Error(String),
}

/// Client → daemon: a request tagged with a correlation `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub id: u32,
    pub cmd: Request,
}

/// Daemon → client: either a correlated reply or a pushed event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerMessage {
    /// Answer to the request with the matching `id`.
    Reply { id: u32, result: Response },
    /// An engine event pushed to a subscribed connection.
    Event(EventDto),
}

// ---- DTOs -----------------------------------------------------------------

/// Serializable snapshot of a daemon's identity/config for the status view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusDto {
    pub public_id: String,
    pub fingerprint: String,
    pub relay: Option<String>,
    pub transfers: usize,
    pub pending: usize,
}

/// Serializable mirror of [`Transfer`] with a base32 peer id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransferDto {
    pub id: u64,
    /// "send" or "recv".
    pub direction: String,
    pub peer: Option<String>,
    pub name: String,
    pub total_size: u64,
    pub transferred: u64,
    /// "active" | "completed" | "deposited" | "cancelled" | "failed: <msg>".
    pub status: String,
}

/// A parked incoming offer awaiting the user's accept/reject.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OfferDto {
    pub id: String,
    pub from: String,
    pub name: String,
    pub size: u64,
}

/// Serializable mirror of [`ManagerEvent`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EventDto {
    OfferReceived {
        id: String,
        from: String,
        name: String,
        size: u64,
    },
    Started {
        id: u64,
        direction: String,
        name: String,
        total_size: u64,
    },
    Progress {
        id: u64,
        transferred: u64,
        total_size: u64,
    },
    Completed {
        id: u64,
        path: Option<String>,
    },
    Deposited {
        id: u64,
    },
    Failed {
        id: u64,
        error: String,
    },
    Cancelled {
        id: u64,
    },
}

fn direction_str(d: Direction) -> &'static str {
    match d {
        Direction::Send => "send",
        Direction::Recv => "recv",
    }
}

fn status_str(s: &TransferStatus) -> String {
    match s {
        TransferStatus::Active => "active".into(),
        TransferStatus::Completed => "completed".into(),
        TransferStatus::Deposited => "deposited".into(),
        TransferStatus::Cancelled => "cancelled".into(),
        TransferStatus::Failed(e) => format!("failed: {e}"),
    }
}

impl From<&Transfer> for TransferDto {
    fn from(t: &Transfer) -> Self {
        TransferDto {
            id: t.id,
            direction: direction_str(t.direction).into(),
            peer: t.peer.as_ref().map(encode_id),
            name: t.name.clone(),
            total_size: t.total_size,
            transferred: t.transferred,
            status: status_str(&t.status),
        }
    }
}

impl From<&ManagerEvent> for EventDto {
    fn from(ev: &ManagerEvent) -> Self {
        match ev {
            ManagerEvent::OfferReceived {
                id,
                from,
                name,
                size,
            } => EventDto::OfferReceived {
                id: id.clone(),
                from: encode_id(from),
                name: name.clone(),
                size: *size,
            },
            ManagerEvent::Started {
                id,
                direction,
                name,
                total_size,
            } => EventDto::Started {
                id: *id,
                direction: direction_str(*direction).into(),
                name: name.clone(),
                total_size: *total_size,
            },
            ManagerEvent::Progress {
                id,
                transferred,
                total_size,
            } => EventDto::Progress {
                id: *id,
                transferred: *transferred,
                total_size: *total_size,
            },
            ManagerEvent::Completed { id, path } => EventDto::Completed {
                id: *id,
                path: path.as_ref().map(|p| p.display().to_string()),
            },
            ManagerEvent::Deposited { id } => EventDto::Deposited { id: *id },
            ManagerEvent::Failed { id, error } => EventDto::Failed {
                id: *id,
                error: error.clone(),
            },
            ManagerEvent::Cancelled { id } => EventDto::Cancelled { id: *id },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_envelope_roundtrips() {
        let env = RequestEnvelope {
            id: 7,
            cmd: Request::Push {
                to: "alice".into(),
                paths: vec!["a.txt".into(), "b/".into()],
            },
        };
        let line = serde_json::to_string(&env).unwrap();
        let back: RequestEnvelope = serde_json::from_str(&line).unwrap();
        assert_eq!(back.id, 7);
        assert_eq!(back.cmd, env.cmd);
    }

    #[test]
    fn unit_request_is_a_bare_string() {
        assert_eq!(serde_json::to_string(&Request::Ping).unwrap(), "\"ping\"");
        let back: Request = serde_json::from_str("\"list_pending\"").unwrap();
        assert_eq!(back, Request::ListPending);
    }

    #[test]
    fn server_message_reply_and_event_are_distinguishable() {
        let reply = ServerMessage::Reply {
            id: 3,
            result: Response::TransferId(42),
        };
        let ev = ServerMessage::Event(EventDto::Deposited { id: 42 });
        let rl = serde_json::to_string(&reply).unwrap();
        let el = serde_json::to_string(&ev).unwrap();
        assert!(rl.contains("\"reply\""));
        assert!(el.contains("\"event\""));
        // Round-trip both.
        assert!(matches!(
            serde_json::from_str::<ServerMessage>(&rl).unwrap(),
            ServerMessage::Reply {
                id: 3,
                result: Response::TransferId(42)
            }
        ));
        assert!(matches!(
            serde_json::from_str::<ServerMessage>(&el).unwrap(),
            ServerMessage::Event(EventDto::Deposited { id: 42 })
        ));
    }

    #[test]
    fn response_error_roundtrips() {
        let r = Response::Error("nope".into());
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
    }
}
