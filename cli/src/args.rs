use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::engine::{ArgValueCandidates, ArgValueCompleter, PathCompleter};

use crate::completions;

/// How `--help` groups the commands: by the job you came to do, not alphabetically.
///
/// Eighteen verbs in one flat column is a list you read rather than scan. clap 4
/// has no notion of subcommand groups — `subcommand_help_heading` only renames the
/// single "Commands:" block — so the listing is rendered by hand in
/// [`grouped_commands`], and the whole risk of doing that is drift: a verb nobody
/// files here would silently vanish from `--help`. The `every_command_is_grouped`
/// test is the guard, and it checks both directions.
const COMMAND_GROUPS: &[(&str, &[&str])] = &[
    ("Get files to someone", &["send", "link", "code", "ticket"]),
    (
        "Get files from someone",
        &["recv", "listen", "daemon", "accept", "reject"],
    ),
    (
        "Follow it, and take it back",
        &["status", "history", "pause", "resume", "cancel"],
    ),
    ("People and devices", &["contacts", "device", "me"]),
    ("Arvolo itself", &["completions", "help"]),
];

/// Like clap's default, but with `{options}` in place of `{all-args}` so the
/// automatic flat command list is left out, and `{after-help}` — which carries
/// [`grouped_commands`] — put where that list used to be.
///
/// No blank line before `{after-help}`: clap prepends its own `\n\n` to it.
const HELP_TEMPLATE: &str = "\
{about-with-newline}
{usage-heading} {usage}{after-help}{options}";

/// The command tree with the grouped listing installed.
///
/// Used for parsing *and* for completion, so the two can never end up looking at
/// different trees.
pub(crate) fn build_cli() -> clap::Command {
    let mut cmd = Cli::command();
    // Materialises the subcommands clap adds itself — `help` above all, which is
    // in a group like any other and would otherwise be missing at reflection time.
    cmd.build();
    let listing = grouped_commands(&cmd);
    cmd.help_template(HELP_TEMPLATE).after_help(listing)
}

/// Render [`COMMAND_GROUPS`] as clap would have rendered its own listing.
///
/// Styling comes from the command's own [`clap::builder::Styles`], so the group
/// headings match the "Options:" heading clap prints below them. Embedding the
/// escape codes is safe: clap writes help through an `anstream::AutoStream`,
/// which strips them when the output is not a terminal.
fn grouped_commands(cmd: &clap::Command) -> String {
    use std::fmt::Write as _;

    let styles = cmd.get_styles();
    let (header, literal) = (styles.get_header(), styles.get_literal());

    // Five commands are unix-only, so a name that isn't in this build is simply
    // skipped rather than printed as a row pointing at nothing.
    let present = |name: &str| cmd.find_subcommand(name);
    let width = COMMAND_GROUPS
        .iter()
        .flat_map(|(_, names)| names.iter())
        .filter(|n| present(n).is_some())
        .map(|n| n.len())
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    for (heading, names) in COMMAND_GROUPS {
        let rows: Vec<_> = names
            .iter()
            .filter_map(|n| present(n).map(|s| (*n, s)))
            .collect();
        if rows.is_empty() {
            continue;
        }
        let _ = writeln!(
            out,
            "{}{heading}:{}",
            header.render(),
            header.render_reset()
        );
        for (name, sub) in rows {
            let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
            let pad = " ".repeat(width - name.len());
            let _ = writeln!(
                out,
                "  {}{name}{}{pad}  {about}",
                literal.render(),
                literal.render_reset()
            );
        }
        out.push('\n');
    }
    // The "Options:" heading belongs to the `{options}` block that follows, but
    // that placeholder emits only the rows — clap writes the heading itself only
    // for `{all-args}`, which is exactly the piece we replaced. So it is written
    // here, in the same style, to sit directly above them.
    let _ = writeln!(out, "{}Options:{}", header.render(), header.render_reset());
    out
}

#[derive(Parser)]
#[command(
    name = "arvolo",
    version,
    about = "arvolo — secure cross-platform file sending",
    arg_required_else_help = true
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

