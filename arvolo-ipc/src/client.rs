//! Client side of the daemon IPC: a thin typed wrapper over the newline-delimited
//! JSON socket. `connect()` fails cleanly when no daemon is listening, so callers
//! can fall back to running the engine in-process (CLI) or spawning the daemon (GUI).

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use arvolo_core::flow::TooLargeForRelay;
use arvolo_core::manager::AcceptRefusal;

use crate::protocol::{
    ConfigDto, ConfigPatch, ContactDto, DepositDto, EventDto, HistoryDto, OfferDto, PairKind,
    PresenceDto, RelayInfoDto, Request, RequestEnvelope, Response, ServerMessage, StatusDto,
    SyncDto, TransferDto,
};

/// The read half of a connection, whatever carried it.
///
/// Boxed rather than generic, and that is a deliberate trade: the transport
/// differs by platform — a unix socket one side, a named pipe the other — and
/// every method below is pure protocol that does not care which. A generic
/// parameter would spread that difference through every signature and every
/// caller; a `cfg` on each method would fork the protocol itself. One virtual
/// call per line of JSON is not a cost anyone will measure on a local socket.
type Read = BufReader<Box<dyn AsyncRead + Unpin + Send>>;
type Write = Box<dyn AsyncWrite + Unpin + Send>;

/// How long one request may wait for its reply before the caller is told the
/// daemon is not answering.
///
/// Every handler on the other end returns promptly *by design*: the slow work —
/// uploads, relay round trips, pairing — runs detached and reports through
/// events. So a reply that never comes means the daemon is wedged, not busy, and
/// a caller standing in front of a person has to be able to say so. Without this
/// there was no way out at all: a window that asked to cancel a transfer and
/// never heard back kept the row on "cancelling…" until the app was restarted,
/// with nothing on screen admitting that nobody had answered.
///
/// Long enough for the slowest legitimate answer (the deposits listing asks every
/// relay it knows, in turn), short enough to be a bounded wait.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The answer to a request that a daemon may carry out in the background.
///
/// Uploading a whole file can take longer than any client should wait on one
/// answer, so a current daemon starts it as a transfer and answers with its id.
/// An older one does the work inside the request and answers with the result.
/// Both are correct answers to the same request; the type makes the caller say
/// what it does with each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Background<T> {
    /// Under way as this transfer; the result arrives as an event.
    Running(u64),
    /// Already done (an older daemon), with the result.
    Done(T),
}

/// Something that answers requests with no socket in between.
///
/// The desktop has a daemon at the other end of a pipe. A phone has neither:
/// the engine runs inside the app, and "asking the daemon" is a function call.
/// Both are the same protocol and the same typed methods below, so the
/// difference stops here rather than forking every caller.
///
/// The trait lives on this side of the line because the dependency only runs one
/// way — the engine knows the protocol crate, and the protocol crate must not
/// know the engine. It is implemented over there.
///
/// Hand-rolled boxed futures rather than an `async fn` in a trait: the returned
/// future has to be nameable and `Send` to cross a spawn, and that is not what
/// bare `async fn` in a trait gives you yet, on the version this workspace pins.
pub trait Answer: Send + Sync {
    /// Answer one request.
    fn answer(&self, cmd: Request) -> BoxFuture<'_, Response>;
    /// Listen to everything the same engine has to say.
    fn events(&self) -> Box<dyn Events>;
}

/// An event subscription that is not a socket. See [`Answer::events`].
pub trait Events: Send {
    /// The next event, or `None` when the engine stops.
    fn next(&mut self) -> BoxFuture<'_, Option<EventDto>>;
}

/// A future that can be boxed, named and sent — see [`Answer`].
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// What carries a request to whoever answers it.
enum Wire {
    /// A local socket, or a named pipe on Windows: the daemon is another process.
    Socket {
        reader: Read,
        writer: Write,
        next_id: u32,
    },
    /// The engine, in this process.
    Embedded(std::sync::Arc<dyn Answer>),
}

/// An RPC connection to the daemon — the one across a socket, or the one running
/// inside this process.
pub struct DaemonClient {
    wire: Wire,
}

