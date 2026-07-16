//! IPC wire contract shared by the daemon (`server`, in the CLI crate) and its
//! clients (`client`, here).
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
    /// The saved address book → [`Response::Contacts`]. Feeds the GUI's "Persone"
    /// grid; the daemon resolves names/fingerprints/verified marks from its ledgers.
    ListContacts,
    /// Send files/folders to a contact; paths are on the *daemon's* filesystem.
    /// → [`Response::TransferId`].
    Push {
        to: String,
        paths: Vec<String>,
        #[serde(default)]
        note: String,
    },
    /// Serve an anonymous P2P ticket in the background (no recipient); paths are on
    /// the *daemon's* filesystem. → [`Response::Served`].
    ServeTicket {
        paths: Vec<String>,
        seed_relay: Option<String>,
    },
    /// Deposit a public, browser-openable **download link** for `path` (on the
    /// daemon's filesystem) on the relay → [`Response::Link`]. `ttl` defaults to
    /// 7 days, `max` to unlimited when omitted.
    CreateLink {
        path: String,
        ttl: Option<u64>,
        max: Option<u32>,
    },
    /// Cancel a transfer by id → [`Response::Ok`].
    Cancel { id: u64 },
    /// Drop one **finished** transfer from the list (per-row "Elimina" in a UI).
    /// No-op for anything still in flight → [`Response::Ok`].
    Remove { id: u64 },
    /// Mark a saved contact (by name) verified after an out-of-band fingerprint
    /// check → [`Response::Ok`] (error if the name isn't a saved contact).
    MarkVerified { name: String },
    /// Everything this client has left on a relay and can still take back: public
    /// download links and sealed mailbox deposits → [`Response::Deposits`].
    ListDeposits,
    /// Withdraw a deposit from the relay by its id and forget the record. The blob
    /// stops being fetchable → [`Response::Ok`].
    RevokeDeposit { id: String },
    /// Pause an in-progress `send --to` by id → [`Response::Ok`].
    Pause { id: u64 },
    /// Resume a paused `send --to` by id → [`Response::Ok`].
    Resume { id: u64 },
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
    /// A background ticket-serve started: its transfer id + the `arvc…` ticket.
    Served {
        id: u64,
        ticket: String,
    },
    /// A public download link (`{relay}/dl/{claim}#{key}`).
    Link(String),
    Transfers(Vec<TransferDto>),
    Pending(Vec<OfferDto>),
    Contacts(Vec<ContactDto>),
    Deposits(Vec<DepositDto>),
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
    /// The daemon binary's version (`CARGO_PKG_VERSION` at its build time).
    /// Optional on the wire so a newer client can still read an **older** daemon's
    /// status (which predates this field) — it just shows up empty there.
    #[serde(default)]
    pub version: String,
    pub public_id: String,
    pub fingerprint: String,
    pub relay: Option<String>,
    pub transfers: usize,
    pub pending: usize,
    /// Where accepted downloads land by default. Optional on the wire so a newer
    /// client can still read an older daemon's status.
    #[serde(default)]
    pub download_dir: String,
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
    /// Swarm metrics (0 for a non-swarm transfer).
    #[serde(default)]
    pub swarm_peers: usize,
    #[serde(default)]
    pub pieces_from_peers: u64,
    /// For a send: distinct peers currently downloading from us.
    #[serde(default)]
    pub download_peers: usize,
    /// Unix seconds when the transfer began, so a UI can group history by real
    /// days. Optional on the wire: an older daemon predates it and reports 0.
    #[serde(default)]
    pub created: u64,
}

/// A parked incoming offer awaiting the user's accept/reject.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OfferDto {
    pub id: String,
    pub from: String,
    pub name: String,
    pub size: u64,
    pub note: String,
    #[serde(default)]
    pub sender_name: String,
}

/// An address-book entry for the send panel's "Persone" grid.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContactDto {
    /// The saved petname (the key passed back as `to` on a [`Request::Push`]).
    pub name: String,
    /// Base32 public id.
    pub id: String,
    /// Word fingerprint for display, if the stored id parses.
    #[serde(default)]
    pub fingerprint: String,
    /// Whether the contact's key has been verified out-of-band.
    #[serde(default)]
    pub verified: bool,
}

