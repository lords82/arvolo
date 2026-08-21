# Changelog

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
