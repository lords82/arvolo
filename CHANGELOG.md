# Changelog

## v0.12.0-rc3

Same code as rc2, plus the fix that lets a prerelease ship Linux packages at all:
the .deb files were being built and then discarded when the .rpm in the same step
rejected the hyphen in the version. Both now get `0.12.0~rc3`, which is how both
ecosystems spell "sorts before the final release".

## v0.12.0-rc2

Second prerelease, on top of rc1.

- **Preparing a send now uses several cores.** Reading stays sequential — that is
  what a disk wants — while sealing and hashing the chunks runs on four threads.
  `seal_chunk` is a pure function of `(key, index, bytes)`, so the digests are
  identical to the single-threaded ones; only the wall clock changes. Measured
  132 → 411 MiB/s, which takes a 10.7 GiB file from 83 seconds to 27 before the
  recipient is offered anything.
- **`--to me` is wired up end to end**: the two halves of sending between your own
  devices had been written but never connected.
- **The relay's inbox long-poll holds properly**, and a deposit wakes it.
- A `cargo fmt` sweep: the check had been failing for a while.

## Unreleased

### Fixed

- **Every client asked the relay for its inbox every two seconds, forever.** The
  long-poll ended the moment the slot was non-empty — and the contact-sync cell
  sits in that slot permanently, so for anyone with sync on it ended immediately,
  every round, and a 25-second hold became 30 requests a minute (held down only by
  a floor on the client side, added after this pegged a relay at >700 req/s). A
  poll now says which rows it already holds and the relay waits for one it does
  not: about 2.4 requests a minute instead. The relay has to be updated first —
  an older one ignores the header and answers exactly as it did before.
- **A held poll cost the relay a database scan twice a second.** Its steady-state
  work grew with the number of *connected* clients rather than with the number of
  deposits: ten thousand idle inboxes meant twenty thousand queries a second, all
  of them answering "still nothing". A deposit now wakes exactly the slot it
  landed in, which also takes the delivery latency of an offer arriving mid-hold
  from up to half a second down to none.
- **Accepting a big file left it sitting at 0 B for minutes.** A file the relay
  refuses as too large can only go peer-to-peer, so the sender keeps trying while
  the recipient is away — on a backoff that grows to five minutes. Nothing told it
  the recipient had said yes, so an accept landed in the middle of that sleep and
  both ends showed a transfer that was doing nothing. The sender now spends the
  wait held open on its offer's status, and the relay answers the moment the
  recipient touches it: accepting starts the transfer in a round trip. Both kinds
  of recipient reach it — the daemon, which acks on accept, and a bare
  `arvolo recv`, which only lists its inbox before dialling.
- **Every delivery attempt left another copy of the same offer.** Each attempt
  posted a fresh offer and withdrew it on the way out, so a recipient who was away
  for a while collected a row per attempt, all but one of them already dead — and
  accepting any of the dead ones named an offer the sender had stopped listening
  for. A held send now keeps one offer standing for its whole life and withdraws
  it once, when the send really ends.
- **Pausing a send and resuming it re-encrypted the whole file.** The preparation —
  the chunk digests, and the content key and node id they belong to — lived in the
  delivery task, and a pause is what ends that task: resuming paid the pass again
  (half a minute for 10 GB) and, worse, minted a fresh key and node id. The
  recipient's transfer was then pointing at a node nobody was serving, and the only
  way back was another offer for them to approve by hand. The preparation now
  belongs to the held send, which survives the pause, so resuming carries on with
  the same content under the same node id.
- **A file from another of your own devices asked for approval.** Two halves of
  `--to me` shipped in rc1 were written but never wired up. An offer that opens
  and authenticates as your own identity can only have been sealed by something
  holding your identity secret — your other device — so it now downloads without
  asking, in all three places that decide (the daemon, `recv` attached to it, and
  standalone `recv`), rather than asking your permission to receive what you just
  sent yourself.
- **`send --to me` offered to pair devices that were already paired.** The marker
  a completed `device pair`/`device join` is supposed to leave was never written,
  so until the first sync round landed a snapshot from the other side, a
  freshly-paired identity still looked unpaired.

## v0.12.0-rc1 — cancelling, and saying what is happening

A prerelease off the work branch, for dogfooding.

### Fixed

- **A live send could not be cancelled.** Marking a send `Active` went through the
  setter that drops the transfer's cancel token, so from the moment it started
  serving nothing could stop it: `cancel` found no token, did nothing, and said
  nothing. The window sat on "cancelling…" until it was restarted.
- **Nothing bounded a request to the daemon.** No timeout anywhere in the IPC
  client, so a request the daemon never answered hung its caller for good.
- **Cancel is now total**: every live transfer either reaches a terminal state or
  has a task that will announce one, and the daemon says so instead of answering
  `Ok` for an id it has never heard of.
- **A cancel during a mailbox upload** was only noticed once the upload finished.

### New

- **"Preparing…"** — a send spends its first minutes reading and encrypting the
  payload, with nothing on the wire and the recipient not yet told anything. It
  said "active, 0 B", which is what a stuck transfer looks like.
- **That preparation is no longer repeated per attempt.** A delivery loop retrying
  against a recipient who has not connected yet reused to redo the whole
  read-and-encrypt pass every time — about a minute and a half per 10 GB — and
  minted a new ticket each round, so an offer already in the recipient's inbox
  went stale. It is computed once and carried.
- **Save the sender while their file is arriving**, in the CLI and the GUI: their
  id is on screen exactly once, and that is also the only moment you know who
  they are.