/// Open the control channel, however this platform carries one.
///
/// The two arms differ in one respect beyond the type: on unix the daemon's
/// absence is a missing *file*, on Windows a missing *pipe*. Both are the same
/// answer to the caller — "no daemon" — and both are the signal to fall back to
/// running the engine in-process or spawning one.
async fn open() -> Result<(Read, Write)> {
    #[cfg(unix)]
    {
        let path = crate::socket_path();
        let stream = tokio::net::UnixStream::connect(&path)
            .await
            .with_context(|| format!("no daemon at {} (start `arvolo daemon`)", path.display()))?;
        let (r, w) = tokio::io::split(stream);
        Ok((BufReader::new(Box::new(r) as _), Box::new(w) as _))
    }
    #[cfg(windows)]
    {
        let name = crate::pipe_name();
        let stream = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&name)
            .with_context(|| format!("no daemon at {name} (start `arvolo daemon`)"))?;
        let (r, w) = tokio::io::split(stream);
        Ok((BufReader::new(Box::new(r) as _), Box::new(w) as _))
    }
}

/// Where a request's source descriptors are being served — the client-side pair
/// for [`crate::fdpass`] (`offer_files` yields both). Plain strings so callers
/// on every platform compile; only unix daemons ever collect.
#[derive(Clone, Debug)]
pub struct FdHandoff {
    pub socket: String,
    pub token: String,
}

fn split(fds: Option<FdHandoff>) -> (Option<String>, Option<String>) {
    match fds {
        Some(f) => (Some(f.socket), Some(f.token)),
        None => (None, None),
    }
}

impl DaemonClient {
    /// Connect to the daemon control socket. Returns `Err` if it's absent or
    /// refuses (no daemon running) — the signal for callers to fall back.
    pub async fn connect() -> Result<Self> {
        let (reader, writer) = open().await?;
        Ok(Self {
            wire: Wire::Socket {
                reader,
                writer,
                next_id: 1,
            },
        })
    }

    /// A client over an engine running in this process. Never fails: there is no
    /// connection to refuse, which is the whole point on a platform that cannot
    /// keep a daemon alive.
    pub fn embedded(engine: std::sync::Arc<dyn Answer>) -> Self {
        Self {
            wire: Wire::Embedded(engine),
        }
    }

    /// Whether this client is talking to another process.
    pub fn is_remote(&self) -> bool {
        matches!(self.wire, Wire::Socket { .. })
    }

    async fn request(&mut self, cmd: Request) -> Result<Response> {
        let (reader, writer, next_id) = match &mut self.wire {
            // No framing, no timeout, no correlation id: the answer is the return
            // value. The timeout below guards against a daemon that has stopped
            // reading its socket, and an engine in this process either answers or
            // is deadlocked with the caller.
            Wire::Embedded(engine) => return Ok(engine.answer(cmd).await),
            Wire::Socket {
                reader,
                writer,
                next_id,
            } => (reader, writer, next_id),
        };
        let id = *next_id;
        *next_id = next_id.wrapping_add(1);
        let mut line = serde_json::to_string(&RequestEnvelope { id, cmd })?;
        line.push('\n');
        let exchange = async {
            // The write is inside the deadline too: a daemon that has stopped
            // reading fills the socket buffer, and then it is the *send* that
            // never returns.
            writer.write_all(line.as_bytes()).await?;
            writer.flush().await?;
            loop {
                let mut resp = String::new();
                let n = reader.read_line(&mut resp).await?;
                if n == 0 {
                    bail!("daemon closed the connection");
                }
                if resp.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<ServerMessage>(resp.trim())? {
                    ServerMessage::Reply { id: rid, result } if rid == id => return Ok(result),
                    // An RPC connection never subscribes, so events/other replies
                    // shouldn't appear; skip defensively rather than error.
                    _ => continue,
                }
            }
        };
        // On timeout this connection is left mid-exchange (a reply may still be
        // in flight for a request nobody is waiting on any more), so it must not
        // be reused: every caller opens one per command, and the error is the
        // signal to drop this one.
        match tokio::time::timeout(REQUEST_TIMEOUT, exchange).await {
            Ok(result) => result,
            Err(_) => bail!(
                "the daemon accepted the connection but did not answer within {}s — \
                 it may be stuck. Check `arvolo daemon status`, and restart it with \
                 `arvolo daemon stop && arvolo daemon start` if it stays silent.",
                REQUEST_TIMEOUT.as_secs()
            ),
        }
    }

