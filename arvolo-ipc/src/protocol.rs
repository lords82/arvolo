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
    ///
    /// The mailbox options mirror `arvolo send`'s: they only bite when the send
    /// actually lands in the relay mailbox — either because `deposit` forced it,
    /// or because the recipient turned out to be offline. All four carry
    /// `#[serde(default)]` so a daemon that predates them still decodes a newer
    /// client's `Push` (it just ignores the options) instead of failing the
    /// request outright.
    Push {
        to: String,
        paths: Vec<String>,
        #[serde(default)]
        note: String,
        /// Skip the live attempt and deposit on the relay even if they're online.
        #[serde(default)]
        deposit: bool,
        /// Mailbox time-to-live in seconds (daemon default: 7 days).
        #[serde(default)]
        ttl: Option<u64>,
        /// Max downloads before the relay deletes the blob (daemon default: 1).
        #[serde(default)]
        max: Option<u32>,
        /// End-to-end password on the deposit; the recipient must supply it.
        #[serde(default)]
        password: Option<String>,
    },
    /// Serve an anonymous P2P ticket in the background (no recipient); paths are on
    /// the *daemon's* filesystem. → [`Response::Served`].
    ServeTicket {
        paths: Vec<String>,
        seed_relay: Option<String>,
    },
    /// Serve a short pairing code in the background: the daemon hosts the
    /// rendezvous *and* the ticket behind it. → [`Response::CodeServed`].
    ///
    /// `keep` serves every receiver until cancelled instead of retiring the code
    /// after the first. A daemon too old to know this variant answers
    /// `Error("bad request: …")` (see the recovery in the daemon's `handle_conn`),
    /// which the CLI reads as "fall back to serving it in the foreground".
    ServeCode {
        paths: Vec<String>,
        relay: Option<String>,
        #[serde(default)]
        keep: bool,
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
    /// Drop **every** finished transfer (completed / cancelled / failed) from the
    /// list → [`Response::Cleared`] with the count. Anything still in flight stays,
    /// including a Deposited send awaiting its pickup. The history log is a separate
    /// store and is not touched.
    ClearFinished,
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
    ///
    /// `password` is for the case where the offer points at a mailbox deposit
    /// sealed with one (`send --deposit --password`). Such an offer is
    /// indistinguishable from any other until the fetch fails, so a UI can send
    /// the accept, read the refusal, ask, and send it again. `#[serde(default)]`
    /// keeps the old two-field shape decodable.
    AcceptOffer {
        offer_id: String,
        out: Option<String>,
        #[serde(default)]
        password: Option<String>,
    },
    /// Reject a parked offer → [`Response::Ok`].
    RejectOffer { offer_id: String },
    /// Receive from a pasted artefact — the daemon-side mirror of `arvolo recv`.
    /// An `arvc…` ticket or a pairing code fetches live (the code is resolved to
    /// its ticket first); an offline `arvm…` ticket is fetched from the relay
    /// mailbox, unwrapping with `password` when it carries one. `out` is a file
    /// path or a directory on the *daemon's* filesystem (default: its download
    /// dir) → [`Response::TransferId`].
    Recv {
        ticket: String,
        out: Option<String>,
        password: Option<String>,
    },
    /// Save (or re-key) a contact → [`Response::Ok`]. Re-keying an existing name
    /// clears its verified/trusted marks, exactly as `arvolo contacts add` does —
    /// a UI should warn before sending that.
    AddContact { name: String, id: String },
    /// Remove a saved contact by name → [`Response::Ok`] (error if unknown).
    RemoveContact { name: String },
    /// Rename a contact, keeping its key and verified/trusted marks → [`Response::Ok`].
    RenameContact { old: String, new: String },
    /// Clear a contact's verified mark → [`Response::Ok`].
    MarkUnverified { name: String },
    /// Trust a contact/id to auto-download without a prompt → [`Response::Ok`].
    /// Refused for an unverified contact unless `force` — auto-downloading from
    /// an unconfirmed key is a MITM risk, same rule as `arvolo contacts trust`.
    MarkTrusted {
        who: String,
        #[serde(default)]
        force: bool,
    },
    /// Stop auto-downloading from a contact/id → [`Response::Ok`].
    MarkUntrusted { who: String },
    /// Silence an identity: its offers are dropped on arrival → [`Response::Ok`].
    Block { who: String },
    /// Let a blocked identity's offers through again → [`Response::Ok`].
    Unblock { who: String },
    /// Approve a contact's pending advertised display name → [`Response::Ok`].
    AcceptName { who: String },
    /// The log of finished transfers, newest first → [`Response::History`].
    ListHistory,
    /// Forget the whole history log → [`Response::Cleared`] with the count.
    ClearHistory,
    /// Set (or clear, with an empty string) the display name advertised inside
    /// offers this daemon sends → [`Response::Ok`]. Applies immediately to the
    /// running engine, and persists to config like `arvolo me name`.
    SetMyName { name: String },
    /// Everything on the settings screen: the effective relay and download folder,
    /// where they came from, and the raw `config.toml` values behind them
    /// → [`Response::Config`].
    GetConfig,
    /// Write settings back to `config.toml`. Every field is a three-state edit:
    /// absent leaves the key alone, a value sets it, and an explicit "clear"
    /// comments it out → [`Response::Config`] with the state that resulted, so a
    /// UI never has to guess what the daemon made of its patch.
    SetConfig(ConfigPatch),
    /// Drop advertised-name records left behind by contacts that no longer exist
    /// → [`Response::Cleared`] with the count (`arvolo contacts prune`).
    PruneNames,
    /// Ask the relay who, among these ids, has a live presence beacon
    /// → [`Response::Presence`]. Deliberately separate from
    /// [`Request::ListContacts`]: the book is read from disk and is instant,
    /// while this is a network round trip per contact, and folding the two would
    /// make every address-book refresh wait on a relay.
    Presence { ids: Vec<String> },
    /// Read-only multi-device summary: shared identity, book size, whether auto
    /// sync is on, and when a round last succeeded → [`Response::Sync`].
    SyncStatus,
    /// Run one address-book sync round against this identity's inbox cell now
    /// → [`Response::Sync`] reflecting the state after the round.
    SyncNow,
    /// Begin a pairing exchange. Pairing is not request/reply — it waits for a
    /// human on another machine — so this returns a session handle immediately
    /// (→ [`Response::PairingStarted`]) and the outcome arrives as
    /// [`EventDto::PairingCode`] / [`EventDto::PairingDone`] /
    /// [`EventDto::PairingFailed`] on the event stream.
    StartPairing {
        kind: PairKind,
        /// Hosting only: which relay to claim the code on. Defaults to the
        /// configured relay.
        #[serde(default)]
        relay: Option<String>,
        /// Joining only: the code read off the other machine.
        #[serde(default)]
        code: Option<String>,
        /// Contact pairing only: what to file the other person under. When absent
        /// the daemon names them after their fingerprint, since there is nobody at
        /// a prompt to ask.
        #[serde(default)]
        name: Option<String>,
    },
    /// Abandon a pairing session started above → [`Response::Ok`]. Also what a UI
    /// sends when its pairing sheet is closed: an unattended `device pair` would
    /// otherwise keep offering this device's identity secret for its full window.
    CancelPairing { session: String },
    /// Ask the daemon to exit cleanly → [`Response::Ok`] (sent before it goes).
    /// What `arvolo daemon stop` and the GUI's restart use, instead of hunting
    /// the pidfile for a process to signal.
    Shutdown,
    /// Turn this connection into an event stream (no further requests on it).
    Subscribe,
}

