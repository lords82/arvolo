use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::engine::{ArgValueCandidates, ArgValueCompleter, PathCompleter};

use crate::completions;

/// How `--help` groups the commands: by the job you came to do, not alphabetically.
///
/// Fourteen verbs in one flat column is a list you read rather than scan. clap 4
/// has no notion of subcommand groups — `subcommand_help_heading` only renames the
/// single "Commands:" block — so the listing is rendered by hand in
/// [`grouped_commands`], and the whole risk of doing that is drift: a verb nobody
/// files here would silently vanish from `--help`. The `every_command_is_grouped`
/// test is the guard, and it checks both directions.
const COMMAND_GROUPS: &[(&str, &[&str])] = &[
    ("Send", &["send"]),
    ("Receive", &["recv", "decline", "listen"]),
    (
        "Follow it, and take it back",
        &["status", "history", "pause", "resume", "cancel"],
    ),
    ("People and devices", &["contacts", "device", "me"]),
    ("Arvolo itself", &["daemon", "completions", "help"]),
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

    // A name that isn't in this build is simply skipped rather than printed as a
    // row pointing at nothing.
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

/// Parse a duration the way people write one: `7d`, `12h`, `45m`, `30s`, or a
/// bare number of seconds.
pub(crate) fn parse_ttl(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration — write 7d, 12h, 45m or seconds".into());
    }
    if let Ok(n) = s.parse::<u64>() {
        return Ok(n);
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: u64 = num
        .trim()
        .parse()
        .map_err(|_| format!("'{s}' is not a duration — write 7d, 12h, 45m or seconds"))?;
    let mult = match unit {
        "s" | "S" => 1,
        "m" | "M" => 60,
        "h" | "H" => 3600,
        "d" | "D" => 86_400,
        _ => return Err(format!("'{unit}' is not a unit — use s, m, h or d")),
    };
    n.checked_mul(mult)
        .ok_or_else(|| format!("'{s}' overflows"))
}

/// The relay flag, defined once and flattened wherever a command can talk to a
/// relay — so the wording, and any future change to it, exists in one place.
#[derive(clap::Args)]
pub(crate) struct RelayOpts {
    /// Relay host or URL. A bare host gets `https://`; write `http://host:port`
    /// for a plaintext/LAN relay. Defaults to ARVOLO_RELAY / config `relay`.
    #[arg(long)]
    pub(crate) relay: Option<String>,
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

// One verb sends, one verb receives; everything else follows or configures.
//
// `send` is one verb with *modes*: the default writes a `.arvolo` ticket file
// (pure P2P, nothing touches a relay until someone redeems it); `--link`,
// `--code` and `--to` choose the other shapes. The combinations that make no
// sense are declared as clap conflicts, so they fail at parse time with the
// reason, not at runtime with a surprise.
//
// Receiving deliberately stays **one** verb: the sender picks the artefact, and
// the receiver shouldn't have to know which one arrived — paste it and go.
//
// Each variant's doc comment opens with one short line: clap shows that alone in
// the command list and the rest under `arvolo <cmd> --help`, so the top-level
// help stays scannable.
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Send files: a .arvolo ticket file by default, or to a contact, or as a link/code.
    ///
    /// With no flags it writes `<name>.arvolo` next to you — a small ticket file,
    /// like a .torrent: share it over any channel, and the other side runs
    /// `arvolo recv <file>.arvolo`. The transfer itself is pure P2P; nothing is
    /// uploaded anywhere until they redeem it, and it keeps working while this
    /// machine stays reachable (a running daemon serves it in the background).
    ///
    /// `--to` delivers to a person instead: live to their daemon if they're
    /// online, else sealed into the relay mailbox with an `arvm…` ticket printed.
    /// `--link` uploads to the relay and prints a browser URL (works for people
    /// without arvolo, survives you going offline). `--code` prints a short code
    /// to read out loud; the payload still travels P2P.
    #[command(
        group(clap::ArgGroup::new("artefact").args(["link", "code", "ticket"])),
        group(clap::ArgGroup::new("relayed").args(["link", "to"]).multiple(true)),
        group(clap::ArgGroup::new("scannable").args(["link", "code"]).multiple(true)),
        after_help = "Examples:\n  \
            arvolo send report.pdf                  writes report.pdf.arvolo — share that file\n  \
            arvolo send report.pdf --to alice       to a contact (live, or mailbox if offline)\n  \
            arvolo send report.pdf --link           browser URL for someone without arvolo\n  \
            arvolo send report.pdf --code           4821-crater-mango, short enough to dictate\n  \
            arvolo send *.jpg --to alice -m 'the photos'   several files, packed, with a note"
    )]
    Send {
        /// The files or folders to send. Several paths (or a folder) are packed
        /// into one archive automatically.
        #[arg(required = true, num_args = 1.., value_hint = clap::ValueHint::AnyPath)]
        paths: Vec<PathBuf>,
        /// Deliver to a saved contact name or a public id: live if they're
        /// online, else deposited in the relay mailbox for them.
        #[arg(long, short = 't', value_name = "WHO",
              conflicts_with = "artefact",
              add = ArgValueCandidates::new(completions::contact_candidates))]
        to: Option<String>,
        /// Deposit in the mailbox even if they're online (send-and-forget).
        /// (The explicit conflicts are needed on every `requires = "to"` flag:
        /// clap reads a required arg that sits in a group as satisfied by any
        /// member of that group, and `to` shares "relayed" with `link`.)
        #[arg(long, requires = "to", conflicts_with = "artefact")]
        mailbox: bool,
        /// A public browser download URL instead of the `.arvolo` file. Anyone
        /// with the URL can download it — no arvolo needed on their side, and it
        /// keeps working after you go offline (the encrypted file lives on the
        /// relay; the key stays in the URL fragment the relay never sees).
        #[arg(long)]
        link: bool,
        /// A short code to read out loud (like 4821-crater-mango) instead of the
        /// `.arvolo` file. The file still travels P2P — the relay only brokers
        /// the rendezvous.
        #[arg(long)]
        code: bool,
        /// Print the raw `arvc…` ticket on stdout instead of writing a `.arvolo`
        /// file — for scripts and pipes.
        #[arg(long)]
        ticket: bool,
        /// Attach a short note delivered *with* the file — it rides inside the
        /// E2E-sealed offer, so the relay never sees it.
        #[arg(long, short = 'm', requires = "to", conflicts_with = "artefact")]
        note: Option<String>,
        /// How long the relay keeps a --link or mailbox deposit: 7d, 12h, 45m or
        /// seconds.
        #[arg(long, default_value = "7d", value_parser = parse_ttl,
              requires = "relayed", value_name = "DURATION")]
        ttl: u64,
        /// Max downloads before the relay deletes it (default: 1 for a mailbox
        /// send, no cap for a --link).
        #[arg(long, requires = "relayed")]
        max: Option<u32>,
        /// Password-protect a mailbox send (E2E — required to decrypt). Write
        /// `--password` alone to be prompted, or `--password=<pw>` inline.
        /// (Same group quirk as `--mailbox`: the artefact conflict is explicit.)
        #[arg(long, requires = "to", conflicts_with = "artefact",
              num_args = 0..=1, require_equals = true,
              default_missing_value = "", value_name = "PASSWORD")]
        password: Option<String>,
        /// Keep the code working for every receiver until you `arvolo cancel`
        /// it, instead of retiring it once the first one has the file.
        ///
        /// (`requires` alone is not enough here: clap reads a required arg that
        /// sits in a group as satisfied by any member of that group, so
        /// `--keep --ticket` would slip through without the explicit conflicts.)
        #[arg(long, requires = "code", conflicts_with_all = ["ticket", "link"])]
        keep: bool,
        /// Serve it **in this terminal** (blocking, Ctrl-C to stop) instead of
        /// handing it to the daemon.
        #[arg(long, conflicts_with_all = ["link", "to"])]
        foreground: bool,
        /// Also render the link or code as a scannable QR.
        /// (Same group quirk as `keep`: the ticket conflict must be explicit.)
        #[arg(long, requires = "scannable", conflicts_with = "ticket")]
        qr: bool,
        #[command(flatten)]
        relay: RelayOpts,
    },
    /// Receive anything: a .arvolo file, ticket, code, link — or pick from what's waiting.
    ///
    /// One verb for all of them — it works out which it is: a `.arvolo` file or
    /// `arvc…` ticket or pairing code fetches live P2P, an `arvm…` mailbox ticket
    /// or download link decrypts from the relay, and an 8-hex handle (what
    /// `arvolo status` shows next to a waiting offer — a unique prefix is enough)
    /// accepts that offer.
    ///
    /// With nothing to paste it lists instead: the sends addressed to *you*,
    /// still sealed on the relay, and you pick one. A code, ticket or link never
    /// appears in that list — it *is* the permission to fetch, so nothing on the
    /// relay knows which ones are meant for you, which is also what stops anyone
    /// from enumerating yours.
    #[command(after_help = "Examples:\n  \
        arvolo recv report.pdf.arvolo      a ticket file someone shared with you\n  \
        arvolo recv 4821-crater-mango      a code someone read to you\n  \
        arvolo recv arvm…                  a mailbox ticket\n  \
        arvolo recv                        see what's waiting, pick one\n  \
        arvolo recv 8cd6                   take the waiting offer with that handle")]
    Recv {
        /// A `.arvolo` file, `arvc…`/`arvm…` ticket, pairing code, download link,
        /// or the handle of a waiting offer. Leave it out to see what's waiting
        /// for you and take one from the list.
        #[arg(value_name = "WHAT", add = ArgValueCandidates::new(completions::offer_candidates))]
        what: Option<String>,
        #[arg(short, long, add = ArgValueCompleter::new(PathCompleter::any()))]
        out: Option<PathBuf>,
        /// Password for a password-protected mailbox ticket. Write `--password`
        /// alone to be prompted, or `--password=<pw>` inline.
        #[arg(long, num_args = 0..=1, require_equals = true,
              default_missing_value = "", value_name = "PASSWORD")]
        password: Option<String>,
    },
    /// Decline a waiting offer without fetching it.
    Decline {
        /// The offer's handle, as shown by `arvolo status` or the `arvolo recv`
        /// picker. A unique prefix is enough.
        #[arg(add = ArgValueCandidates::new(completions::offer_candidates))]
        handle: String,
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
        #[arg(long, conflicts_with = "json")]
        watch: bool,
        /// Print as JSON instead of the human view, for scripts. Handles are the
        /// ids to act on; secrets (content keys, revoke tokens) never appear.
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        action: Option<StatusAction>,
    },
    /// See what already happened.
    ///
    /// The log of finished transfers — delivered, cancelled, failed, and the
    /// mailbox deposits that were handed to a relay. Read-only: nothing here can
    /// still be acted on, which is exactly what separates it from `arvolo status`.
    /// Prints the whole log; pipe it (`arvolo history | head`) to trim.
    History {
        /// Print as JSON instead of the human view, for scripts.
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        action: Option<HistoryAction>,
    },
    /// Stay reachable for this session, deciding offer by offer.
    ///
    /// Shows each incoming offer (sender, name, size) and asks you about it,
    /// downloading accepted ones transparently; Ctrl-C ends it. Use `arvolo
    /// daemon start` instead to stay reachable *always*, as a background service
    /// that decides on its own from your trust settings.
    ///
    /// If a daemon is already running, this attaches to it as the approver
    /// rather than starting a second engine — it says so when it does. Needs a
    /// relay (--relay / ARVOLO_RELAY / config).
    Listen {
        /// Answer yes for a whole group instead of being asked each time:
        /// `contacts` auto-accepts saved contacts, `verified` only verified
        /// ones, `all` accepts everyone. Trusted contacts are always
        /// auto-accepted, with or without this.
        #[arg(long, value_enum, value_name = "WHO")]
        accept: Option<AcceptWho>,
        /// Don't auto-sync your address book across your linked devices.
        #[arg(long)]
        no_sync: bool,
        #[command(flatten)]
        relay: RelayOpts,
    },
    /// Run arvolo as a background service (start|run|stop|status).
    ///
    /// The same job as `arvolo listen`, but for a machine rather than a session:
    /// nobody is at the keyboard, so it decides from your trust settings instead
    /// of asking (`arvolo contacts trust`), and notifies you about the rest. It
    /// also exposes a local control socket, so `send`/`status`/etc. all drive
    /// this one shared instance.
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Pause an in-progress transfer (hold it; resume or cancel later).
    Pause {
        /// The transfer id shown by `arvolo status`.
        #[arg(add = ArgValueCandidates::new(completions::transfer_candidates))]
        id: String,
    },
    /// Pick up where something left off.
    ///
    /// Takes a paused or interrupted transfer by the id `arvolo status` shows, a
    /// send that was interrupted — by its session id, or by the `arvc…` ticket /
    /// `.arvolo` file you already shared together with its original file, so the
    /// ticket you handed out keeps working.
    Resume {
        /// The id shown by `arvolo status`, or the `arvc…` ticket / `.arvolo`
        /// file you shared.
        #[arg(value_name = "ID|TICKET|FILE", add = ArgValueCandidates::new(completions::resumable_candidates))]
        id: String,
        /// The original file — needed only when resuming from an `arvc…` ticket
        /// or `.arvolo` file.
        #[arg(value_hint = clap::ValueHint::AnyPath)]
        path: Option<PathBuf>,
    },
    /// Take it back.
    ///
    /// Anything `arvolo status` shows: a running transfer, a file left on a
    /// relay — link or sealed mailbox deposit, deleted from the relay, not just
    /// locally — or a resumable send you no longer want. Also takes the `arvm…`
    /// ticket or download link itself.
    Cancel {
        /// The id shown by `arvolo status`, or an `arvm…` ticket / download link.
        #[arg(value_name = "ID|TICKET|LINK", add = ArgValueCandidates::new(completions::cancelable_candidates))]
        id: String,
    },
    /// Manage your address book of recipients (used by `arvolo send --to`).
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
    #[command(alias = "whoami")]
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