// Every verb names the thing you're trying to achieve. (A plain comment, not a
// doc comment: clap would hoist the latter into `arvolo --help` as the tool's
// own description.)
//
// Sending splits along one question — *do I know who gets this?* If you do,
// `send` delivers to them. If you don't, you ask for the **artefact** you want to
// hand around yourself, and the verb is its name: `link`, `code`, `ticket`. They
// were once `send --link` / `--code` / `--ticket`, which forced a pile of runtime
// checks to reject the combinations that make no sense (`--link --to`, `--code
// --to`, `--link --code`, `--max` without a mailbox). As separate verbs those
// states can't be spelled, so the checks are gone.
//
// Receiving deliberately stays **one** verb: the sender picks the artefact, and
// the receiver shouldn't have to know which one arrived — paste it and go.
//
// Each variant's doc comment opens with one short line: clap shows that alone in
// the command list and the rest under `arvolo <cmd> --help`, so the top-level
// help stays scannable.
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Send files to a contact.
    ///
    /// If they're online it goes live to their daemon; if they're not, it's
    /// deposited on the relay (mailbox) and an `arvm…` ticket is printed so you
    /// can hand it over yourself. Multiple paths or a folder are packed into one
    /// archive automatically.
    Send {
        /// Who gets it: a saved contact name (see `arvolo contacts`) or a public id.
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        who: String,
        /// The files or folders to deliver.
        #[arg(required = true, num_args = 1.., value_hint = clap::ValueHint::AnyPath)]
        paths: Vec<PathBuf>,
        /// Don't try a live delivery: deposit it on the mailbox even if they're
        /// online (send-and-forget, with an `arvm…` ticket you can also share).
        #[arg(long)]
        deposit: bool,
        /// Attach a short note delivered *with* the file — it rides inside the
        /// E2E-sealed offer, so the relay never sees it.
        #[arg(long, short = 'm')]
        note: Option<String>,
        /// Relay host or URL (https assumed; pass --use-http for plaintext).
        /// Defaults to ARVOLO_RELAY / config `relay`.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`
        /// (LAN / dev / plaintext relays). Explicit schemes are always kept.
        #[arg(long)]
        use_http: bool,
        /// Mailbox time-to-live in seconds (default 7 days).
        #[arg(long, default_value_t = 7 * 24 * 3600, help_heading = "If it gets deposited")]
        ttl: u64,
        /// Max downloads before the relay deletes it (default 1).
        #[arg(long, help_heading = "If it gets deposited")]
        max: Option<u32>,
        /// Password-protect the deposit (E2E — required to decrypt).
        #[arg(long, help_heading = "If it gets deposited")]
        password: Option<String>,
        /// Also render the `arvm…` ticket as a scannable QR code.
        #[arg(long, help_heading = "If it gets deposited")]
        qr: bool,
    },
    /// Get a browser download link.
    ///
    /// Anyone can open it — the person you send it to needs no arvolo and no
    /// account. The file is decrypted client-side; the key rides in the URL
    /// fragment, which browsers never send to the relay.
    Link {
        /// The files or folders to publish.
        #[arg(required = true, num_args = 1.., value_hint = clap::ValueHint::AnyPath)]
        paths: Vec<PathBuf>,
        /// Relay host or URL that hosts the link (https assumed; pass --use-http
        /// for plaintext). Defaults to ARVOLO_RELAY / config `relay`.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`.
        #[arg(long)]
        use_http: bool,
        /// Time-to-live in seconds (default 7 days).
        #[arg(long, default_value_t = 7 * 24 * 3600)]
        ttl: u64,
        /// Max downloads before the relay deletes it (default: no cap).
        #[arg(long)]
        max: Option<u32>,
        /// Password-protect the link (E2E — required to decrypt).
        #[arg(long)]
        password: Option<String>,
        /// Also render the link as a scannable QR code.
        #[arg(long)]
        qr: bool,
    },
    /// Get a short code you can read out loud.
    ///
    /// Something like 4821-crater-mango, short enough to dictate or type by
    /// hand. The file itself still travels P2P: the relay only brokers the
    /// rendezvous, so it needs one — `--relay`, ARVOLO_RELAY, or config.
    ///
    /// If a daemon is running it hosts the code in the background — the command
    /// returns straight away, `arvolo status` shows the code, and it survives
    /// this terminal and the daemon restarting. Track it there, stop it with
    /// `arvolo cancel <id>`.
    Code {
        /// The files or folders to send.
        #[arg(required = true, num_args = 1.., value_hint = clap::ValueHint::AnyPath)]
        paths: Vec<PathBuf>,
        /// Rendezvous relay. When given, it is embedded in the code so the
        /// receiver needs no configuration. Host or URL, e.g. relay.example.com
        /// (https assumed; pass --use-http for plaintext).
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`.
        #[arg(long)]
        use_http: bool,
        /// Keep the code working for every receiver until you `arvolo cancel` it,
        /// instead of retiring it once the first one has the file.
        ///
        /// Convenient for handing one code to a room; also a bigger capability —
        /// a code glimpsed over a shoulder stays usable for as long as it lives.
        #[arg(long)]
        keep: bool,
        /// Serve it **in this terminal** (blocking, Ctrl-C to stop) instead of
        /// handing it to the daemon.
        #[arg(long)]
        foreground: bool,
        /// Also render the code as a scannable QR code.
        #[arg(long)]
        qr: bool,
    },
    /// Get an `arvc…` ticket to paste into a chat.
    ///
    /// Pure P2P: the ticket itself is the capability, so it works with no relay
    /// at all and with no address book on either side. If a daemon is running it
    /// serves the ticket in the background — track it with `arvolo status`.
    Ticket {
        /// The files or folders to serve.
        #[arg(required = true, num_args = 1.., value_hint = clap::ValueHint::AnyPath)]
        paths: Vec<PathBuf>,
        /// Swarm relay embedded in the ticket, so the receiver can backfill from
        /// it if you go offline and peers can seed to each other. Defaults to
        /// ARVOLO_RELAY / config `relay`; the send still works without one.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`.
        #[arg(long)]
        use_http: bool,
        /// Serve it **in this terminal** (blocking, Ctrl-C to stop) instead of
        /// handing it to the daemon.
        #[arg(long)]
        foreground: bool,
        /// Also render the ticket as a scannable QR code.
        #[arg(long)]
        qr: bool,
    },
    /// Receive from any ticket, code or link — or see what's waiting for you.
    ///
    /// One verb for all of them — it works out which it is: a P2P ticket
    /// (`arvc…`) or pairing code (`N-word-word[@relay]`) fetches live, an
    /// offline/mailbox ticket (`arvm…`) or download link decrypts from the relay.
    ///
    /// With nothing to paste it lists instead: the sends addressed to *you*
    /// (`arvolo send <you> …`), still sealed on the relay, and you pick one. A
    /// code, ticket or link never appears in that list — it *is* the permission to
    /// fetch, so nothing on the relay knows which ones are meant for you, which is
    /// also what stops anyone from enumerating yours.
    Recv {
        /// The ticket, pairing code or download link. Leave it out to see what's
        /// waiting for you and take one from the list.
        ticket: Option<String>,
        #[arg(short, long, add = ArgValueCompleter::new(PathCompleter::any()))]
        out: Option<PathBuf>,
        /// Password for a password-protected offline ticket / link.
        #[arg(long)]
        password: Option<String>,
    },
    /// See what's going on, and what you've left lying around.
    ///
    /// Everything you can still act on: live transfers (in/out) and offers
    /// awaiting approval, files you left on a relay (links and sealed mailbox
    /// deposits), and interrupted sends you can resume. With a daemon running the
    /// offers are the ones it has parked; without one they're read straight from
    /// your inbox on the relay, since nobody else is watching it. Take any of it
    /// back with `arvolo cancel <id>` — or take an offer with `arvolo recv`.
    ///
    /// What already finished is a different question, and lives in `arvolo
    /// history`.
    Status {
        /// Keep the view open and redraw as transfers progress (needs a daemon).
        #[arg(long)]
        watch: bool,
        #[command(subcommand)]
        action: Option<StatusAction>,
    },
    /// See what already happened.
    ///
    /// The log of finished transfers — delivered, cancelled, failed, and the
    /// mailbox deposits that were handed to a relay. Read-only: nothing here can
    /// still be acted on, which is exactly what separates it from `arvolo status`.
    History {
        /// Show every record instead of the most recent ones.
        #[arg(long)]
        all: bool,
        #[command(subcommand)]
        action: Option<HistoryAction>,
    },
    /// Stay reachable for this session, deciding offer by offer.
    ///
    /// Shows each incoming offer (sender, name, size) and asks you about it,
    /// downloading accepted ones transparently; Ctrl-C ends it. Use `arvolo
    /// daemon` instead to stay reachable *always*, as a background service that
    /// decides on its own from your trust settings.
    ///
    /// If a daemon is already running, this attaches to it as the approver
    /// rather than starting a second engine — it says so when it does. Needs a
    /// relay (--relay / ARVOLO_RELAY / config).
    Listen {
        /// Directory to save accepted downloads into (default: ~/Arvolo).
        #[arg(long, add = ArgValueCompleter::new(PathCompleter::dir()))]
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
    /// Stay reachable always, as a background service.
    ///
    /// The same job as `arvolo listen`, but for a machine rather than a session:
    /// nobody is at the keyboard, so it decides from your trust settings instead
    /// of asking (`arvolo contacts trust`), and notifies you about the rest. It
    /// also exposes a local control socket, so `send`/`status`/etc. all drive
    /// this one shared instance. Meant to run under systemd/launchd. Needs a relay.
    #[cfg(unix)]
    Daemon {
        /// Directory to save accepted downloads into
        /// (default: ~/Arvolo).
        #[arg(long, add = ArgValueCompleter::new(PathCompleter::dir()))]
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
    /// Accept a parked offer by its id (see `arvolo status`) and download it.
    #[cfg(unix)]
    Accept {
        /// The offer id shown by `arvolo status`.
        #[arg(add = ArgValueCandidates::new(completions::offer_candidates))]
        offer_id: String,
        /// Save to this path instead of the daemon's download dir.
        #[arg(long, add = ArgValueCompleter::new(PathCompleter::any()))]
        out: Option<PathBuf>,
    },
    /// Reject a parked offer by its id.
    #[cfg(unix)]
    Reject {
        /// The offer id shown by `arvolo status`.
        #[arg(add = ArgValueCandidates::new(completions::offer_candidates))]
        offer_id: String,
    },
    /// Pause an in-progress transfer (hold it; resume or cancel later).
    #[cfg(unix)]
    Pause {
        /// The transfer id shown by `arvolo status`.
        #[arg(add = ArgValueCandidates::new(completions::transfer_candidates))]
        id: u64,
    },
    /// Pick up where something left off.
    ///
    /// Takes a paused transfer (a plain number), or a send that was interrupted:
    /// by its **session id**, which recovers a delivery to a contact with no file
    /// to re-supply, or by the **`arvc…` ticket** you already shared, together
    /// with its file, so the ticket you handed out keeps working.
    Resume {
        /// The id shown by `arvolo status`, or the `arvc…` ticket you shared.
        #[arg(value_name = "ID|TICKET", add = ArgValueCandidates::new(completions::resumable_candidates))]
        id: String,
        /// The original file — needed only when resuming from an `arvc…` ticket.
        #[arg(value_hint = clap::ValueHint::AnyPath)]
        path: Option<PathBuf>,
        /// Also render the reprinted ticket as a scannable QR code. Resuming a
        /// plain `arvc…` ticket prints a fresh one the receiver must use, so
        /// there is something new to scan.
        #[arg(long)]
        qr: bool,
    },
    /// Take it back.
    ///
    /// Anything `arvolo status` shows: a running transfer (a plain number), a
    /// file left on a relay — link or sealed mailbox deposit, deleted from the
    /// relay, not just locally — or a resumable send you no longer want. Also
    /// takes the `arvm…` ticket or download link itself, for withdrawing
    /// something you sent from **another machine** — that needs `--token`.
    Cancel {
        /// The id shown by `arvolo status`, or an `arvm…` ticket / download link.
        #[arg(value_name = "ID|TICKET|LINK", add = ArgValueCandidates::new(completions::cancelable_candidates))]
        id: String,
        /// The revoke token printed when you sent it. Only needed for a ticket or
        /// link this machine has no record of.
        #[arg(long)]
        token: Option<String>,
    },
    /// Manage your address book of recipients (used by `arvolo send`).
    Contacts {
        #[command(subcommand)]
        action: ContactAction,
    },
    /// Use arvolo on more than one device.
    ///
    /// Pairing another device shares one identity, so your contacts see a single
    /// id and any device can open files sent to you. It also keeps your address
    /// book in step across them.
    Device {
        #[command(subcommand)]
        action: DeviceAction,
    },
    /// Who you are here.
    ///
    /// Your public id, the fingerprint that confirms it out-of-band, and the
    /// display name you advertise. Creates an identity on first use. The id goes
    /// to stdout on its own, so `arvolo me` pipes cleanly into a message.
    Me {
        #[command(subcommand)]
        action: Option<MeAction>,
    },
    // No `version` verb: `--version` prints this binary's, and whether the daemon
    // is up (and on which version) is a *status* question — `arvolo status`
    // answers it in both directions. A verb that needed two other commands to
    // justify its existence was one verb too many.
    /// Set up <TAB> completion for your shell.
    ///
    /// Prints the shell integration. Because completion is computed by arvolo
    /// itself rather than baked into a static script, <TAB> also offers your
    /// contact names and the live ids from `arvolo status`.
    ///
    /// Add it to your shell, e.g.
    ///     arvolo completions zsh  > ~/.zfunc/_arvolo
    ///     arvolo completions bash > ~/.local/share/bash-completion/completions/arvolo
    ///     arvolo completions fish > ~/.config/fish/completions/arvolo.fish
    ///
    /// Re-run it after upgrading arvolo: the shell side and the binary side are
    /// versioned together.
    Completions {
        /// The shell to emit integration for.
        shell: CompletionShell,
    },
}

// `clear` means one thing in both places it appears — *get rid of what this view
// shows and is over* — which is why neither needs a scope flag. Under `status`
// that's the finished rows; under `history` it's the whole log, since a log is
// past by definition. It used to be one `clear` whose meaning flipped with
// `--history`: selective on the live list, total on the log.
#[derive(Subcommand)]
pub(crate) enum StatusAction {
    /// Close out what's over: drop every completed, cancelled and failed row from
    /// the list. Anything still going stays — including a mailbox send awaiting
    /// pickup, which looks done but isn't. This never touches the relay:
    /// withdraw with `arvolo cancel <id>`.
    Clear,
}

#[derive(Subcommand)]
pub(crate) enum HistoryAction {
    /// Forget the log. Does not touch the live list, your relay deposits or your
    /// resumable sends — none of which live here.
    Clear,
}

#[derive(Subcommand)]
pub(crate) enum MeAction {
    /// Show or set your display name — the self-chosen name advertised to
    /// recipients inside each sealed offer (a petname claim, never a verified
    /// identity). No argument prints the current name; pass a name to set it, or
    /// an empty string to clear it.
    Name {
        /// The display name to set. Omit to show the current one.
        name: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum ContactAction {
    /// Save (or update) a contact: a name and their public id.
    Add { name: String, id: String },
    /// List saved contacts, with who's online if a relay is configured.
    /// Pass a filter to show only matches: a public id (exact or prefix,
    /// case-insensitive) or a substring of the contact name.
    List {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        filter: Option<String>,
        /// Skip the online check. Presence is the only part of this command that
        /// touches the network, so this makes it purely local and instant.
        #[arg(long)]
        no_presence: bool,
        /// Print as JSON instead of a table, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Trade public ids with someone over a short code, and come away with each
    /// other saved *and* verified.
    ///
    /// Show a code with no argument; type theirs to answer one. The code is a
    /// SPAKE2 secret, so the channel only forms between two people who both know
    /// it — which is what makes the key that arrives through it verified, rather
    /// than merely received. It is as strong as the channel you read the code
    /// over.
    ///
    /// Not `arvolo device pair`: that shares your secret identity to make another
    /// machine *you*. This trades public ids between two different people.
    Pair {
        /// The code the other person is showing. Omit to show one yourself.
        code: Option<String>,
        /// What to file them under. Asked for interactively if omitted, and
        /// required in a non-interactive shell.
        #[arg(long)]
        name: Option<String>,
        /// Rendezvous relay (host or URL), embedded in the code you show so the
        /// other side needs no configuration.
        #[arg(long)]
        relay: Option<String>,
        /// Treat a bare relay address as `http://` instead of `https://`.
        #[arg(long)]
        use_http: bool,
        /// Also render the code as a scannable QR.
        #[arg(long)]
        qr: bool,
    },
    /// Remove a saved contact.
    Remove {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        name: String,
    },
    /// Change the local name you filed someone under, keeping everything else:
    /// their key, and your verified and trusted marks. Doing this as
    /// `remove` + `add` would silently drop both marks.
    Rename {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        old: String,
        new: String,
    },
    /// Drop advertised-name records left behind by contacts you removed. They
    /// belong to nobody and keep syncing between your devices; nothing you can
    /// still see is affected.
    Prune,
    /// Silence someone: their offers are dropped on arrival, with no prompt and
    /// no notification. Takes a contact name or a raw id — usually a stranger you
    /// have no name for. With no argument, lists who is blocked.
    Block {
        /// Contact name or base32 id. Omit to list the blocked identities.
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        who: Option<String>,
    },
    /// Stop silencing someone: their offers reach you again.
    Unblock {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        who: String,
    },
    /// Write the address book to stdout as JSON, to back it up or move it to a
    /// machine you don't want to share an identity with (which is what `arvolo
    /// device pair` would do).
    Export,
    /// Read an address book back in. Names already in use are skipped, so an
    /// import can never silently rebind an existing contact to a different key.
    ///
    /// Verified and trusted marks are **not** imported unless you ask: those are
    /// decisions you made looking at a fingerprint, not data to copy around.
    Import {
        /// The file to read, or `-` for stdin.
        #[arg(value_hint = clap::ValueHint::FilePath)]
        file: String,
        /// Also import the verified and trusted marks from the file, trusting
        /// whoever produced it as much as you trust yourself.
        #[arg(long)]
        with_marks: bool,
    },
    /// Mark a contact verified after comparing its fingerprint out-of-band.
    /// Shows the fingerprint and asks for confirmation before marking; pass
    /// `--yes` to skip the prompt (required in a non-interactive shell).
    Verify {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        name: String,
        /// Mark verified without the confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Remove a contact's verified mark.
    Unverify {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        name: String,
    },
    /// Trust a contact so the daemon auto-downloads their files without asking.
    /// Refuses an unverified contact unless `--force` is given (auto-downloading
    /// from a key you haven't confirmed out-of-band is a MITM risk).
    Trust {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        name: String,
        /// Trust even if the contact isn't verified yet.
        #[arg(long)]
        force: bool,
    },
    /// Stop auto-downloading from a contact (their files will ask again).
    Untrust {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        name: String,
    },
    /// Approve a contact's advertised display name (the name they chose for
    /// themselves). Pins the pending name so it's shown from now on. Pass a
    /// contact name or a raw id, or `--all` to approve every pending name.
    AcceptName {
        /// Contact name or base32 id. Omit when using `--all`.
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
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
    /// Publish this device's address book and merge any pending updates now.
    Sync,
    /// Show sync state (identity fingerprint, contact count, last sync).
    Status,
}

/// The shells `arvolo completions` can emit integration for. These are the five
/// [`clap_complete::env::Shells::builtins`], named the way their users name them.
#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub(crate) enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    Elvish,
    Powershell,
}

impl CompletionShell {
    /// The name [`clap_complete::env::Shells::completer`] matches on.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
            Self::Elvish => "elvish",
            Self::Powershell => "powershell",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// clap validates the whole tree — duplicate names, conflicting short flags,
    /// a `conflicts_with` pointing at an arg that doesn't exist — only when asked.
    /// Asking here turns "the CLI is malformed" from a runtime panic on one
    /// unlucky subcommand into a compile-cheap test failure.
    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    /// The grouped `--help` listing is rendered by hand, so nothing forces it to
    /// keep up with the enum. This does: a new verb that isn't filed in
    /// [`COMMAND_GROUPS`] would be missing from `--help` with nothing to notice
    /// it, and a name left behind after a rename would print a row for a command
    /// that no longer exists. Both directions fail here instead.
    ///
    /// Uses the same `build()`-ed tree as [`build_cli`], so the unix-only verbs
    /// are checked exactly as they exist in this build.
    #[test]
    fn every_command_is_grouped() {
        let mut cmd = Cli::command();
        cmd.build();

        let mut grouped: HashSet<&str> = HashSet::new();
        for (heading, names) in COMMAND_GROUPS {
            for name in *names {
                assert!(
                    grouped.insert(name),
                    "`{name}` is filed under more than one group (last: {heading})"
                );
            }
        }

        for sub in cmd.get_subcommands() {
            let name = sub.get_name();
            assert!(
                grouped.contains(name),
                "`{name}` is missing from COMMAND_GROUPS, so it would not appear in `--help`"
            );
        }
        for name in &grouped {
            assert!(
                cmd.find_subcommand(name).is_some(),
                "COMMAND_GROUPS lists `{name}`, which is not a command"
            );
        }
    }

    /// Every command carries a short first line for the listing — an empty cell
    /// in `--help` is the kind of thing only a reader notices.
    #[test]
    fn every_command_has_a_summary() {
        let mut cmd = Cli::command();
        cmd.build();
        for sub in cmd.get_subcommands() {
            assert!(
                sub.get_about().is_some_and(|a| !a.to_string().is_empty()),
                "`{}` has no summary line for `--help`",
                sub.get_name()
            );
        }
    }
}