    pub async fn ping(&mut self) -> Result<()> {
        match self.request(Request::Ping).await? {
            Response::Pong => Ok(()),
            other => unexpected(other),
        }
    }

    pub async fn status(&mut self) -> Result<StatusDto> {
        match self.request(Request::Status).await? {
            Response::Status(s) => Ok(s),
            other => unexpected(other),
        }
    }

    /// Ask the daemon to exit cleanly. The daemon answers before it goes; an
    /// older daemon that predates the request answers with an error instead,
    /// which the caller treats as "fall back to the pidfile".
    pub async fn shutdown(&mut self) -> Result<()> {
        match self.request(Request::Shutdown).await? {
            Response::Ok => Ok(()),
            other => unexpected(other),
        }
    }

    pub async fn list(&mut self) -> Result<Vec<TransferDto>> {
        match self.request(Request::ListTransfers).await? {
            Response::Transfers(v) => Ok(v),
            other => unexpected(other),
        }
    }

    pub async fn list_pending(&mut self) -> Result<Vec<OfferDto>> {
        match self.request(Request::ListPending).await? {
            Response::Pending(v) => Ok(v),
            other => unexpected(other),
        }
    }

    pub async fn list_contacts(&mut self) -> Result<Vec<ContactDto>> {
        match self.request(Request::ListContacts).await? {
            Response::Contacts(v) => Ok(v),
            other => unexpected(other),
        }
    }

    /// A live-first send. The mailbox options ride along because the *daemon*
    /// re-probes presence: a recipient who drops offline between the caller's
    /// probe and the daemon's would otherwise be deposited to with the defaults,
    /// silently ignoring the ttl/max/password the user asked for.
    #[allow(clippy::too_many_arguments)]
    pub async fn push(
        &mut self,
        to: String,
        paths: Vec<String>,
        note: String,
        ttl: Option<u64>,
        max: Option<u32>,
        password: Option<String>,
        device: Option<String>,
        // Never fall back to the mailbox: wait for them, or stay waiting.
        live_only: bool,
        fds: Option<FdHandoff>,
    ) -> Result<u64> {
        let (fd_socket, fd_token) = split(fds);
        match self
            .request(Request::Push {
                to,
                paths,
                note,
                deposit: false,
                detach: false,
                live_only,
                ttl,
                max,
                password,
                device,
                fd_socket,
                fd_token,
            })
            .await?
        {
            Response::TransferId(id) => Ok(id),
            other => unexpected(other),
        }
    }

    /// Deposit straight to the recipient's mailbox — `arvolo send --deposit`.
    ///
    /// A current daemon answers at once with the transfer id,
    /// [`Background::Running`]; the `arvm…` ticket for hand-delivery is filed
    /// with the deposit's receipt when the upload lands. A daemon from before
    /// that answers at the end with both, [`Background::Done`].
    #[allow(clippy::too_many_arguments)]
    pub async fn deposit(
        &mut self,
        to: String,
        paths: Vec<String>,
        note: String,
        ttl: Option<u64>,
        max: Option<u32>,
        password: Option<String>,
        device: Option<String>,
        fds: Option<FdHandoff>,
    ) -> Result<Background<(u64, String)>> {
        let (fd_socket, fd_token) = split(fds);
        match self
            .request(Request::Push {
                to,
                paths,
                note,
                deposit: true,
                detach: true,
                live_only: false,
                ttl,
                max,
                password,
                device,
                fd_socket,
                fd_token,
            })
            .await?
        {
            Response::TransferId(id) => Ok(Background::Running(id)),
            Response::Served { id, ticket } => Ok(Background::Done((id, ticket))),
            other => unexpected(other),
        }
    }

