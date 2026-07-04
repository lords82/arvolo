# Arvolo

Secure, cross-platform file sending. **P2P-first** when both devices are online;
**store-and-forward via a self-hostable, expiring relay** when the recipient is
away. Every transfer is **end-to-end encrypted** and the relay is **zero-knowledge**
(it only ever holds ciphertext).

**Your data never leaves your infrastructure — or the EU.** Files travel P2P
directly between devices; when a relay is needed it holds only ciphertext and you
**self-host it** (on your own servers, in your own datacenter, on EU soil). No
third-party cloud, no US provider, nothing to subpoena under the CLOUD Act. Unlike
WeTransfer, Dropbox, or other US SaaS, there is **no vendor in the middle** that
can read, retain, or be compelled to hand over your files — which makes GDPR data
residency and digital-sovereignty requirements straightforward to meet.

> Status: **working CLI** (v0.6). P2P + relay-backfill transfer with resume,
> per-chunk E2E encryption, short human pairing codes, send-to-a-contact, folders,
> and an expiring zero-knowledge mailbox. Plus an **always-open client**
> (`listen`/`push`) that receives pushed files with a live watchdog, and
> **browser download links** — share a URL that decrypts in any browser, no
> install. Desktop GUI, relay federation, and mobile are planned (see Roadmap).

## Install

**One-liner** (Linux x86_64/aarch64, macOS arm64):

```sh
curl -fsSL https://raw.githubusercontent.com/lords82/arvolo/main/install.sh | sh
```

Installs `arvolo` (and `arvolo-relay`) into `/usr/local/bin` (override with
`ARVOLO_INSTALL_DIR`). Pin a version with `ARVOLO_VERSION=vX.Y.Z`.