/// Something this client left on a relay and can still withdraw: a public download
/// link, or a sealed mailbox deposit awaiting its recipient. The revoke token is
/// deliberately **not** here — it is the sender's secret, it never needs to leave
/// the daemon, and a UI only ever needs the id to ask for a withdrawal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DepositDto {
    pub id: String,
    /// "link" (public browser URL) or "offline" (sealed, for one recipient).
    pub kind: String,
    pub name: String,
    pub size: u64,
    /// The full browser URL, for a link. Empty for a sealed deposit.
    #[serde(default)]
    pub link: String,
    /// The recipient's base32 id, for a sealed deposit. Empty for a link.
    #[serde(default)]
    pub recipient: String,
    pub created: u64,
    /// Unix seconds when the relay drops it on its own.
    pub expires: u64,
    /// Already past `expires`: the relay has let it go, so there is nothing left to
    /// withdraw — only the local record to tidy away.
    pub expired: bool,
    /// Human download cap ("1 download", "nessun limite").
    #[serde(default)]
    pub max_label: String,
    /// Whether the relay still holds the blob. The local record is only a receipt
    /// of the deposit: it cannot know that someone downloaded a one-shot link or
    /// that a recipient collected a sealed deposit, because nothing reports that
    /// back. So this is asked of the relay when the list is built. `None` means the
    /// relay could not be reached — a UI must then say "unknown", never "alive".
    #[serde(default)]
    pub present: Option<bool>,
    /// How many times the relay has served the blob. `None` against a relay that
    /// could not be reached, or an older one that only reports presence.
    #[serde(default)]
    pub downloads: Option<u32>,
    /// The relay's own download cap, which may be lower than the one requested
    /// (the relay clamps to its maximum). `None` as for `downloads`.
    #[serde(default)]
    pub max_downloads: Option<u32>,
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
        #[serde(default)]
        note: String,
        #[serde(default)]
        sender_name: String,
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
    Waiting {
        id: u64,
        reason: String,
    },
    Paused {
        id: u64,
        reason: String,
    },
    Failed {
        id: u64,
        error: String,
    },
    Cancelled {
        id: u64,
    },
    /// The address book changed under the daemon. Carries nothing: re-issue
    /// [`Request::ListContacts`] to pick the new book up.
    ContactsChanged,
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
        TransferStatus::Waiting(r) => format!("waiting: {r}"),
        TransferStatus::Paused(r) => format!("paused: {r}"),
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
            swarm_peers: t.swarm_peers,
            pieces_from_peers: t.pieces_from_peers,
            download_peers: t.download_peers,
            created: t.created,
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
                note,
                sender_name,
            } => EventDto::OfferReceived {
                id: id.clone(),
                from: encode_id(from),
                name: name.clone(),
                size: *size,
                note: note.clone(),
                sender_name: sender_name.clone(),
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
            ManagerEvent::Waiting { id, reason } => EventDto::Waiting {
                id: *id,
                reason: reason.clone(),
            },
            ManagerEvent::Paused { id, reason } => EventDto::Paused {
                id: *id,
                reason: reason.clone(),
            },
            ManagerEvent::Failed { id, error } => EventDto::Failed {
                id: *id,
                error: error.clone(),
            },
            ManagerEvent::Cancelled { id } => EventDto::Cancelled { id: *id },
            ManagerEvent::ContactsChanged => EventDto::ContactsChanged,
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
                note: "have a look".into(),
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

    /// The GUI's TypeScript mirrors these bytes by hand (`gui/src/events.ts`), so a
    /// change in serde's representation must fail loudly here rather than silently
    /// stop every live update in the app. This exact shape — externally tagged, a
    /// unit variant as a bare string — is what the frontend's `normalizeEvent`
    /// flattens. It was already once assumed to be `{"type":"started",...}`; it is
    /// not, and the board quietly stopped refreshing.
    #[test]
    fn event_wire_format_is_stable() {
        let cases: Vec<(EventDto, &str)> = vec![
            (
                EventDto::Started {
                    id: 3,
                    direction: "send".into(),
                    name: "evtest.txt".into(),
                    total_size: 6,
                },
                r#"{"started":{"id":3,"direction":"send","name":"evtest.txt","total_size":6}}"#,
            ),
            (
                EventDto::Progress {
                    id: 1,
                    transferred: 5,
                    total_size: 10,
                },
                r#"{"progress":{"id":1,"transferred":5,"total_size":10}}"#,
            ),
            (
                EventDto::Completed { id: 1, path: None },
                r#"{"completed":{"id":1,"path":null}}"#,
            ),
            (EventDto::Deposited { id: 2 }, r#"{"deposited":{"id":2}}"#),
            (
                EventDto::Waiting {
                    id: 3,
                    reason: "relay unavailable".into(),
                },
                r#"{"waiting":{"id":3,"reason":"relay unavailable"}}"#,
            ),
            (
                EventDto::Failed {
                    id: 5,
                    error: "boom".into(),
                },
                r#"{"failed":{"id":5,"error":"boom"}}"#,
            ),
            (EventDto::Cancelled { id: 6 }, r#"{"cancelled":{"id":6}}"#),
            // A unit variant is a bare string, NOT an object — the frontend has to
            // handle both forms.
            (EventDto::ContactsChanged, r#""contacts_changed""#),
        ];
        for (ev, want) in cases {
            assert_eq!(
                serde_json::to_string(&ev).unwrap(),
                want,
                "wire shape changed — update gui/src/events.ts to match"
            );
        }
    }

    #[test]
    fn contacts_response_roundtrips() {
        let r = Response::Contacts(vec![ContactDto {
            name: "alice".into(),
            id: "if2xmne".into(),
            fingerprint: "able-otter-nine".into(),
            verified: true,
        }]);
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
    }
}