    /// The machines sharing this identity, as they describe themselves.
    pub async fn list_devices(&mut self) -> Result<Vec<crate::protocol::DeviceDto>> {
        match self.request(Request::ListDevices).await? {
            Response::Devices(d) => Ok(d),
            other => unexpected(other),
        }
    }

    /// Rename the machine this daemon runs on. Only that one — see the request.
    pub async fn rename_this_device(&mut self, name: String) -> Result<()> {
        match self.request(Request::RenameThisDevice { name }).await? {
            Response::Ok => Ok(()),
            other => unexpected(other),
        }
    }

    /// Retire a machine from the register.
    pub async fn forget_device(&mut self, who: String) -> Result<()> {
        match self.request(Request::ForgetDevice { who }).await? {
            Response::Ok => Ok(()),
            other => unexpected(other),
        }
    }

    /// Serve an anonymous P2P ticket in the daemon; returns (transfer id, `arvc…`).
    pub async fn serve_ticket(
        &mut self,
        paths: Vec<String>,
        seed_relay: Option<String>,
        fds: Option<FdHandoff>,
    ) -> Result<(u64, String)> {
        let (fd_socket, fd_token) = split(fds);
        match self
            .request(Request::ServeTicket {
                paths,
                seed_relay,
                fd_socket,
                fd_token,
            })
            .await?
        {
            Response::Served { id, ticket } => Ok((id, ticket)),
            other => unexpected(other),
        }
    }

    /// Host a short pairing code in the daemon; returns (transfer id, code).
    pub async fn serve_code(
        &mut self,
        paths: Vec<String>,
        relay: Option<String>,
        keep: bool,
        fds: Option<FdHandoff>,
    ) -> Result<(u64, String)> {
        let (fd_socket, fd_token) = split(fds);
        match self
            .request(Request::ServeCode {
                paths,
                relay,
                keep,
                fd_socket,
                fd_token,
            })
            .await?
        {
            Response::CodeServed { id, code } => Ok((id, code)),
            other => unexpected(other),
        }
    }

    /// Start a public browser download link for `path` (on the daemon's
    /// filesystem).
    ///
    /// A current daemon answers at once with the transfer that uploads it —
    /// [`Background::Running`] — and the URL follows as
    /// [`crate::protocol::EventDto::LinkReady`]. A daemon from before that
    /// uploads inside the request and answers with the URL itself,
    /// [`Background::Done`]: which one came back is the daemon's choice, and a
    /// caller handles both.
    pub async fn create_link(
        &mut self,
        path: String,
        ttl: Option<u64>,
        max: Option<u32>,
        fds: Option<FdHandoff>,
    ) -> Result<Background<String>> {
        let (fd_socket, fd_token) = split(fds);
        match self
            .request(Request::CreateLink {
                path,
                ttl,
                max,
                fd_socket,
                fd_token,
                typed_refusal: true,
                detach: true,
            })
            .await?
        {
            Response::TransferId(id) => Ok(Background::Running(id)),
            Response::Link(url) => Ok(Background::Done(url)),
            // The refusal as itself, as on `accept_with_password`: a window reads
            // the code and the limit off it, everyone else prints the sentence.
            // `size` is not on the wire — the sentence already names it — so the
            // typed value a caller gets back carries the limit and a zero size.
            Response::Refused {
                code,
                message,
                limit_bytes,
            } => Err(if code == TooLargeForRelay::CODE {
                anyhow::Error::new(TooLargeForRelay {
                    size: 0,
                    limit: limit_bytes,
                })
                .context(message)
            } else {
                anyhow::anyhow!(message)
            }),
            other => unexpected(other),
        }
    }

    pub async fn cancel(&mut self, id: u64) -> Result<()> {
        expect_ok(self.request(Request::Cancel { id }).await?)
    }

    /// Drop one finished transfer from the daemon's list (per-row "Elimina").
    pub async fn remove(&mut self, id: u64) -> Result<()> {
        expect_ok(self.request(Request::Remove { id }).await?)
    }

