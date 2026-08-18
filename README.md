# Arvolo

*Leggilo in [italiano](README.it.md) · Lisez-le en [français](README.fr.md) ·
Lies es auf [Deutsch](README.de.md).*

**Send files to anyone — end-to-end encrypted, no account, even if they're
offline.**

![The Arvolo desktop app](docs/assets/arvolo-app.png)

When both devices are online, files travel **peer-to-peer** — straight from one
machine to the other, never through a server. When the recipient is away, the
file waits **sealed** in a mailbox on a relay: a small server **you can
self-host**, which only ever stores ciphertext and cannot read a thing. And for
someone with nothing installed, a **link** downloads and decrypts the file in
any browser.

- **End-to-end encrypted, always** — keys never touch a server.
- **No account, no vendor in the middle** — your files move through
  infrastructure you control.
- **Reaches people who are offline** — sealed deposits wait for them, then burn
  after read.
- **App and command line, one engine** — macOS (signed and notarized), Windows
  and Linux.

## Try it in two minutes

**1. Get Arvolo.** Download the app from the
[latest release](https://github.com/lords82/arvolo/releases) — `.dmg` for
macOS, `.msi` for Windows, `.AppImage` for Linux — or install the command-line
client:

```sh
curl -fsSL https://raw.githubusercontent.com/lords82/arvolo/main/install.sh | sh
```

**2. Point it at a relay.** Arvolo has no central server — that is rather the
point — so it needs a relay: the one your company or a friend already runs, or
your own, up in one command:

```sh
docker run -d --name arvolo-relay -p 6282:6282 -v arvolo-data:/data \
  ghcr.io/lords82/arvolo-relay:latest
```

In the app, set it under **Settings → Network**; on the command line, the first
run asks and remembers.

**3. Send something.** In the app: drag a file into the window and share the
short code it gives you — the other side pastes it into their Arvolo and the
file lands. Same thing from the terminal:

```sh
# you
arvolo send --code ./photo.jpg
#   ->  4821-crater-mango

# them
arvolo recv 4821-crater-mango
```

A bare `arvolo send ./photo.jpg` writes a small `photo.jpg.arvolo` ticket file
instead — share it over any channel, like a .torrent, and the other side runs
`arvolo recv photo.jpg.arvolo`.

Nothing installed on the other side? `arvolo send --link ./report.pdf` prints a URL
that downloads and decrypts in any browser — no install, no account. This is
what whoever opens it sees:

![Opening an Arvolo link in a browser: the file decrypts right there — the
decryption key lives only in the link's #fragment, which browsers never send
to the server](docs/assets/arvolo-link-browser.png)

## Where to go next

| | |
|---|---|
| [The manual](docs/MANUAL.md) | Every command, every flag, every setting — and how it all works inside. |
| [Quickstart](docs/QUICKSTART.md) | Standing up a relay properly, LAN and behind nginx + TLS, end to end. |
| [Deploying](docs/DEPLOY.md) | Production self-hosting: systemd, Docker, abuse hardening. |
| [The desktop app](gui/README.md) | The GUI in detail, with its CLI parity table. |
| [The protocol](docs/PROTOCOL.md) | The wire format and every flow, for the curious and the auditing. |

## License

Open core: the client and the relay are free software under
[AGPL-3.0-only](LICENSE); a separate commercial license covers proprietary use
and business features. "Arvolo" is a trademark of the project owner — see
[CONTRIBUTING.md](CONTRIBUTING.md).
