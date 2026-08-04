//! Client side of the daemon IPC: a thin typed wrapper over the newline-delimited
//! JSON socket. `connect()` fails cleanly when no daemon is listening, so callers
//! can fall back to running the engine in-process (CLI) or spawning the daemon (GUI).

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

use crate::protocol::{
    ContactDto, DepositDto, EventDto, HistoryDto, OfferDto, Request, RequestEnvelope, Response,
    ServerMessage, StatusDto, TransferDto,
};
use crate::socket_path;

/// An RPC connection to the running daemon.
pub struct DaemonClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u32,
}

impl DaemonClient {
    /// Connect to the daemon control socket. Returns `Err` if it's absent or
    /// refuses (no daemon running) — the signal for callers to fall back.
    pub async fn connect() -> Result<Self> {
        let path = socket_path();
        let stream = UnixStream::connect(&path)
            .await
            .with_context(|| format!("no daemon at {} (start `arvolo daemon`)", path.display()))?;
        let (read, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read),
            writer,
            next_id: 1,
        })
    }

    async fn request(&mut self, cmd: Request) -> Result<Response> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let mut line = serde_json::to_string(&RequestEnvelope { id, cmd })?;
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await?;
        loop {
            let mut resp = String::new();
            let n = self.reader.read_line(&mut resp).await?;
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

    pub async fn push(&mut self, to: String, paths: Vec<String>, note: String) -> Result<u64> {
        match self.request(Request::Push { to, paths, note }).await? {
            Response::TransferId(id) => Ok(id),
            other => unexpected(other),
        }
    }

    /// Serve an anonymous P2P ticket in the daemon; returns (transfer id, `arvc…`).
    pub async fn serve_ticket(
        &mut self,
        paths: Vec<String>,
        seed_relay: Option<String>,
    ) -> Result<(u64, String)> {
        match self
            .request(Request::ServeTicket { paths, seed_relay })
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
    ) -> Result<(u64, String)> {
        match self
            .request(Request::ServeCode { paths, relay, keep })
            .await?
        {
            Response::CodeServed { id, code } => Ok((id, code)),
            other => unexpected(other),
        }
    }

    /// Deposit a public browser download link for `path` (on the daemon's
    /// filesystem); returns the shareable URL.
    pub async fn create_link(
        &mut self,
        path: String,
        ttl: Option<u64>,
        max: Option<u32>,
    ) -> Result<String> {
        match self.request(Request::CreateLink { path, ttl, max }).await? {
            Response::Link(url) => Ok(url),
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

    pub async fn accept(&mut self, offer_id: String, out: Option<PathBuf>) -> Result<u64> {
        let out = out.map(|p| p.to_string_lossy().into_owned());
        match self.request(Request::AcceptOffer { offer_id, out }).await? {
            Response::TransferId(id) => Ok(id),
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
    pub async fn add_contact(&mut self, name: String, id: String) -> Result<()> {
        expect_ok(self.request(Request::AddContact { name, id }).await?)
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

    /// Turn this connection into an event stream. Sends `Subscribe`, consumes the
    /// `Ok`, then yields pushed events until the daemon closes.
    pub async fn subscribe(mut self) -> Result<EventStream> {
        expect_ok(self.request(Request::Subscribe).await?)?;
        Ok(EventStream {
            reader: self.reader,
        })
    }
}

/// A live stream of engine events from a subscribed connection.
pub struct EventStream {
    reader: BufReader<OwnedReadHalf>,
}

impl EventStream {
    /// The next event, or `None` when the daemon closes the connection.
    pub async fn next(&mut self) -> Result<Option<EventDto>> {
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None);
            }
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ServerMessage>(line.trim())? {
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