    /// Drop every finished transfer from the daemon's list; returns how many went.
    /// In-flight ones stay, and the history log is untouched.
    pub async fn clear_finished(&mut self) -> Result<usize> {
        match self.request(Request::ClearFinished).await? {
            Response::Cleared(n) => Ok(n),
            other => unexpected(other),
        }
    }

    /// Mark a saved contact verified after an out-of-band fingerprint check.
    pub async fn mark_verified(&mut self, name: String) -> Result<()> {
        expect_ok(self.request(Request::MarkVerified { name }).await?)
    }

    /// Everything still withdrawable from a relay: links and sealed deposits.
    pub async fn list_deposits(&mut self) -> Result<Vec<DepositDto>> {
        match self.request(Request::ListDeposits).await? {
            Response::Deposits(v) => Ok(v),
            other => unexpected(other),
        }
    }

    /// Withdraw a deposit from the relay and forget it.
    pub async fn revoke_deposit(&mut self, id: String) -> Result<()> {
        expect_ok(self.request(Request::RevokeDeposit { id }).await?)
    }

    pub async fn pause(&mut self, id: u64) -> Result<()> {
        expect_ok(self.request(Request::Pause { id }).await?)
    }

    pub async fn resume(&mut self, id: u64) -> Result<()> {
        expect_ok(self.request(Request::Resume { id }).await?)
    }

    /// Going to sleep: stop what is in flight so it can be picked up again.
    pub async fn suspend(&mut self) -> Result<()> {
        expect_ok(self.request(Request::Suspend).await?)
    }

    /// Awake: pick up exactly what the suspension stopped.
    pub async fn resume_suspended(&mut self) -> Result<()> {
        expect_ok(self.request(Request::ResumeSuspended).await?)
    }

    /// Park a waiting send on the relay instead of waiting longer for an accept.
    pub async fn deposit_now(&mut self, id: u64) -> Result<()> {
        expect_ok(self.request(Request::DepositNow { id }).await?)
    }

    pub async fn accept(&mut self, offer_id: String, out: Option<PathBuf>) -> Result<u64> {
        self.accept_with_password(offer_id, out, None).await
    }

    /// Accept an offer whose deposit was sealed with a password.
    ///
    /// Such an offer is indistinguishable from any other until the fetch fails,
    /// so a UI sends the plain accept, reads the refusal, asks, and sends it
    /// again with this.
    pub async fn accept_with_password(
        &mut self,
        offer_id: String,
        out: Option<PathBuf>,
        password: Option<String>,
    ) -> Result<u64> {
        let out = out.map(|p| p.to_string_lossy().into_owned());
        match self
            .request(Request::AcceptOffer {
                offer_id,
                out,
                password,
                typed_refusal: true,
            })
            .await?
        {
            Response::TransferId(id) => Ok(id),
            // Hand the refusal back as itself, not as its sentence: a caller that
            // has something to do about one — put the question back in front of the
            // user, show the password field — can then ask what it is instead of
            // reading English. `{e:#}` still prints the same words for everyone
            // else. An unknown code (a daemon newer than this client) keeps the
            // message and loses only the ability to act on it.
            Response::Refused { code, message, .. } => Err(match AcceptRefusal::from_code(&code) {
                Some(r) => anyhow::Error::new(r),
                None => anyhow::anyhow!(message),
            }),
            other => unexpected(other),
        }
    }

    pub async fn reject(&mut self, offer_id: String) -> Result<()> {
        expect_ok(self.request(Request::RejectOffer { offer_id }).await?)
    }

    /// Receive from a pasted `arvc…` ticket, pairing code or `arvm…` offline
    /// ticket — the daemon fetches it as a normal transfer; returns its id.
    pub async fn recv(
        &mut self,
        ticket: String,
        out: Option<PathBuf>,
        password: Option<String>,
    ) -> Result<u64> {
        let out = out.map(|p| p.to_string_lossy().into_owned());
        match self
            .request(Request::Recv {
                ticket,
                out,
                password,
            })
            .await?
        {
            Response::TransferId(id) => Ok(id),
            other => unexpected(other),
        }
    }