**Prebuilt binaries** — or grab `arvolo` (and `arvolo-relay`) for your OS from the
[latest release](https://github.com/lords82/arvolo/releases), unpack, and put it
on your `PATH`.

**From crates.io** (needs Rust ≥ 1.88):

```sh
cargo install arvolo-cli    # the `arvolo` client
cargo install arvolo-relay  # the relay (self-host)
```

**From git** (latest, unreleased):

```sh
cargo install --git https://github.com/lords82/arvolo arvolo-cli
cargo install --git https://github.com/lords82/arvolo arvolo-relay
```

**Relay via Docker** — self-host the zero-knowledge relay in one command:

```sh
docker run -d --name arvolo-relay -p 8787:8787 -v arvolo-data:/data \
  ghcr.io/lords82/arvolo-relay:latest
```

## Quickstart

**P2P, both online** — share a short code instead of a giant ticket:

```sh
# sender (relay is used only to bootstrap the code exchange, never for your data)
arvolo send --code --relay relay.example.com ./photo.jpg
#   ->  4821-crater-mango@relay.example.com

# receiver
arvolo recv 4821-crater-mango@relay.example.com
```

The relay address defaults to `https://` — just pass the host. With a configured
default relay (see [Config](#config)) the code is even shorter, just
`4821-crater-mango`. Plain `arvolo send ./file` (no `--code`) prints a
self-contained `arvc…` ticket instead — no relay needed at all.

Relay without TLS (LAN / dev)? Add `--use-http`:

```sh
arvolo send --code --relay relay.local:8787 --use-http ./photo.jpg
```

**Offline mailbox** — recipient is away; encrypt to their identity and leave it
on a relay until they fetch it:

```sh
arvolo id                                             # recipient shows their public id
arvolo send-offline ./report.pdf --to <id-or-contact> # sender deposits (HPKE E2E)
arvolo recv arvm…                                     # recipient fetches + decrypts (burns on read)
```

**Always-open client** — stay online and receive files contacts push to you; each
incoming offer shows sender, name, and size, and accepted transfers download
transparently (no ticket to copy):

```sh
# receiver: stay online (auto-accept files from saved contacts)
arvolo listen --auto-accept-contacts

# sender: push straight to a contact — live P2P if they're online, else it lands
# in the mailbox and is delivered when they return
arvolo push ./photo.jpg --to alice
```

**Browser download link** — share a URL anyone can open in a browser to download
and decrypt the file, **no arvolo install and no account**. The key lives only in
the link's `#fragment`, so the relay stays zero-knowledge:

```sh
arvolo send-offline ./report.pdf --link
#   ->  https://relay.example.com/dl/<claim>#<key>

# The link is tracked as a local session; cancel it (and delete the file from the
# relay) anytime:
arvolo sessions list
arvolo sessions rm <id>
```

## Commands

| Command | What it does |
|---|---|
| `arvolo send <paths…>` | Serve one or more files/folders P2P (multiple paths or a folder are packed into one archive). Prints an `arvc…` ticket. |
| &nbsp;&nbsp;`--code` | Show a short pairing code instead of the ticket (needs a relay). |
| &nbsp;&nbsp;`--relay <host>` | Rendezvous relay for `--code`; embedded in the code so the receiver needs no config. `https://` is assumed for a bare host. |
| &nbsp;&nbsp;`--use-http` | Treat bare relay hosts as `http://` instead of `https://` (LAN / dev). An explicit scheme is always kept. |
| &nbsp;&nbsp;`--to <name\|id>` | Encrypt so **only** this recipient can receive, and authenticate you as sender. |
| &nbsp;&nbsp;`--seed-relay <host>` | Also seed to a relay so the transfer finishes even if you go offline (lazy backfill). |
| &nbsp;&nbsp;`--qr` | Also render the ticket/code as a scannable QR. |
| `arvolo recv <ticket\|code> [-o out] [--password]` | Receive from **any** ticket or code — auto-detects: `arvc…`/pairing-code fetch live P2P (resumes, unpacks folders), `arvm…`/download-link decrypt from the relay. |
| `arvolo id` | Show your public id (created on first use). |
| `arvolo contacts add\|list\|verify\|remove\|trust\|untrust` | Address book of recipients (used by `--to`); TOFU + out-of-band fingerprint verification. `trust` lets the daemon auto-download that contact's files (default: ask). |
| `arvolo send-offline <file> --to <name\|id> [--relay --ttl --max --password --qr]` | Encrypt (HPKE) and deposit on a relay for an offline recipient. |
| &nbsp;&nbsp;`--link` | Instead, produce a **browser download link** (public capability, decrypts client-side). `--to` is not used; **no download cap** by default. |
| `arvolo listen [--download-dir --auto-accept-contacts --auto-accept-verified]` | Stay online and receive files contacts push to you (offers, live watchdog, transparent download). |
| `arvolo push <paths…> --to <name\|id>` | Push to a contact: live P2P if online, else deposited to the mailbox and delivered on their return. |
| `arvolo sessions list\|rm <id>` | List relay deposits (link / sealed) with live relay status + resumable sends; `rm` **revokes on the relay**, deleting the file/link. |
| `arvolo revoke <arvm…> --token <t>` / `revoke-link <url> --token <t>` | Delete a deposited ticket / download link from the relay. |
| `arvolo transfers [--watch]` / `transfers clear` | One view of everything: with a daemon, live in/out transfers + pending offers, then history below (`--watch` redraws); without one, just history. `clear` wipes history. |
| `arvolo daemon [--download-dir --relay]` | Run the always-on background engine + local control socket (systemd/launchd). See [`docs/DAEMON.md`](docs/DAEMON.md). |
| `arvolo accept <id>` / `reject <id>` / `cancel <id>` | Drive the running daemon: approve/decline a parked offer, cancel a transfer (ids from `arvolo transfers`). |

Run `arvolo <cmd> --help` for the full flag list.

## Config

`~/.config/arvolo/config.toml`:

```toml
relay = "relay.example.com"   # default relay for --code / recv <code> / send-offline (https assumed)
download_dir = "/srv/arvolo/incoming"   # where the daemon saves accepted files (default: <config>/downloads)
```

For a relay without TLS, write the scheme explicitly: `relay = "http://relay.local:8787"`.

Contacts live in `~/.config/arvolo/contacts.toml` (managed via `arvolo contacts`).

**Environment variables** (override config where relevant):

| Var | Meaning |
|---|---|
| `ARVOLO_RELAY` | Default relay URL (wins over `config.toml`). |
| `ARVOLO_DOWNLOAD_DIR` | Where the daemon saves accepted files (wins over `config.toml`). |
| `ARVOLO_IDENTITY` | Path to your identity key (default `~/.config/arvolo/identity.key`). |
| `ARVOLO_CONFIG_DIR` | Override the config/contacts directory. |
| `ARVOLO_IROH_RELAY` | Self-hosted **iroh** NAT relay for P2P hole-punching (vs. n0's public relays). |

## How it works

- **P2P transport** over [iroh](https://www.iroh.computer/) QUIC (dial by key, not
  IP; automatic hole-punching with relay fallback).
- **Per-chunk E2E encryption**: files are split into 16 MiB chunks, each sealed
  with AES-256-GCM under a per-transfer key; the content key travels only in the
  ticket/code. The sender encrypts **on the fly** and stores nothing — sending a
  file uses bounded memory and **no extra disk**, regardless of file size.
- **One AEAD everywhere**: AES-256-GCM is used for HPKE, the chunk stream, and the
  password wrap — the same cipher the browser decrypts natively via WebCrypto for
  download links. Equivalent strength to ChaCha20-Poly1305 and hardware-accelerated
  (AES-NI); the nonce discipline guarantees a `(key, nonce)` pair is never reused.
- **Zero-knowledge relay**: for lazy backfill (sender may go offline) or the
  offline mailbox, the relay holds only **ciphertext** addressed by BLAKE3 hash,
  and auto-deletes on release / TTL / burn-after-read.
- **Short-code pairing** (magic-wormhole style): a SPAKE2 PAKE over a relay
  rendezvous exchanges the ticket, so two short words are safe (no offline
  dictionary attack) and the relay never sees the ticket in the clear.
- **Always-open client**: `listen` keeps a client online; senders `push` offers
  through a zero-knowledge inbox (proof-of-possession session auth) and presence
  beacons, with a two-phase watchdog that delivers **live P2P** when the recipient
  is online and falls back to the **mailbox** when they aren't.
- **Browser download links**: `send-offline --link` deposits a chunked AES-256-GCM
  container; the relay serves a self-contained page that fetches the ciphertext and
  decrypts it in the browser (key only in the URL `#fragment`), streaming to disk
  without buffering the whole file. Each link is a local **session** whose removal
  revokes the blob on the relay.
- **Resume**: interrupted receives resume — both across chunks and *within* a chunk.

For the full wire protocol (ticket formats, relay HTTP API, and every flow), see
[`docs/PROTOCOL.md`](docs/PROTOCOL.md); for the security rationale, see
[`docs/TECHNICAL-OVERVIEW.md`](docs/TECHNICAL-OVERVIEW.md).

**Self-host everything** (production, no third party): run `arvolo-relay` and your
own iroh relay on a VPS, point clients with `ARVOLO_IROH_RELAY` and a configured
`relay`. See [`docs/DEPLOY.md`](docs/DEPLOY.md) (`relay/` ships a Dockerfile +
`docker-compose.yml`).

## Workspace layout

| Crate | Path | Role |
|-------|------|------|
| `arvolo-core` | [`core/`](core/) | Engine: transport, chunk protocol, crypto, flows. |
| `arvolo-cli` (`arvolo`) | [`cli/`](cli/) | Command-line client. |
| `arvolo-relay` | [`relay/`](relay/) | Self-hostable zero-knowledge relay / mailbox. |

Build & test: `cargo build && cargo test`.

## Roadmap

Shipped recently: the always-open client (`listen`/`push`) and browser download
links (the Firefox Send heir). Planned next: desktop GUI, relay federation (short
codes across independent relays), and mobile. Post-MVP ideas are tracked in
[`docs/ROADMAP-FUTURE.md`](docs/ROADMAP-FUTURE.md).

## Licensing

Open-core. The core (client + single relay) is free software under
**[AGPL-3.0-only](LICENSE)** — self-host and modify it; the AGPL keeps it open
even when run as a network service. A separate **commercial license** is available
for proprietary/embedded use without the AGPL's obligations; business features
(federation, SSO, audit, managed hosting) are commercial.

The AGPL covers the **code**, not the **name**: "Arvolo" is a trademark of the
project owner and may not be used by forks in a way that implies endorsement.
See [`CONTRIBUTING.md`](CONTRIBUTING.md).
