# The arvolo daemon (always-on receive + local control)

By default `arvolo` is one-shot: `send`/`recv` move a single payload and exit.
The **daemon** turns the engine into a persistent background service so you can:

- **stay online and receive** files without keeping a terminal open, surviving
  logout/reboot when run under systemd/launchd;
- **queue several sends** at once — `arvolo send` hands off to the daemon and the
  transfers run concurrently;
- **see everything in one place** — `arvolo status` lists incoming *and* outgoing
  transfers plus offers awaiting approval;
- **auto-download from people you trust** while everyone else waits for your
  approval (the default).

One engine, many front-ends: the CLI talks to the daemon over a local
Unix-domain socket at `~/.config/arvolo/daemon.sock` (owner-only permissions are
the access control — no token to manage). A future desktop GUI drives the same
socket. It needs a relay (`--relay` / `ARVOLO_RELAY` / config `relay`).

## Run it

```sh
arvolo daemon start                 # spawn in the background and return
arvolo daemon run                   # foreground; Ctrl-C or SIGTERM to stop (systemd/launchd)
arvolo daemon stop                  # shut it down — and keep it down
arvolo daemon status                # is it up, and what is it doing
arvolo daemon start --download-dir ~/Downloads/arvolo
```

`stop` leaves a marker so nothing respawns the daemon behind your back (the
desktop app supervises it and would otherwise bring it right back); the next
`start`/`run` — or the app's own restart button — clears it.

Downloads default to `~/Arvolo`. Change the folder with the
`--download-dir` flag, the `ARVOLO_DOWNLOAD_DIR` env var, or a `download_dir` key
in `~/.config/arvolo/config.toml` (flag > env > config > default) — handy for a
service, so accepted files land where you want without editing the unit:

```toml
# ~/.config/arvolo/config.toml
relay = "https://mailbox.example.com"
download_dir = "/srv/arvolo/incoming"
```

A second daemon refuses to start while one is already running
(single-instance guard).

Then, from any terminal (or a second machine's account):

```sh
arvolo status                 # is the daemon up? live transfers (→ out, ← in) + pending offers
arvolo status --watch         # redraw as things progress
arvolo history                # what already happened (the log)
arvolo recv <handle>          # download a parked offer (handle from `arvolo status`)
arvolo decline <handle>       # decline it
arvolo cancel <transfer-id>   # stop a running transfer
arvolo send <file> --to bob   # hand a send to the daemon (live if online, else mailbox)
arvolo send <file>            # .arvolo ticket file, served by the daemon in the background
arvolo send <file> --code     # short dictatable code, hosted by the daemon
arvolo listen                 # attach as an interactive approver (Ctrl-C detaches)
```

A bare `arvolo send <file>` hands its ticket to the daemon by default: it
serves in the background and you watch it (who's pulling, %, delivered) with
`arvolo status`, surviving your terminal. `--foreground`
keeps the inline behavior (serve here, Ctrl-C to stop).

`arvolo code <file>` does the same for a short code. It prints the code and
returns; the daemon holds the rendezvous open and serves the file behind it.
`arvolo status` shows the live code, so you can read it out again later — useful,
because the terminal that printed it is usually long gone:

```
  [3] → anonymous  holiday.zip  0/2.1 GB  (active)
        code: arvolo recv 4821-crater-mango@relay.example.com
```

The code **outlives a daemon restart**: it is re-attached to its rendezvous when
the daemon comes back, so one already written down keeps working. By default it
retires once its receiver has the file — the transfer carries on after that, since
the ticket is a separate capability and the download has only just started. Pass
`--keep` to serve everyone who has the code until you `arvolo cancel <id>`; that is
a bigger capability, so it is opt-in rather than the default. `--foreground` keeps
the old inline behaviour.

Three wrong-code attempts retire a code on the spot, and the sender says so — just
run `arvolo code` again for a new one. Hosting a background code needs a relay
running rendezvous v2; against an older one the command falls back to serving in
the foreground and tells you why.

**Resuming a download.** An interrupted receive leaves the partial file, its piece
bitfield, and the ticket it was fetched under. Finish it with the path, not the
code — a code is consumed on use, so it is not the way back:

```sh
arvolo resume ~/Arvolo/holiday.zip
```

With no daemon running, `send`/`listen` fall back to their in-process
behavior, so nothing you already do breaks.

## Trust: auto-download vs. ask

Every incoming offer **asks for approval by default** — it parks (visible in
`arvolo status`) and (on a desktop) raises a notification. Mark the senders you
trust to skip the prompt:

```sh
arvolo contacts add bob <bob-id>
arvolo contacts verify bob      # compare fingerprints out-of-band first
arvolo contacts trust bob       # now bob's files auto-download, no prompt
arvolo contacts trust bob --undo   # back to asking
```

`contacts list` shows `⬇trusted` next to trusted contacts. Trust is separate from
`verified` (verified = the key is really theirs; trusted = auto-accept). Trust is
keyed to the contact's id, so re-adding a contact under a **different** key clears
both marks — you re-verify before trusting the new key.

## Run at boot/login

Templates live in [`packaging/`](../packaging/):

- **macOS (launchd):** [`it.lords82.arvolo.plist`](../packaging/it.lords82.arvolo.plist)
  → `~/Library/LaunchAgents/`. Edit the binary path, `ARVOLO_RELAY`, and `HOME`,
  then `launchctl load ~/Library/LaunchAgents/it.lords82.arvolo.plist`.
- **Linux (systemd user unit):** [`arvolo.service`](../packaging/arvolo.service)
  → `~/.config/systemd/user/`. Edit `ExecStart` + `ARVOLO_RELAY`, then
  `systemctl --user enable --now arvolo` and `loginctl enable-linger "$USER"` so
  it keeps running after logout. On a headless server the desktop notification is
  a no-op — the offer still shows in `arvolo status` and the journal.

Both run the daemon in the **foreground** (the supervisor manages the process and
restarts it on failure); `stop` sends SIGTERM, which removes the socket cleanly.

See [`DEPLOY.md`](DEPLOY.md) for the server side (the `arvolo-relay` mailbox and
the iroh NAT relay).