    /// Save (or re-key) a contact. Re-keying clears verified/trusted marks —
    /// warn the user first.
    /// Save a contact. Refused if the name already belongs to another key —
    /// see [`Self::rebind_contact`] for the deliberate version of that.
    pub async fn add_contact(&mut self, name: String, id: String) -> Result<()> {
        expect_ok(
            self.request(Request::AddContact {
                name,
                id,
                rebind: false,
            })
            .await?,
        )
    }

    /// Point an existing contact name at a different key, dropping its verified
    /// and trusted marks. Returns the two fingerprints, so the caller can show
    /// what changed. Also succeeds (with `None`) when the name was free.
    pub async fn rebind_contact(
        &mut self,
        name: String,
        id: String,
    ) -> Result<Option<(String, String)>> {
        match self
            .request(Request::AddContact {
                name,
                id,
                rebind: true,
            })
            .await?
        {
            Response::KeyChanged {
                old_fingerprint,
                new_fingerprint,
            } => Ok(Some((old_fingerprint, new_fingerprint))),
            Response::Ok => Ok(None),
            other => unexpected(other),
        }
    }

    pub async fn remove_contact(&mut self, name: String) -> Result<()> {
        expect_ok(self.request(Request::RemoveContact { name }).await?)
    }

    pub async fn rename_contact(&mut self, old: String, new: String) -> Result<()> {
        expect_ok(self.request(Request::RenameContact { old, new }).await?)
    }

    /// Clear a contact's verified mark.
    pub async fn mark_unverified(&mut self, name: String) -> Result<()> {
        expect_ok(self.request(Request::MarkUnverified { name }).await?)
    }

    /// Trust a contact/id to auto-download. Refused for an unverified contact
    /// unless `force`.
    pub async fn mark_trusted(&mut self, who: String, force: bool) -> Result<()> {
        expect_ok(self.request(Request::MarkTrusted { who, force }).await?)
    }

    pub async fn mark_untrusted(&mut self, who: String) -> Result<()> {
        expect_ok(self.request(Request::MarkUntrusted { who }).await?)
    }

    /// Silence an identity: offers from it are dropped on arrival.
    pub async fn block(&mut self, who: String) -> Result<()> {
        expect_ok(self.request(Request::Block { who }).await?)
    }

    pub async fn unblock(&mut self, who: String) -> Result<()> {
        expect_ok(self.request(Request::Unblock { who }).await?)
    }

    /// Approve a contact's pending advertised display name.
    pub async fn accept_name(&mut self, who: String) -> Result<()> {
        expect_ok(self.request(Request::AcceptName { who }).await?)
    }

    /// The log of finished transfers, newest first.
    pub async fn list_history(&mut self) -> Result<Vec<HistoryDto>> {
        match self.request(Request::ListHistory).await? {
            Response::History(v) => Ok(v),
            other => unexpected(other),
        }
    }

    /// Forget the whole history log; returns how many records went.
    pub async fn clear_history(&mut self) -> Result<usize> {
        match self.request(Request::ClearHistory).await? {
            Response::Cleared(n) => Ok(n),
            other => unexpected(other),
        }
    }

    /// Set (or clear, with an empty string) the advertised display name.
    pub async fn set_my_name(&mut self, name: String) -> Result<()> {
        expect_ok(self.request(Request::SetMyName { name }).await?)
    }

    /// The settings screen's state.
    pub async fn get_config(&mut self) -> Result<ConfigDto> {
        match self.request(Request::GetConfig).await? {
            Response::Config(c) => Ok(c),
            other => unexpected(other),
        }
    }

    /// Write settings back, and read the resulting state in the same round trip.
    pub async fn set_config(&mut self, patch: ConfigPatch) -> Result<ConfigDto> {
        match self.request(Request::SetConfig(patch)).await? {
            Response::Config(c) => Ok(c),
            other => unexpected(other),
        }
    }