/// Which of the two pairings — they share a mechanism and share nothing else.
///
/// `Contact*` trades **public** ids between two different people and marks each
/// verified. `Device*` hands this device's **identity secret** to another machine
/// so both become the same person. Conflating them is the one mistake in this
/// area that cannot be undone, so they are separate variants rather than a
/// boolean, and the joining side is separate from the hosting side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairKind {
    /// Show a code; save whoever answers it as a verified contact.
    ContactHost,
    /// Answer someone's code; save them as a verified contact.
    ContactJoin,
    /// Show a code that hands this device's identity to a new one.
    DeviceHost,
    /// Answer a code and *replace* this device's identity with the shared one.
    DeviceJoin,
}

/// A settings edit. `None` means "don't touch this key"; `Some(Clear)` comments it
/// out so the built-in default applies again. Two levels of optionality look
/// redundant until you need to tell "leave the relay alone" from "stop overriding
/// the relay" — a distinction a bare `Option<String>` cannot carry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfigPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<Setting<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_dir: Option<Setting<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<Setting<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<Setting<bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<Setting<bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm: Option<Setting<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<Setting<u32>>,
}

/// One field of a [`ConfigPatch`]: set it to a value, or clear it back to default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Setting<T> {
    Set(T),
    Clear,
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
    /// A background code-serve started: its transfer id + the code to read out.
    CodeServed {
        id: u64,
        code: String,
    },
    /// A public download link (`{relay}/dl/{claim}#{key}`).
    Link(String),
    Transfers(Vec<TransferDto>),
    Pending(Vec<OfferDto>),
    Contacts(Vec<ContactDto>),
    Deposits(Vec<DepositDto>),
    History(Vec<HistoryDto>),
    /// The settings screen's state, after any edit that produced it.
    Config(ConfigDto),
    /// The multi-device summary.
    Sync(SyncDto),
    /// Who is reachable right now, one entry per id asked about.
    Presence(Vec<PresenceDto>),
    /// A pairing session is running; its code and outcome arrive as events.
    PairingStarted {
        session: String,
    },
    /// How many rows a bulk removal actually dropped.
    Cleared(usize),
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
    /// The display name advertised inside offers (empty when none is set).
    /// Optional on the wire for older daemons.
    #[serde(default)]
    pub display_name: String,
}

