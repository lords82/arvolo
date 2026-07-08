use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "arvolo",
    version,
    about = "arvolo — secure cross-platform file sending"
)]
pub(crate) struct Cli {
    /// Explain each step as it happens, in arvolo's own words. `-v` narrates the
    /// transfer (relay chosen, ticket, receiver connected, chunk sources) and
    /// silences iroh's low-level networking noise; `-vv` adds finer detail (e.g.
    /// when chunks switch between the sender and the relay), still without iroh.
    /// `-vvv` opens iroh's raw logs for deep network debugging. An explicit
    /// `RUST_LOG` always wins.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub(crate) verbose: u8,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Send files/folders — the tool picks the channel. Without `--to`: a P2P
    /// `arvc…` ticket to share. With `--to <id>`: live to their daemon if online,
    /// else mailbox + an `arvm…` ticket. `--link` makes a public browser link.
    /// Multiple paths or a folder are packed into one archive automatically.
    Send {
        #[arg(num_args = 0..)]
        paths: Vec<PathBuf>,
        /// Resume an interrupted send so the ticket you already shared stays valid.
        /// Pass a **session id** (see `arvolo sessions list`) — recovers `--to`
        /// sends too, no file needed — OR a plain **`arvc…` ticket** together with
        /// its file (re-serves it; the receiver uses the reprinted ticket).
        #[arg(long, value_name = "ID|TICKET")]
        resume: Option<String>,
        /// Show a short pairing code (e.g. 4821-crater-mango) instead of the long
        /// ticket. Needs a relay: --relay, or the ARVOLO_RELAY env var.
        #[arg(long)]
        code: bool,
        /// Rendezvous relay for --code. When given, it is embedded in the code so
        /// the receiver needs no configuration. Host or URL, e.g.
        /// relay.example.com (https assumed; pass --use-http for plaintext).
        #[arg(long)]
        relay: Option<String>,
        /// Treat bare relay addresses as `http://` instead of `https://`
        /// (LAN / dev / plaintext relays). Explicit schemes are always kept.
        #[arg(long)]
        use_http: bool,
        /// Send to a known recipient (a saved contact name or public id). The
        /// tool picks the channel: if they're online it's delivered live to their
        /// daemon; if offline it's deposited on the relay (mailbox) + an `arvm…`
        /// ticket is printed so you can also hand it over. Without `--to` you get
        /// a P2P `arvc…` ticket to share.
        #[arg(long)]
        to: Option<String>,
        /// With `--to`: force the mailbox/`arvm…` path even if the recipient is
        /// online (send-and-forget with a shareable code).
        #[arg(long)]
        ticket: bool,
        /// With `--to`: attach a short note delivered *with* the file — it rides
        /// inside the E2E-sealed offer (the relay never sees it).
        #[arg(long, short = 'm')]
        note: Option<String>,
        /// Produce a public browser **download link** instead (anyone with the
        /// link decrypts it client-side). `--to` is not used.
        #[arg(long)]
        link: bool,
        /// Mailbox/link time-to-live in seconds (default 7 days).
        #[arg(long, default_value_t = 7 * 24 * 3600)]
        ttl: u64,
        /// Mailbox/link max downloads before deletion (default 1 for a sealed
        /// send; unlimited for `--link`).
        #[arg(long)]
        max: Option<u32>,
        /// Password-protect a mailbox/link send (E2E — required to decrypt).
        #[arg(long)]
        password: Option<String>,
        /// Serve the P2P ticket **in this terminal** (blocking, Ctrl-C to stop)
        /// instead of handing it to the daemon. Default: if a daemon is running, a
        /// plain ticket send is served by it in the background (track with `arvolo
        /// transfers`). No effect with `--to`/`--link`/`--code`.
        #[arg(long)]
        foreground: bool,
        /// Also render the ticket/code as a scannable QR code.
        #[arg(long)]
        qr: bool,
    },
    /// Receive from any arvolo ticket or code — the tool picks how automatically:
    /// a P2P ticket (`arvc…`) or pairing code (`N-word-word[@relay]`) fetches live,
    /// an offline/mailbox ticket (`arvm…`) or download link decrypts from the relay.
    Recv {
        ticket: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Password for a password-protected offline ticket / link.
        #[arg(long)]
        password: Option<String>,
    },
    /// Manage your address book of recipients (used by --to).
    Contacts {
        #[command(subcommand)]
        action: ContactAction,
    },
    /// Link another device to share one identity, so your contacts see a single
    /// id and any device can open files sent to you. Also carries your address
    /// book between devices.
    Device {
        #[command(subcommand)]
        action: DeviceAction,
    },
    /// Synchronize your address book across your linked devices (see `device`).
    Sync {
        #[command(subcommand)]
        action: Option<SyncAction>,
    },
    /// List or delete resumable send sessions (used by `send --resume`).
    Sessions {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Show all transfers — live (in/out) and past — plus offers awaiting
    /// approval. With a daemon running it includes its live state; otherwise it
    /// shows the persisted history. `clear` wipes the history.
    Transfers {
        /// Keep the view open and redraw as transfers progress (needs a daemon).
        #[arg(long)]
        watch: bool,
        #[command(subcommand)]
        action: Option<TransferAction>,
    },
    /// Show your public id (creates an identity on first use).
    Id,
    /// Show or set your display name — the self-chosen name advertised to
    /// recipients inside each sealed offer (a petname claim, never a verified
    /// identity). No argument prints the current name; pass a name to set it, or
    /// an empty string to clear it.
    Name {
        /// The display name to set. Omit to show the current one.
        name: Option<String>,
    },
    /// Show the CLI version and whether the daemon is running (and its version).
    Version,
    /// Revoke a mailbox send or a browser link, deleting its blob from the relay.
    Revoke {
        /// The offline ticket (`arvm…`) or download link (`…/dl/<claim>#…`) you
        /// sent. For a link the `#key` part is ignored.
        target: String,
        /// The revoke token printed when you sent it.
        #[arg(long)]
        token: String,
    },
    /// Stay online and receive files pushed to you by contacts: shows each
    /// incoming offer (sender, name, size) and downloads accepted ones
    /// transparently. Needs a relay (--relay / ARVOLO_RELAY / config).
    Listen {
        /// Directory to save accepted downloads into (default: ~/Arvolo).
        #[arg(long)]
        download_dir: Option<PathBuf>,
        /// Relay host or URL (https assumed; pass --use-http for plaintext).
        /// Defaults to ARVOLO_RELAY / config `relay`.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`.
        #[arg(long)]
        use_http: bool,
        /// Auto-accept offers from saved contacts (still prompt for unknown senders).
        #[arg(long)]
        auto_accept_contacts: bool,
        /// Auto-accept offers from verified contacts only (safer than
        /// --auto-accept-contacts). Still prompts for everyone else.
        #[arg(long)]
        auto_accept_verified: bool,
        /// Accept every incoming offer without prompting.
        #[arg(long)]
        yes: bool,
        /// Don't auto-sync your address book across your linked devices.
        #[arg(long)]
        no_sync: bool,
    },
    /// Run the always-on background engine: stays online, receives files, and
    /// exposes a local control socket so `send`/`transfers`/etc. drive one shared
    /// instance. Meant to run under systemd/launchd. Needs a relay.
    #[cfg(unix)]
    Daemon {
        /// Directory to save accepted downloads into
        /// (default: ~/Arvolo).
        #[arg(long)]
        download_dir: Option<PathBuf>,
        /// Relay host or URL (https assumed; pass --use-http for plaintext).
        /// Defaults to ARVOLO_RELAY / config `relay`.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`.
        #[arg(long)]
        use_http: bool,
        /// Don't auto-sync your address book across your linked devices.
        #[arg(long)]
        no_sync: bool,
    },
    /// Accept a parked offer by its id (see `arvolo transfers`) and download it.
    #[cfg(unix)]
    Accept {
        /// The offer id shown by `arvolo transfers`.
        offer_id: String,
        /// Save to this path instead of the daemon's download dir.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Reject a parked offer by its id.
    #[cfg(unix)]
    Reject {
        /// The offer id shown by `arvolo transfers`.
        offer_id: String,
    },
    /// Cancel a running transfer by its id (see `arvolo transfers`).
    #[cfg(unix)]
    Cancel {
        /// The transfer id shown by `arvolo transfers`.
        id: u64,
    },
    /// Pause an in-progress `send --to` (hold it; resume or cancel later).
    #[cfg(unix)]
    Pause {
        /// The transfer id shown by `arvolo transfers`.
        id: u64,
    },
    /// Resume a paused `send --to`.
    #[cfg(unix)]
    Resume {
        /// The transfer id shown by `arvolo transfers`.
        id: u64,
    },
}

#[derive(Subcommand)]
pub(crate) enum ContactAction {
    /// Save (or update) a contact: a name and their public id.
    Add { name: String, id: String },
    /// List saved contacts (with online status if a relay is configured).
    /// Pass a filter to show only matches: a public id (exact or prefix,
    /// case-insensitive) or a substring of the contact name.
    List { filter: Option<String> },
    /// Remove a saved contact.
    Remove { name: String },
    /// Mark a contact verified after comparing its fingerprint out-of-band.
    /// Shows the fingerprint and asks for confirmation before marking; pass
    /// `--yes` to skip the prompt (required in a non-interactive shell).
    Verify {
        name: String,
        /// Mark verified without the confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Remove a contact's verified mark.
    Unverify { name: String },
    /// Trust a contact so the daemon auto-downloads their files without asking.
    /// Refuses an unverified contact unless `--force` is given (auto-downloading
    /// from a key you haven't confirmed out-of-band is a MITM risk).
    Trust {
        name: String,
        /// Trust even if the contact isn't verified yet.
        #[arg(long)]
        force: bool,
    },
    /// Stop auto-downloading from a contact (their files will ask again).
    Untrust { name: String },
    /// Approve a contact's advertised display name (the name they chose for
    /// themselves). Pins the pending name so it's shown from now on. Pass a
    /// contact name or a raw id, or `--all` to approve every pending name.
    AcceptName {
        /// Contact name or base32 id. Omit when using `--all`.
        who: Option<String>,
        /// Approve every pending advertised name at once.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum DeviceAction {
    /// On a device you already use: show a pairing code for a new device to
    /// join. Shares this device's identity + address book once it connects.
    Pair {
        /// Rendezvous relay (host or URL). Embedded in the code so the new device
        /// needs no config. Defaults to ARVOLO_RELAY / config / built-in.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`.
        #[arg(long)]
        use_http: bool,
        /// Also render the pairing code as a scannable QR code.
        #[arg(long)]
        qr: bool,
    },
    /// On the new device: join using the code shown by `device pair`. Overwrites
    /// this device's identity with the shared one and imports the address book.
    Join {
        /// The pairing code from `device pair` (e.g. 4821-crater-mango@relay).
        code: String,
        /// Overwrite an existing identity without the confirmation prompt
        /// (required in a non-interactive shell).
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum SyncAction {
    /// Publish this device's address book and merge any pending updates now.
    Now,
    /// Show sync state (identity fingerprint, contact count, last sync).
    Status,
}

#[derive(Subcommand)]
pub(crate) enum TransferAction {
    /// Delete all transfer history.
    Clear,
}

#[derive(Subcommand)]
pub(crate) enum SessionAction {
    /// List resumable send sessions.
    List,
    /// Delete a saved session by id.
    Rm { id: String },
}