    /// Ask the relay who is reachable right now. `None` in a row means the relay
    /// could not be asked — never "offline".
    pub async fn presence(&mut self, ids: Vec<String>) -> Result<Vec<PresenceDto>> {
        match self.request(Request::Presence { ids }).await? {
            Response::Presence(v) => Ok(v),
            other => unexpected(other),
        }
    }

    /// What the configured relay is, and what version it reports.
    pub async fn relay_info(&mut self) -> Result<RelayInfoDto> {
        match self.request(Request::RelayInfo).await? {
            Response::RelayInfo(info) => Ok(info),
            other => unexpected(other),
        }
    }

    pub async fn sync_status(&mut self) -> Result<SyncDto> {
        match self.request(Request::SyncStatus).await? {
            Response::Sync(s) => Ok(s),
            other => unexpected(other),
        }
    }

    pub async fn sync_now(&mut self) -> Result<SyncDto> {
        match self.request(Request::SyncNow).await? {
            Response::Sync(s) => Ok(s),
            other => unexpected(other),
        }
    }

    /// Begin a pairing exchange; the code and the outcome arrive as events on a
    /// subscribed connection. Returns the session handle to cancel it by.
    pub async fn start_pairing(
        &mut self,
        kind: PairKind,
        relay: Option<String>,
        code: Option<String>,
        name: Option<String>,
    ) -> Result<String> {
        match self
            .request(Request::StartPairing {
                kind,
                relay,
                code,
                name,
            })
            .await?
        {
            Response::PairingStarted { session } => Ok(session),
            other => unexpected(other),
        }
    }

    pub async fn cancel_pairing(&mut self, session: String) -> Result<()> {
        expect_ok(self.request(Request::CancelPairing { session }).await?)
    }

    /// Turn this connection into an event stream.
    ///
    /// Across a socket that means sending `Subscribe`, consuming the `Ok`, and
    /// then reading pushed events until the daemon closes — the connection is
    /// spent for anything else. In this process it is simply a subscription: the
    /// engine has one to give, and nothing needed sending to ask for it.
    pub async fn subscribe(mut self) -> Result<EventStream> {
        if let Wire::Embedded(engine) = &self.wire {
            return Ok(EventStream {
                source: Source::Embedded(engine.events()),
            });
        }
        expect_ok(self.request(Request::Subscribe).await?)?;
        match self.wire {
            Wire::Socket { reader, .. } => Ok(EventStream {
                source: Source::Socket(reader),
            }),
            Wire::Embedded(_) => unreachable!("handled above"),
        }
    }
}

/// Where a subscribed stream's events come from.
enum Source {
    Socket(Read),
    Embedded(Box<dyn Events>),
}

/// A live stream of engine events from a subscribed connection.
pub struct EventStream {
    source: Source,
}

impl EventStream {
    /// The next event, or `None` when the daemon closes the connection.
    pub async fn next(&mut self) -> Result<Option<EventDto>> {
        let reader = match &mut self.source {
            // Already an `EventDto`, and already stamped: nothing to parse and
            // nothing that can be a variant we have never heard of.
            Source::Embedded(events) => return Ok(events.next().await),
            Source::Socket(reader) => reader,
        };
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None);
            }
            if line.trim().is_empty() {
                continue;
            }
            // A daemon newer than this client can send a variant we have never
            // heard of. Skipping it keeps the stream alive; propagating the
            // parse error tore the subscription down, which the GUI's event pump
            // reports as "daemon non raggiungibile" and then reconnects into,
            // flapping once per unknown event. The reverse direction is already
            // handled in the daemon's `handle_conn`.
            let parsed = match serde_json::from_str::<ServerMessage>(line.trim()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            match parsed {
                ServerMessage::Event(ev) => return Ok(Some(ev)),
                ServerMessage::Reply { .. } => continue,
            }
        }
    }
}

fn expect_ok(r: Response) -> Result<()> {
    match r {
        Response::Ok => Ok(()),
        Response::Error(e) => bail!(e),
        other => unexpected(other),
    }
}

fn unexpected<T>(r: Response) -> Result<T> {
    match r {
        Response::Error(e) => bail!(e),
        other => bail!("unexpected daemon response: {other:?}"),
    }
}
