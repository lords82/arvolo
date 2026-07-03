# The arvolo daemon (always-on receive + local control)

By default `arvolo` is one-shot: `send`/`recv` move a single payload and exit.
The **daemon** turns the engine into a persistent background service so you can:

- **stay online and receive** files without keeping a terminal open, surviving
  logout/reboot when run under systemd/launchd;
- **queue several sends** at once — `arvolo push` hands off to the daemon and the
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
arvolo daemon                       # foreground; Ctrl-C or SIGTERM to stop
arvolo daemon --download-dir ~/Downloads/arvolo
```

Downloads default to `~/.config/arvolo/downloads`. A second `arvolo daemon`
refuses to start while one is already running (single-instance guard).

Then, from any terminal (or a second machine's account):

```sh
arvolo status                 # live transfers (→ out, ← in) + pending offers
arvolo status --watch         # redraw as things progress
arvolo pending                # offers waiting for your approval
arvolo accept <offer-id>      # download a parked offer
arvolo reject <offer-id>      # decline it
arvolo cancel <transfer-id>   # stop a running transfer
arvolo push <file> --to bob   # hand a send to the daemon (concurrent)
arvolo listen                 # attach as an interactive approver (Ctrl-C detaches)
```

With no daemon running, `push`/`listen` fall back to their old in-process
behavior, so nothing you already do breaks.

## Trust: auto-download vs. ask

Every incoming offer **asks for approval by default** — it parks in
`arvolo pending` and (on a desktop) raises a notification. Mark the senders you
trust to skip the prompt:

```sh
arvolo contacts add bob <bob-id>
arvolo contacts verify bob      # compare fingerprints out-of-band first
arvolo contacts trust bob       # now bob's files auto-download, no prompt
arvolo contacts untrust bob     # back to asking
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
  a no-op — the offer still lands in `arvolo pending` and the journal.

Both run the daemon in the **foreground** (the supervisor manages the process and
restarts it on failure); `stop` sends SIGTERM, which removes the socket cleanly.

See [`DEPLOY.md`](DEPLOY.md) for the server side (the `arvolo-relay` mailbox and
the iroh NAT relay).