/// Serializable mirror of [`Transfer`] with a base32 peer id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransferDto {
    pub id: u64,
    /// The 8-hex handle a person types (`arvolo pause/resume/cancel <handle>`).
    /// Derived from what the engine persists (`created`, direction, name, size),
    /// so — unlike `id`, a per-process counter — it survives a daemon restart.
    /// `#[serde(default)]` so an older peer's DTO still decodes (empty = absent).
    #[serde(default)]
    pub handle: String,
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
    /// The live pairing code this send is reachable by, while one is being hosted.
    /// Cleared once the code retires — the send itself carries on.
    #[serde(default)]
    pub code: Option<String>,
    /// This row is a file being **made available**, not a transfer under way.
    ///
    /// Three things land here and they are all the same thing: an `arvolo ticket`
    /// or `code` served in the background, the same serve restored after a daemon
    /// restart, and the seeding a completed download turns into. None of them ends
    /// on its own — they exist until withdrawn, because the whole point is that
    /// someone can still fetch the file.
    ///
    /// A UI that shows them as ordinary transfers gets two things wrong at once: a
    /// served ticket sits at 100% for ever, which reads as stuck, and a seed row
    /// appears at 0% as an outgoing send of a file the user never sent. Neither is
    /// progress towards anything, so neither should be drawn as progress.
    ///
    /// Derived, not stored: a send with no recipient is exactly a send that exists
    /// to be fetched (`send --to` and a live `push` both carry their peer). It says
    /// nothing about *now* — pair it with `download_peers` to tell "available" from
    /// "someone is pulling it right this moment".
    #[serde(default)]
    pub sharing: bool,
    /// Receivers that fetched **every** chunk of this share. Exact — it counts
    /// completed pickups, not people: one person fetching twice counts twice, and
    /// an anonymous ticket carries no identity that could tell them apart.
    #[serde(default)]
    pub copies_served: u64,
    /// Bytes uploaded for this share, across every receiver and across daemon
    /// restarts. An estimate to within a chunk — it is what this share costs in
    /// bandwidth, not an audit.
    #[serde(default)]
    pub bytes_served: u64,
    /// Unix seconds of the last completed pickup; 0 = nobody has finished one.
    #[serde(default)]
    pub last_pickup: u64,
    /// Unix seconds of the download that started this share, when it exists only
    /// because one finished (seed-after-complete); 0 when the user asked for the
    /// share themselves. A row nobody created has to be able to say why it is here.
    #[serde(default)]
    pub from_download: u64,
    /// Where a completed receive was saved, so a UI can offer to open it — or the
    /// folder holding it — from any row, not only from one it watched finish.
    /// `None` for a send, an unfinished receive, or a daemon that predates it.
    #[serde(default)]
    pub path: Option<String>,
    /// How far the inbox offer for a **deposited** send has got: `"pending"`,
    /// `"arrived"`, `"taken"`, `"gone"`. `None` when there is nothing to say —
    /// any other kind of row, or a relay not yet asked.
    ///
    /// The same vocabulary the deposits list uses, on purpose: a deposited send
    /// appears in both places, and two ways of saying the same thing would be two
    /// things to keep in step.
    #[serde(default)]
    pub offer_status: Option<String>,
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
    /// Whether their files auto-download without a prompt (`contacts trust`).
    #[serde(default)]
    pub trusted: bool,
    /// Whether their offers are dropped on arrival (`contacts block`).
    #[serde(default)]
    pub blocked: bool,
    /// The advertised display name the user already approved (empty when none).
    #[serde(default)]
    pub display_name: String,
    /// An advertised name awaiting the user's approval (empty when none) —
    /// surfaced so a UI can offer [`Request::AcceptName`].
    #[serde(default)]
    pub pending_name: String,
}