/// Who `arvolo listen --accept` says yes to without asking.
#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub(crate) enum AcceptWho {
    /// Saved contacts (still prompts for strangers).
    Contacts,
    /// Verified contacts only (safer: you compared their fingerprint).
    Verified,
    /// Everyone — accept every incoming offer without prompting.
    All,
}

/// How the daemon runs and is driven.
#[derive(Subcommand)]
pub(crate) enum DaemonAction {
    /// Start it in the background and return.
    Start {
        /// Directory to save accepted downloads into (default: ~/Arvolo).
        #[arg(long, add = ArgValueCompleter::new(PathCompleter::dir()))]
        download_dir: Option<PathBuf>,
        /// Don't auto-sync your address book across your linked devices.
        #[arg(long)]
        no_sync: bool,
        #[command(flatten)]
        relay: RelayOpts,
    },
    /// Run it in this terminal (blocking) — for systemd/launchd and the GUI.
    Run {
        /// Directory to save accepted downloads into (default: ~/Arvolo).
        #[arg(long, add = ArgValueCompleter::new(PathCompleter::dir()))]
        download_dir: Option<PathBuf>,
        /// Don't auto-sync your address book across your linked devices.
        #[arg(long)]
        no_sync: bool,
        #[command(flatten)]
        relay: RelayOpts,
    },
    /// Stop the running daemon.
    Stop,
    /// Is it running, and what is it doing: version, identity, relay, transfers.
    Status {
        /// Print as JSON instead of the human view, for scripts.
        #[arg(long)]
        json: bool,
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
    /// Add someone: by their public id, by a pairing code, or by showing one.
    ///
    /// Three forms, one verb:
    ///   arvolo contacts add alice <public-id>      you have their id — saved directly
    ///   arvolo contacts add alice 4821-crater-mango   they're showing a pairing code — join it
    ///   arvolo contacts add alice                  show a code yourself and wait for them
    ///
    /// The pairing code is a SPAKE2 secret, so the channel only forms between two
    /// people who both know it — which is what makes the key that arrives through
    /// it verified, rather than merely received. It is as strong as the channel
    /// you read the code over.
    ///
    /// Not `arvolo device pair`: that shares your secret identity to make another
    /// machine *you*. This trades public ids between two different people.
    #[command(after_help = "Examples:\n  \
        arvolo contacts add alice if2xmne…    their public id (from `arvolo me` on their side)\n  \
        arvolo contacts add alice 4821-crater-mango    the code alice is showing\n  \
        arvolo contacts add alice             show a code, read it to alice, wait")]
    Add {
        /// What to file them under.
        name: String,
        /// Their public id (base32), or the pairing code they're showing. Omit
        /// to show a pairing code yourself.
        #[arg(value_name = "ID|CODE")]
        id_or_code: Option<String>,
        /// Also render the pairing code as a scannable QR.
        #[arg(long)]
        qr: bool,
        #[command(flatten)]
        relay: RelayOpts,
    },
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
        /// List the blocked identities instead.
        #[arg(long)]
        blocked: bool,
    },
    /// Remove a saved contact.
    Remove {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        name: String,
    },
    /// Change the local name you filed someone under, keeping everything else:
    /// their key, and your verified and trusted marks. Doing this as
    /// `remove` + `add` would silently drop both marks.
    ///
    /// With no new name, adopts the display name they advertised for themselves
    /// (shown as pending in `arvolo contacts list`), after asking.
    Rename {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        old: String,
        /// The new name. Omit to adopt their advertised name.
        new: Option<String>,
    },
    /// Silence someone: their offers are dropped on arrival, with no prompt and
    /// no notification. Takes a contact name or a raw id — usually a stranger you
    /// have no name for. See who's blocked with `contacts list --blocked`.
    Block {
        /// Contact name or base32 id.
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        who: String,
    },
    /// Stop silencing someone: their offers reach you again.
    Unblock {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        who: String,
    },
    /// Read an address book back in (the JSON `contacts list --json` writes).
    /// Names already in use are skipped, so an import can never silently rebind
    /// an existing contact to a different key.
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
        /// Remove the verified mark instead.
        #[arg(long, conflicts_with = "yes")]
        undo: bool,
        /// Mark verified without the confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Trust a contact so the daemon auto-downloads their files without asking.
    /// Refuses an unverified contact unless `--force` is given (auto-downloading
    /// from a key you haven't confirmed out-of-band is a MITM risk).
    Trust {
        #[arg(add = ArgValueCandidates::new(completions::contact_candidates))]
        name: String,
        /// Stop auto-downloading from them instead (their files will ask again).
        #[arg(long, conflicts_with = "force")]
        undo: bool,
        /// Trust even if the contact isn't verified yet.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum DeviceAction {
    /// On a device you already use: show a pairing code for a new device to
    /// join. Shares this device's identity + address book once it connects.
    Pair {
        /// Also render the pairing code as a scannable QR code.
        #[arg(long)]
        qr: bool,
        #[command(flatten)]
        relay: RelayOpts,
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
    /// Show sync state: identity fingerprint, contact count, auto-sync on/off.
    Status {
        /// Print as JSON instead of the human view, for scripts.
        #[arg(long)]
        json: bool,
    },
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

    /// The mode combinations that make no sense must fail at parse time, with
    /// clap's error naming both flags — not at runtime, after something already
    /// left the machine.
    #[test]
    fn send_mode_conflicts_are_declared() {
        let refused = [
            // Two artefacts at once.
            vec!["arvolo", "send", "f", "--link", "--code"],
            vec!["arvolo", "send", "f", "--link", "--ticket"],
            vec!["arvolo", "send", "f", "--code", "--ticket"],
            // A recipient is not an artefact.
            vec!["arvolo", "send", "f", "--to", "alice", "--link"],
            vec!["arvolo", "send", "f", "--to", "alice", "--code"],
            vec!["arvolo", "send", "f", "--to", "alice", "--ticket"],
            // Flags that require a mode they don't have.
            vec!["arvolo", "send", "f", "--mailbox"],
            vec!["arvolo", "send", "f", "--note", "hi"],
            vec!["arvolo", "send", "f", "--password"],
            vec!["arvolo", "send", "f", "--keep"],
            vec!["arvolo", "send", "f", "--keep", "--ticket"],
            vec!["arvolo", "send", "f", "--ttl", "1d"],
            vec!["arvolo", "send", "f", "--max", "3"],
            vec!["arvolo", "send", "f", "--qr"],
            vec!["arvolo", "send", "f", "--qr", "--ticket"],
            vec!["arvolo", "send", "f", "--foreground", "--link"],
            vec!["arvolo", "send", "f", "--foreground", "--to", "alice"],
            // The person-only flags never apply to an artefact.
            vec!["arvolo", "send", "f", "--link", "--password=pw"],
            vec!["arvolo", "send", "f", "--link", "--mailbox"],
            vec!["arvolo", "send", "f", "--code", "--note", "hi"],
        ];
        for argv in refused {
            assert!(
                Cli::try_parse_from(&argv).is_err(),
                "{argv:?} must be refused at parse time"
            );
        }

        let accepted = [
            vec!["arvolo", "send", "f"],
            vec!["arvolo", "send", "f", "--ticket"],
            vec!["arvolo", "send", "f", "--foreground"],
            vec!["arvolo", "send", "f", "--link", "--ttl", "12h", "--max", "3", "--qr"],
            vec!["arvolo", "send", "f", "--code", "--keep", "--foreground", "--qr"],
            vec!["arvolo", "send", "f", "--to", "alice", "--mailbox", "-m", "hi"],
            vec!["arvolo", "send", "f", "--to", "alice", "--password", "--ttl", "1d"],
            vec!["arvolo", "send", "f", "-t", "alice", "--max", "1"],
        ];
        for argv in accepted {
            if let Err(e) = Cli::try_parse_from(&argv) {
                panic!("{argv:?} must parse, got: {e}");
            }
        }
    }

    /// `--password` without a value means "prompt me" and must stay
    /// distinguishable from no `--password` at all.
    #[test]
    fn bare_password_flag_means_prompt() {
        let cli = Cli::try_parse_from(["arvolo", "send", "f", "--to", "a", "--password"]).unwrap();
        match cli.command {
            Command::Send { password, .. } => assert_eq!(password.as_deref(), Some("")),
            _ => unreachable!(),
        }
        let cli = Cli::try_parse_from(["arvolo", "send", "f", "--to", "a"]).unwrap();
        match cli.command {
            Command::Send { password, .. } => assert_eq!(password, None),
            _ => unreachable!(),
        }
    }

    /// Durations read the way people write them.
    #[test]
    fn ttl_accepts_human_durations() {
        assert_eq!(parse_ttl("3600"), Ok(3600));
        assert_eq!(parse_ttl("30s"), Ok(30));
        assert_eq!(parse_ttl("45m"), Ok(45 * 60));
        assert_eq!(parse_ttl("12h"), Ok(12 * 3600));
        assert_eq!(parse_ttl("7d"), Ok(7 * 86_400));
        assert!(parse_ttl("7w").is_err());
        assert!(parse_ttl("").is_err());
        assert!(parse_ttl("d").is_err());
    }
}