- **`daemon status` names the process**: pid and binary, in the CLI and in the
  GUI's settings, above the button that restarts it.
- **Direct sends between your own devices** (`--to me`), with a device id in the
  beacon.

## v0.11.0 — the CLI, simplified

One verb sends, one verb receives. The surface drops from 18 top-level commands
to 14, output becomes pipeable, and every id a person types is the same 8-hex
handle. **This is a breaking release: the old verbs are gone, with no aliases.**

### Command mapping

| Before | Now |
|---|---|
| `arvolo send <who> <paths…>` | `arvolo send <paths…> --to <who>` |
| `arvolo send … --deposit` | `arvolo send … --to <who> --mailbox` |
| `arvolo link <paths…>` | `arvolo send <paths…> --link` |
| `arvolo code <paths…>` | `arvolo send <paths…> --code` |
| `arvolo ticket <paths…>` | `arvolo send <paths…>` (writes a `.arvolo` ticket file; `--ticket` prints the raw string) |
| `arvolo accept <26-char offer id>` | `arvolo recv <8-hex handle>` (unique prefix is enough) |
| `arvolo reject <26-char offer id>` | `arvolo decline <handle>` |
| `arvolo listen --yes` / `--auto-accept-*` | `arvolo listen --accept contacts\|verified\|all` |
| `arvolo daemon` | `arvolo daemon start` — plus `run` (foreground, for systemd/launchd), `stop`, `status` |
| `arvolo contacts pair [code]` | `arvolo contacts add <name> [code]` (no code: shows one and waits) |
| `arvolo contacts block` (no arg) | `arvolo contacts list --blocked` |
| `arvolo contacts accept-name <n>` | `arvolo contacts rename <n>` (no new name adopts the pending one) |
| `arvolo contacts unverify` / `untrust` | `arvolo contacts verify\|trust <n> --undo` |
| `arvolo contacts prune` | removed — pruning runs automatically after a sync merge |
| `arvolo contacts export` | removed — `arvolo contacts list --json` (import reads it as-is) |
| `arvolo history --all` | removed with the 20-row cap: `history` prints everything, pipe it |
| `arvolo cancel --token <t>` | removed — the token it asked for was never printed anywhere |
| `--use-http` (8 commands) | removed — write the scheme in the address: `--relay http://host:port` |
| `link --password` | removed — it was documented but always failed |
| config `debug` / `ARVOLO_DEBUG` | removed — use `-v`/`RUST_LOG` |

### New

- **`.arvolo` ticket files.** A bare `arvolo send file` writes `file.arvolo` —
  share it over any channel, like a .torrent; the other side runs
  `arvolo recv file.arvolo` (or double-clicks it: the desktop app registers the
  extension, opens on it, and can save one from a ticket result).
- **One handle shape.** Everything you type back is 8 hex chars with unique-prefix
  matching: transfers (restart-stable now — the old numeric ids were reassigned on
  every daemon restart), relay deposits, resumable sends, waiting offers. Tickets,
  codes, links and public ids stay what they are: capabilities.
- **Pipeable output.** stdout carries exactly the artefact — the URL, the code,
  the ticket, the saved file's path — and all narration goes to stderr.
  `arvolo send --link f | pbcopy` copies the URL and nothing else.
- **`--json`** on `status`, `history`, `device status`, `daemon status` (contacts
  list already had it). Handles included; secrets never.
- **Progress everywhere**: the mailbox download used to be silent until the end;
  the live send used to smear `\r` into logs. One renderer, bar on a terminal,
  milestone lines in a pipe.
- **Bare `--password` prompts on the TTY** instead of demanding the secret on
  argv (`--password=<pw>` stays, for scripts).
- Parse-time conflicts: the flag combinations that make no sense
  (`--link --code`, `--to --ticket`, `--keep` without `--code`, …) fail
  immediately, with the reason.
- Received files land in the configured download dir through **every** door, and
  derived names never overwrite — an existing `report.pdf` makes the new one
  `report (1).pdf`.

### Fixed

- Desktop notifications no longer fire twice while the app is open (a bitwise-NOT
  bug made the daemon notify unconditionally).
- `arvolo me name` now takes effect immediately when the daemon runs — it used
  to keep advertising the old name until a restart.
- `--ttl/--max/--password` are no longer silently dropped when a send is handed
  to the daemon and the recipient goes offline between the two presence probes.
- Removing a contact from the desktop app now clears its verified/trusted marks,
  exactly as the CLI does.
- Accepting or declining an offer from the CLI now removes its row from any open
  window (new `offer_gone` event).
- Resumable-send records are deleted on delivery and swept after 30 days — the
  list used to grow forever.
- The first-run wizard no longer claims "no relay set" while a built-in default
  exists; `arvolo status` names the built-in relay before first contacting it.
- Typing "Yes" at a y/n prompt no longer means no (one prompt had a
  case-sensitive accept list).
- **Test builds move data at full speed**: dependencies now compile at
  `opt-level 2` even in dev, ending the ~120 KB/s crawl that made every
  bulk-transfer test flaky on some machines.

### Upgrade notes

- **Re-run `arvolo completions <shell>`** — the completion script and the binary
  are versioned together, and the verbs changed.
- **systemd/launchd units**: the daemon verb is now `arvolo daemon run`
  (templates in `packaging/` are updated; edit any unit you deployed by hand).
- The desktop app must be updated together with the CLI it drives (same
  workspace version, as always).