/// One finished transfer from the history log — read-only by construction (the
/// live, still-actionable rows are [`TransferDto`]s).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryDto {
    pub id: String,
    /// "send" or "recv".
    pub direction: String,
    /// The peer's base32 id (recipient of a send, sender of a receive), if known.
    pub peer: Option<String>,
    pub name: String,
    pub total_size: u64,
    pub transferred: u64,
    /// "completed" / "cancelled" / "failed: <msg>" / "deposited".
    pub status: String,
    /// Unix seconds when the record was written.
    pub created: u64,
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
    /// The sender's `arvm…` ticket, for a sealed deposit — what you paste to
    /// someone so they can fetch it by hand, instead of waiting for their inbox.
    /// Empty for a link (its URL is above) and for a deposit made before tickets
    /// were kept, whose ticket is gone for good.
    ///
    /// Unlike the revoke token, this is not a capability over the *sender's* side
    /// of the deposit, and its payload key is sealed to the recipient — so it can
    /// cross this boundary, and has to, or the sender can never hand it over twice.
    #[serde(default)]
    pub ticket: String,
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
    /// How far the inbox offer pointing at this deposit has got: `"pending"` (no
    /// client of theirs has read it), `"arrived"` (it reached one of their devices
    /// — nobody has necessarily looked at it), `"taken"` (they fetched the file and
    /// acked), or `"gone"` (retracted, or lapsed unread).
    ///
    /// No state says a person saw it: the relay sees reads of a slot, not eyes on
    /// a screen, so `"arrived"` is the most the middle one can claim.
    ///
    /// `None` when the question doesn't apply or couldn't be answered: a public
    /// link has no offer, nor does a deposit whose offer post failed, and a relay
    /// that can't be reached — or one older than the `taken` state — leaves it
    /// unset. A UI must treat it the way it treats `present: None`: say nothing
    /// rather than invent a state. In particular, absence is never "not taken".
    #[serde(default)]
    pub offer_status: Option<String>,
}

