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
docker run -d --name arvolo-relay -p 6282:6282 -v arvolo-data:/data \
  ghcr.io/lords82/arvolo-relay:latest
```

## Quickstart

> New here? [`docs/QUICKSTART.md`](docs/QUICKSTART.md) walks through standing up a
> relay (LAN/dev **and** behind nginx+TLS) and the minimal client config end to end.

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
arvolo send --code --relay relay.local:6282 --use-http ./photo.jpg
```

**Offline mailbox** — recipient is away; encrypt to their identity and leave it
on a relay until they fetch it:

```sh
arvolo id                                          # recipient shows their public id
arvolo send ./report.pdf --to <id-or-contact> --ticket  # deposit (HPKE E2E) + print an arvm ticket
arvolo recv arvm…                                  # recipient fetches + decrypts (burns on read)
```

**Send to a known recipient** — `send --to` picks the channel automatically: if
they're online it's delivered live to their daemon; if offline it's deposited on
the relay (mailbox) and an `arvm…` ticket is printed so you can also hand it over.
Each incoming offer shows sender, name, and size; accepted transfers download
transparently (no ticket to copy):

```sh
# receiver: stay online (auto-accept files from saved contacts)
arvolo listen --auto-accept-contacts

# sender: online → live; offline → mailbox + a shareable arvm ticket
arvolo send ./photo.jpg --to alice
```

**Browser download link** — share a URL anyone can open in a browser to download
and decrypt the file, **no arvolo install and no account**. The key lives only in
the link's `#fragment`, so the relay stays zero-knowledge:

```sh
arvolo send ./report.pdf --link
#   ->  https://relay.example.com/dl/<claim>#<key>

# The link is tracked as a local session; cancel it (and delete the file from the
# relay) anytime:
arvolo sessions list
arvolo sessions rm <id>
```

## Commands

| Command | What it does |
|---|---|
| `arvolo send <paths…>` | Get files/folders to someone — the tool picks the channel (multiple paths / a folder are packed into one archive). Without `--to`: prints an `arvc…` P2P ticket to share; if a daemon is running it serves it in the **background** (track with `arvolo transfers`). |
| &nbsp;&nbsp;`--to <name\|id>` | Send to a known recipient: **online** → delivered live to their daemon; **offline** → deposited on the relay (mailbox) + an `arvm…` ticket printed so you can also hand it over. |
| &nbsp;&nbsp;`--ticket` | With `--to`: force the mailbox/`arvm…` path even if they're online (send-and-forget). |
| &nbsp;&nbsp;`--link` | Produce a public **browser download link** instead (decrypts client-side; `--to` not used; no download cap by default). |
| &nbsp;&nbsp;`--code` | Show a short pairing code instead of the P2P ticket (no `--to`; needs a relay). |
| &nbsp;&nbsp;`--ttl --max --password` | Mailbox/link tuning: expiry, download cap, E2E password. |
| &nbsp;&nbsp;`--seed-relay <host>` | P2P ticket send: also seed to a relay so it finishes even if you go offline (lazy backfill). |
| &nbsp;&nbsp;`--foreground` | Serve the P2P ticket in **this terminal** (blocking, Ctrl-C to stop) instead of the daemon. |
| &nbsp;&nbsp;`--relay --use-http --qr` | Relay for `--code`/mailbox/link; `http://` for bare hosts (LAN/dev); render the ticket/code/link as a QR. |
| `arvolo recv <ticket\|code> [-o out] [--password]` | Receive from **any** ticket or code — auto-detects: `arvc…`/pairing-code fetch live P2P (resumes, unpacks folders), `arvm…`/download-link decrypt from the relay. |
| `arvolo id` | Show your public id (created on first use). |
| `arvolo version` | CLI version + whether the daemon is running (and its version). |
| `arvolo contacts add\|list\|verify\|remove\|trust\|untrust` | Address book of recipients (used by `--to`); TOFU + out-of-band fingerprint verification. `trust` lets the daemon auto-download that contact's files (default: ask). |
| `arvolo listen [--download-dir --auto-accept-contacts --auto-accept-verified]` | Stay online and receive files contacts send to you (offers, live watchdog, transparent download). |
| `arvolo sessions list\|rm <id>` | List relay deposits (link / sealed) with live relay status + resumable sends; `rm` **revokes on the relay**, deleting the file/link. |
| `arvolo revoke <arvm…\|link> --token <t>` | Delete a deposited mailbox ticket **or** a browser download link from the relay (auto-detects which). |
| `arvolo transfers [--watch]` / `transfers clear` | One view of everything: with a daemon, live in/out transfers + pending offers, then history below (`--watch` redraws); without one, just history. `clear` wipes history. |
| `arvolo daemon [--download-dir --relay]` | Run the always-on background engine + local control socket (systemd/launchd). See [`docs/DAEMON.md`](docs/DAEMON.md). |
| `arvolo accept <id>` / `reject <id>` / `cancel <id>` | Drive the running daemon: approve/decline a parked offer, cancel a transfer (ids from `arvolo transfers`). |

