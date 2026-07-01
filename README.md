# Arvolo

Secure, cross-platform file sending. **P2P-first** when both devices are online;
**store-and-forward via a self-hostable, expiring relay** when the recipient is
away. Every transfer is **end-to-end encrypted** and the relay is **zero-knowledge**
(it only ever holds ciphertext).

> Status: **working CLI** (v0.1). P2P + relay-backfill transfer with resume,
> per-chunk E2E encryption, short human pairing codes, send-to-a-contact, folders,
> and an expiring zero-knowledge mailbox. Desktop GUI, browser link-mode and
> federation are planned (see Roadmap).

## Install

**One-liner** (Linux x86_64, macOS arm64):

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
arvolo send --code --relay https://relay.example.com ./photo.jpg
#   ->  4821-crater-mango@https://relay.example.com

# receiver
arvolo recv 4821-crater-mango@https://relay.example.com
```

With a configured default relay (see [Config](#config)) the code is just
`4821-crater-mango`. Plain `arvolo send ./file` (no `--code`) prints a
self-contained `arvc…` ticket instead — no relay needed at all.

**Offline mailbox** — recipient is away; encrypt to their identity and leave it
on a relay until they fetch it:

```sh
arvolo id                                             # recipient shows their public id
arvolo send-offline ./report.pdf --to <id-or-contact> # sender deposits (HPKE E2E)
arvolo recv-offline arvm…                             # recipient fetches + decrypts (burns on read)
```

## Commands

| Command | What it does |
|---|---|
| `arvolo send <paths…>` | Serve one or more files/folders P2P (multiple paths or a folder are packed into one archive). Prints an `arvc…` ticket. |
| &nbsp;&nbsp;`--code` | Show a short pairing code instead of the ticket (needs a relay). |
| &nbsp;&nbsp;`--relay <url>` | Rendezvous relay for `--code`; embedded in the code so the receiver needs no config. |
| &nbsp;&nbsp;`--to <name\|id>` | Encrypt so **only** this recipient can receive, and authenticate you as sender. |
| &nbsp;&nbsp;`--seed-relay <url>` | Also seed to a relay so the transfer finishes even if you go offline (lazy backfill). |
| &nbsp;&nbsp;`--qr` | Also render the ticket/code as a scannable QR. |
| `arvolo recv <ticket\|code> [-o out]` | Receive from an `arvc…` ticket **or** a pairing code; resumes if interrupted; unpacks folders. |
| `arvolo id` | Show your public id (created on first use). |
| `arvolo contacts add\|list\|remove` | Address book of recipients, used by `--to`. |
| `arvolo send-offline <file> --to <name\|id> [--relay --ttl --max --qr]` | Encrypt (HPKE) and deposit on a relay for an offline recipient. |
| `arvolo recv-offline <arvm…> [-o out]` | Fetch + decrypt an offline ticket. |

Run `arvolo <cmd> --help` for the full flag list.

## Config

`~/.config/arvolo/config.toml`:

```toml
relay = "https://relay.example.com"   # default relay for --code / recv <code> / send-offline
```

Contacts live in `~/.config/arvolo/contacts.toml` (managed via `arvolo contacts`).

**Environment variables** (override config where relevant):

| Var | Meaning |
|---|---|
| `ARVOLO_RELAY` | Default relay URL (wins over `config.toml`). |
| `ARVOLO_IDENTITY` | Path to your identity key (default `~/.config/arvolo/identity.key`). |
| `ARVOLO_CONFIG_DIR` | Override the config/contacts directory. |
| `ARVOLO_IROH_RELAY` | Self-hosted **iroh** NAT relay for P2P hole-punching (vs. n0's public relays). |

## How it works

- **P2P transport** over [iroh](https://www.iroh.computer/) QUIC (dial by key, not
  IP; automatic hole-punching with relay fallback).
- **Per-chunk E2E encryption**: files are split into 16 MiB chunks, each sealed
  with ChaCha20-Poly1305 under a per-transfer key; the content key travels only in
  the ticket/code. The sender encrypts **on the fly** and stores nothing — sending
  a file uses bounded memory and **no extra disk**, regardless of file size.
- **Zero-knowledge relay**: for lazy backfill (sender may go offline) or the
  offline mailbox, the relay holds only **ciphertext** addressed by BLAKE3 hash,
  and auto-deletes on release / TTL / burn-after-read.
- **Short-code pairing** (magic-wormhole style): a SPAKE2 PAKE over a relay
  rendezvous exchanges the ticket, so two short words are safe (no offline
  dictionary attack) and the relay never sees the ticket in the clear.
- **Resume**: interrupted receives resume — both across chunks and *within* a chunk.

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

Planned next: desktop GUI, browser link-mode (Firefox Send heir), relay
federation (short codes across independent relays), mobile. Post-MVP ideas are
tracked in [`docs/ROADMAP-FUTURE.md`](docs/ROADMAP-FUTURE.md).

## Licensing

Open-core. The core (client + single relay) is free software under
**[AGPL-3.0-only](LICENSE)** — self-host and modify it; the AGPL keeps it open
even when run as a network service. A separate **commercial license** is available
for proprietary/embedded use without the AGPL's obligations; business features
(federation, SSO, audit, managed hosting) are commercial.

The AGPL covers the **code**, not the **name**: "Arvolo" is a trademark of the
project owner and may not be used by forks in a way that implies endorsement.
See [`CONTRIBUTING.md`](CONTRIBUTING.md).