/// The settings screen: what is in force, and what `config.toml` actually says.
///
/// The two are not the same and a UI must be able to show both. `relay` is what
/// the next send will use — which may come from `ARVOLO_RELAY`, from the file, or
/// from the value compiled into the binary — while `relay_configured` is the file's
/// own key, the only one an edit can change. Showing only the first makes a
/// built-in default look like a saved setting; showing only the second leaves the
/// field blank on a machine that is demonstrably reaching a relay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigDto {
    /// The relay in force, normalized to a full URL. `None` only when there is no
    /// built-in default either.
    pub relay: Option<String>,
    /// `config.toml`'s own `relay` key, empty when unset.
    #[serde(default)]
    pub relay_configured: String,
    /// Where `relay` came from: "env" | "config" | "builtin" | "none". A UI shows
    /// this so an unchangeable value isn't presented as an editable one.
    #[serde(default)]
    pub relay_source: String,
    /// The folder accepted downloads land in, resolved.
    #[serde(default)]
    pub download_dir: String,
    /// `config.toml`'s own `download_dir` key, empty when unset.
    #[serde(default)]
    pub download_dir_configured: String,
    /// Whether `download_dir` is being forced by `ARVOLO_DOWNLOAD_DIR`, in which
    /// case editing the file would change nothing until that is unset.
    #[serde(default)]
    pub download_dir_from_env: bool,
    /// The display name advertised inside outgoing offers, empty when none.
    #[serde(default)]
    pub display_name: String,
    /// Automatic address-book sync across linked devices.
    #[serde(default)]
    pub sync: bool,
    /// Keep seeding completed files into the swarm. `None` = daemon default.
    #[serde(default)]
    pub seed: Option<bool>,
    /// Swarm mode: "on" | "off" | "relay-only". Empty = daemon default.
    #[serde(default)]
    pub swarm: String,
    /// Parallel chunk fetches. `None` = daemon default.
    #[serde(default)]
    pub concurrency: Option<u32>,
    /// Absolute path of `config.toml`, so a UI can offer "reveal in file manager"
    /// for everything it deliberately does not expose as a control.
    #[serde(default)]
    pub config_path: String,
    /// Absolute path of the identity key file.
    #[serde(default)]
    pub identity_path: String,
}

/// Whether one identity is reachable on the relay right now.
///
/// `online` is an `Option` and that is the whole point. A network failure is not
/// the same as being away, and collapsing the two is what makes a dead relay look
/// exactly like everyone having gone home. `None` means the relay could not be
/// asked, and a UI must say "non lo so" rather than "offline".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PresenceDto {
    pub id: String,
    #[serde(default)]
    pub online: Option<bool>,
}

/// Multi-device state: one identity across several machines, and the address book
/// they keep in step through an encrypted cell on the relay inbox.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncDto {
    /// The shared identity's word fingerprint — the same on every linked device,
    /// which is exactly what makes it the thing to compare when checking a link.
    pub fingerprint: String,
    /// The shared identity's base32 public id.
    #[serde(default)]
    pub public_id: String,
    /// How many contacts the book currently holds.
    pub contacts: usize,
    /// Whether the daemon runs sync rounds on its own.
    pub enabled: bool,
    /// Unix seconds of the last successful round, 0 for "not since this daemon
    /// started". Deliberately not persisted: a stamp from a previous run says
    /// nothing about whether sync works *now*.
    #[serde(default)]
    pub last_sync: u64,
    /// How many updates the last round merged from the other devices.
    #[serde(default)]
    pub last_merged: usize,
    /// Why the last round failed, empty when it didn't.
    #[serde(default)]
    pub last_error: String,
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
        /// Whether the daemon is auto-downloading this one because the sender is
        /// trusted, so it needs no decision. A front-end must announce those as an
        /// arrival, not as a request, or it tells the user to choose something that
        /// has already been chosen.
        ///
        /// The engine cannot know: trust lives in the address book, above it. So
        /// `From<&ManagerEvent>` leaves this false and the daemon fills it in when
        /// it fans the event out — see `stream_events`. `serde(default)` keeps a
        /// client talking to an older daemon working, and false is the safe way to
        /// be wrong: the worst case is a prompt for something already downloading.
        #[serde(default)]
        auto: bool,
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
    /// A short pairing code is live for this send (freshly minted, or restored
    /// after a daemon restart).
    CodeReady {
        id: u64,
        code: String,
    },
    /// A receiver used the code and now holds the ticket.
    CodePaired {
        id: u64,
        done: u32,
    },
    /// The code stopped working. The send behind it carries on.
    CodeClosed {
        id: u64,
        reason: String,
    },
    /// The address book changed under the daemon. Carries nothing: re-issue
    /// [`Request::ListContacts`] to pick the new book up.
    ContactsChanged,
    /// Finished rows were dropped from the daemon's list — somebody else's
    /// [`Request::ClearFinished`], typically `arvolo status clear` run while a
    /// window is open. Carries nothing: drop what *you* consider finished, by the
    /// same rule the daemon uses (a deposit awaiting pickup is not finished).
    FinishedCleared,
    /// A waiting offer left the daemon's parked list by *another* client's hand —
    /// accepted or declined over IPC (`arvolo recv <handle>` / `arvolo decline`
    /// while a window is open). Without this the other front-ends keep showing a
    /// row wired to an offer that no longer exists.
    OfferGone { id: String },
    /// A hosted pairing session has its code — this is what the other machine
    /// types. Only ever emitted for the `*Host` kinds.
    PairingCode {
        session: String,
        code: String,
    },
    /// A pairing session finished successfully. `summary` is a ready-to-show
    /// sentence (who was saved, or how many contacts came across), and
    /// `needs_restart` marks the one case a UI must act on: a device join replaced
    /// the identity this daemon is running as, so it has to come back up as the
    /// new one before anything else it says can be believed.
    PairingDone {
        session: String,
        kind: PairKind,
        summary: String,
        #[serde(default)]
        needs_restart: bool,
    },
    /// A pairing session ended without pairing. `cancelled` separates "the user
    /// closed the sheet" from a real failure, so a UI can stay quiet about the
    /// former instead of showing an error nobody needs to read.
    PairingFailed {
        session: String,
        kind: PairKind,
        error: String,
        #[serde(default)]
        cancelled: bool,
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
        TransferStatus::Waiting(r) => format!("waiting: {r}"),
        TransferStatus::Paused(r) => format!("paused: {r}"),
        TransferStatus::Cancelled => "cancelled".into(),
        TransferStatus::Failed(e) => format!("failed: {e}"),
    }
}