Run `arvolo <cmd> --help` for the full flag list.

## Config

`~/.config/arvolo/config.toml`:

```toml
relay = "relay.example.com"   # default relay for --code / recv <code> / send --to (https assumed)
download_dir = "/srv/arvolo/incoming"   # where accepted files are saved (default: ~/Arvolo)
```

For a relay without TLS, write the scheme explicitly: `relay = "http://relay.local:6282"`.

Contacts live in `~/.config/arvolo/contacts.toml` (managed via `arvolo contacts`).

### Environment variables

**Required** — everything else has a sane default, but this must be set (env or
`config.toml`) or the client cannot pair codes, send `--to`, use the mailbox/links,
or join the swarm:

| Var | Meaning |
|---|---|
| `ARVOLO_RELAY` | Relay URL. Mandatory unless `relay` is set in `config.toml`. A bare host assumes `https://`; write `http://host:6282` for plaintext. |

Everything below is **optional** (defaults shown).

**Client** (`arvolo` / daemon):

| Var | Default | Meaning |
|---|---|---|
| `ARVOLO_DOWNLOAD_DIR` | `~/Arvolo` | Where accepted files are saved (wins over `config.toml`). |
| `ARVOLO_TEMP_DIR` | `<config>/tmp` | Scratch dir for staged tars (folder sends, archive receives) — kept off the download dir and off a small system tmpfs. |
| `ARVOLO_CONFIG_DIR` | `~/.config/arvolo` | Config/contacts/identity/resume directory. |
| `ARVOLO_IDENTITY` | `<config>/identity.key` | Path to your identity key. |
| `ARVOLO_SEED` | `1` (on) | Keep seeding a completed file into the swarm. Set `0`/`false`/`no`/`off` to opt out. |
| `ARVOLO_SEED_AFTER` | `0` (off) | Seconds to keep backfilling the relay after a transfer completes. |
| `ARVOLO_SWARM` | on | BitTorrent-style swarm for shared `arvc…` tickets. Set `off`/`0`/`relay-only` to disable (privacy escape hatch). |
| `ARVOLO_CONCURRENCY` | `4` | Parallel chunk fetches (clamped to 1–16). |
| `ARVOLO_IPV4_ONLY` | auto | `1` forces IPv4-only (auto-detected when there's no IPv6 route). |
| `ARVOLO_IROH_RELAY` | n0 public | Self-hosted **iroh** NAT relay for P2P hole-punching. |
| `ARVOLO_MAX_FETCH_BYTES` | 512 MiB | Max size a single download link/blob will fetch. |
| `ARVOLO_DEBUG` | off | Extra diagnostics. |
| `RUST_LOG` | `info` | `tracing` log level. |

**Relay** (`arvolo-relay` — all optional; the server runs with defaults):

| Var | Default | Meaning |
|---|---|---|
| `ARVOLO_RELAY_ADDR` | `0.0.0.0:6282` | Listen address. |
| `ARVOLO_RELAY_DB` | `arvolo-relay.db` | Mailbox database path. |
| `ARVOLO_RELAY_BLOBS` | `arvolo-blobs` | Blob directory. |
| `ARVOLO_RELAY_BLOBSTORE` | `arvolo-blobstore` | Blobstore directory. |
| `ARVOLO_DISABLE_LINKS` | off | `1`/`true`/`yes`/`on` disables browser download links. |
| `ARVOLO_MAX_BLOB_BYTES` | 256 MiB | Max deposited-blob size. |
| `ARVOLO_MAX_TTL` | 30 days | Max mailbox/blob TTL (seconds). |
| `ARVOLO_SEED_TTL` | 24 h | TTL for seeded chunks not yet released (seconds). |
| `ARVOLO_MAX_ENTRIES` | 100 000 | Global row cap. |
| `ARVOLO_MAX_INBOX_ROWS` / `ARVOLO_MAX_PRESENCE_ROWS` / `ARVOLO_MAX_RZ_ROWS` / `ARVOLO_MAX_SEEDED_ROWS` | per-table | Per-table row caps. |

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
- **Browser download links**: `send --link` deposits a chunked AES-256-GCM
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