/// The visible handle of a transfer: first 4 bytes of a BLAKE3 over the fields
/// the engine persists, hex. Restart-stable where `id` is not — a restored share
/// keeps its `created`/name/size, so it keeps its handle too.
pub fn transfer_handle(t: &Transfer) -> String {
    use arvolo_core::reexport::Hash;
    let seed = format!(
        "{}-{}-{}-{}",
        t.created,
        direction_str(t.direction),
        t.name,
        t.total_size
    );
    data_encoding::HEXLOWER.encode(&Hash::new(seed.as_bytes()).as_bytes()[..4])
}

impl From<&Transfer> for TransferDto {
    fn from(t: &Transfer) -> Self {
        TransferDto {
            id: t.id,
            handle: transfer_handle(t),
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
            code: t.code.clone(),
            // A send with nobody on the other end is a serve: `send --to` and a
            // live push both know their recipient, so the absence of one is the
            // signal, not a gap in it.
            sharing: matches!(t.direction, Direction::Send) && t.peer.is_none(),
            copies_served: t.copies_served,
            bytes_served: t.bytes_served,
            last_pickup: t.last_pickup,
            from_download: t.from_download,
            path: t.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            offer_status: (!t.offer_status.is_empty()).then(|| t.offer_status.clone()),
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
                // Left false on purpose: trust is an address-book fact the engine
                // has no access to. The daemon overwrites it on the way out.
                auto: false,
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
            // `info` is dropped on purpose, and this is the line that keeps the
            // promise made on `DepositDto`: it carries the deposit's revoke token,
            // a sender-only secret, and every subscriber on the socket would get a
            // copy. A UI only ever needs the id — it asks the daemon to withdraw.
            ManagerEvent::Deposited { id, .. } => EventDto::Deposited { id: *id },
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
            ManagerEvent::CodeReady { id, code } => EventDto::CodeReady {
                id: *id,
                code: code.clone(),
            },
            ManagerEvent::CodePaired { id, done } => EventDto::CodePaired {
                id: *id,
                done: *done,
            },
            ManagerEvent::CodeClosed { id, reason } => EventDto::CodeClosed {
                id: *id,
                reason: reason.clone(),
            },
            ManagerEvent::ContactsChanged => EventDto::ContactsChanged,
            ManagerEvent::FinishedCleared => EventDto::FinishedCleared,
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
                deposit: false,
                ttl: None,
                max: None,
                password: None,
            },
        };
        let line = serde_json::to_string(&env).unwrap();
        let back: RequestEnvelope = serde_json::from_str(&line).unwrap();
        assert_eq!(back.id, 7);
        assert_eq!(back.cmd, env.cmd);
    }

    /// The four mailbox options were added to `Push` after the fact. They carry
    /// `#[serde(default)]` precisely so that the old shape — the one every
    /// already-installed client still sends — keeps decoding. Without this, an
    /// upgrade would break every request from a client older than the daemon,
    /// which is the exact window an upgrade leaves behind.
    #[test]
    fn push_without_the_mailbox_options_still_decodes() {
        let old = r#"{"push":{"to":"alice","paths":["a.txt"],"note":"hi"}}"#;
        let back: Request = serde_json::from_str(old).unwrap();
        assert_eq!(
            back,
            Request::Push {
                to: "alice".into(),
                paths: vec!["a.txt".into()],
                note: "hi".into(),
                deposit: false,
                ttl: None,
                max: None,
                password: None,
            }
        );
    }

    /// A `Setting` is externally tagged, so clearing a key is the bare string
    /// `"clear"` and setting one is `{"set": …}`. The GUI writes this shape by
    /// hand in TypeScript; a change here silently turns every settings edit into
    /// a rejected request.
    #[test]
    fn config_patch_wire_shape_is_stable() {
        let patch = ConfigPatch {
            relay: Some(Setting::Set("relay.example".into())),
            display_name: Some(Setting::Clear),
            ..ConfigPatch::default()
        };
        assert_eq!(
            serde_json::to_string(&patch).unwrap(),
            r#"{"relay":{"set":"relay.example"},"display_name":"clear"}"#,
            "wire shape changed — update gui/src/types.ts to match"
        );
        // An absent key means "leave it alone" and must not appear at all.
        let empty = serde_json::to_string(&ConfigPatch::default()).unwrap();
        assert_eq!(empty, "{}");
    }

    /// The pairing events reach the GUI through the same externally tagged
    /// stream as everything else (`gui/src/events.ts` mirrors it by hand).
    #[test]
    fn pairing_events_keep_their_wire_shape() {
        assert_eq!(
            serde_json::to_string(&EventDto::PairingCode {
                session: "pair-1".into(),
                code: "4821-crater-mango".into(),
            })
            .unwrap(),
            r#"{"pairing_code":{"session":"pair-1","code":"4821-crater-mango"}}"#
        );
        assert_eq!(
            serde_json::to_string(&EventDto::PairingDone {
                session: "pair-1".into(),
                kind: PairKind::DeviceJoin,
                summary: "ok".into(),
                needs_restart: true,
            })
            .unwrap(),
            r#"{"pairing_done":{"session":"pair-1","kind":"device_join","summary":"ok","needs_restart":true}}"#
        );
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

    /// `ManagerEvent::Deposited` carries a [`DepositInfo`] holding the deposit's
    /// **revoke token** — the sender-only capability to delete the blob from the
    /// relay. Every client subscribed to the daemon socket receives every event, so
    /// the conversion to the wire must drop it. It has no reason to travel: a UI
    /// asks the daemon to withdraw by id (`Request::RevokeDeposit`), and the daemon
    /// reads the token from its own store.
    ///
    /// Widening `EventDto::Deposited` to carry `info` would look like a convenience
    /// and would be a capability leak. This test is the tripwire.
    #[test]
    fn deposited_event_never_carries_the_revoke_token_onto_the_wire() {
        let ev = ManagerEvent::Deposited {
            id: 9,
            info: arvolo_core::manager::DepositInfo {
                relay: "https://relay.example".into(),
                claim: "claim-abc".into(),
                revoke_token: "SUPER-SECRET-REVOKE-TOKEN".into(),
                name: "budget.xlsx".into(),
                size: 4242,
                expires: 1_800_000_000,
                max: 1,
                recipient: None,
                offer_id: "offer-1".into(),
                poster_token: "SUPER-SECRET-POSTER-TOKEN".into(),
                ticket: "arvmHANDOVERTICKET".into(),
            },
        };
        let line = serde_json::to_string(&EventDto::from(&ev)).unwrap();
        assert_eq!(line, r#"{"deposited":{"id":9}}"#);
        assert!(!line.contains("SUPER-SECRET-REVOKE-TOKEN"));
        // The other capability in there: it retracts the recipient's inbox entry.
        assert!(!line.contains("SUPER-SECRET-POSTER-TOKEN"));
        assert!(!line.contains("claim-abc"));
        // The hand-over ticket is not a secret from the recipient's side, but the
        // event is still just an id: a front-end keeps the ticket from its own
        // receipt, not by listening to somebody else's deposit going by.
        assert!(!line.contains("arvmHANDOVERTICKET"));
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
            (EventDto::FinishedCleared, r#""finished_cleared""#),
        ];
        for (ev, want) in cases {
            assert_eq!(
                serde_json::to_string(&ev).unwrap(),
                want,
                "wire shape changed — update gui/src/events.ts to match"
            );
        }
    }

    /// The live fields are `Option`s on purpose: "the relay could not be asked" is a
    /// third state, distinct from present and from gone, and the GUI renders it as
    /// "unknown". If they ever silently became plain values, an unreachable relay
    /// would start reporting `false`/`0` — a downloaded link shown as untouched.
    #[test]
    fn deposits_response_roundtrips_and_keeps_unknown_unknown() {
        let r = Response::Deposits(vec![DepositDto {
            id: "a1b2c3d4".into(),
            kind: "link".into(),
            name: "photo.jpg".into(),
            size: 4242,
            link: "https://relay.example/dl/claim#key".into(),
            ticket: String::new(),
            recipient: String::new(),
            created: 1_700_000_000,
            expires: 1_700_604_800,
            expired: false,
            max_label: "unlimited".into(),
            present: None,
            downloads: None,
            max_downloads: None,
            offer_status: None,
        }]);
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);

        // An older daemon predates the live fields entirely: they must decode as
        // "unknown" rather than fail the whole list.
        let old = r#"{"deposits":[{"id":"a1","kind":"link","name":"x","size":1,
            "created":1,"expires":2,"expired":false}]}"#;
        let back: Response = serde_json::from_str(old).unwrap();
        let Response::Deposits(v) = back else {
            panic!("expected deposits")
        };
        assert_eq!(v[0].present, None);
        assert_eq!(v[0].downloads, None);
        // And the hand-over ticket: an older daemon never sent one, which reads as
        // "there is none to give", not as an empty string someone could paste.
        assert!(v[0].ticket.is_empty());
        // Including the offer's own state: a daemon that predates it must not be
        // read as "the recipient hasn't taken it", which is a claim nobody made.
        assert_eq!(v[0].offer_status, None);
    }

    #[test]
    fn contacts_response_roundtrips() {
        let r = Response::Contacts(vec![ContactDto {
            name: "alice".into(),
            id: "if2xmne".into(),
            fingerprint: "able-otter-nine".into(),
            verified: true,
            trusted: true,
            blocked: false,
            display_name: "Alice A.".into(),
            pending_name: String::new(),
        }]);
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);

        // An older daemon predates the trust/name fields: they must default, not
        // fail the whole list.
        let old = r#"{"contacts":[{"name":"bob","id":"abc"}]}"#;
        let Response::Contacts(v) = serde_json::from_str::<Response>(old).unwrap() else {
            panic!("expected contacts")
        };
        assert!(!v[0].trusted && !v[0].blocked && v[0].pending_name.is_empty());
    }

    #[test]
    fn history_response_roundtrips() {
        let r = Response::History(vec![HistoryDto {
            id: "a1b2c3".into(),
            direction: "recv".into(),
            peer: Some("if2xmne".into()),
            name: "photo.jpg".into(),
            total_size: 4242,
            transferred: 4242,
            status: "completed".into(),
            created: 1_700_000_000,
        }]);
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), r);
    }
}
