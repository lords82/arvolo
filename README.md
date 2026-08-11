# Arvolo

Secure, cross-platform file sending that reaches people **even when they're
offline**. When both devices are up, files travel P2P directly between them — not
through a server. When the recipient is away, Arvolo encrypts to their identity and
**leaves it in an expiring mailbox** until they fetch it; or you hand anyone a
**browser download link that decrypts with no install and no account**. Every
transfer is end-to-end encrypted, and the relay only ever holds ciphertext (a
zero-knowledge store) — which you self-host.

**Your data never leaves infrastructure you control.** Files travel P2P directly
between devices; when a relay is needed it holds only ciphertext and you
**self-host it** — on your own servers, in your own datacenter, inside your own
borders. There is **no vendor in the middle** that can read, retain, or be
compelled to hand over your files, and no foreign provider whose jurisdiction,
outages or pricing you inherit. That makes Arvolo a fit for companies moving
sensitive files between sites, for secure and tightly-segmented environments where
a public cloud is not an option, and for regions with data-residency rules or a
digital-sovereignty mandate — anywhere data has to move without depending on
another country's infrastructure.

> Status: **working CLI + desktop app** (v0.9.4). P2P + relay-backfill transfer
> with resume, per-chunk E2E encryption, short human pairing codes,
> send-to-a-contact (delivered live when they're online, **held and retried** when
> the relay can't take it — never silently failed), a **chunked, streamed offline
> mailbox** (big files aren't buffered in RAM), folders, and burn-after-read
> pickup. Plus an **always-open client** (`listen`/`daemon`) that receives sent
> files with a live watchdog, **browser download links** — share a URL that
> decrypts in any browser, no install — **multi-device** support on one shared
> identity, and a **desktop GUI** (Tauri 2, macOS/Linux) that drives the same
> daemon. Relay federation, mobile and Windows are planned (see Roadmap).

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

**Desktop app** — built from source for now (no prebuilt bundle yet), needs Node
≥ 18 and the platform WebView:

```sh
cargo build -p arvolo-cli      # the daemon the app drives
cd gui && npm install && npm run tauri build   # -> src-tauri/target/release/bundle/
```

## Desktop app

A cross-platform GUI (Tauri 2 + React) that runs **alongside** the CLI rather than
replacing it: it holds no second transfer engine, it is a thin client of the same
background daemon, over the local IPC socket. Closing the window leaves transfers
running, and both frontends share one engine and one identity.

The app is **two verbs and six places** — Send / Receive above Transfers, People,
Links and deposits, History, Your devices and Settings — all reachable from a
`⌘K` palette, with drag-and-drop, a system tray, native notifications for
incoming offers, light/dark themes, and the interface in **English, Italian,
French and German**. It covers the CLI's surface (see the parity table in
[`gui/README.md`](gui/README.md)); QR *scanning* and disk-session send recovery
stay CLI-only.

**macOS and Linux** are supported; Windows is deferred until `arvolo-ipc` grows a
named-pipe transport.

## Quickstart

> New here? [`docs/QUICKSTART.md`](docs/QUICKSTART.md) walks through standing up a
> relay (LAN/dev **and** behind nginx+TLS) and the minimal client config end to end.

**P2P, both online** — share a short code instead of a giant ticket:

```sh
# sender (relay is used only to bootstrap the code exchange, never for your data)
arvolo code --relay relay.example.com ./photo.jpg
#   ->  4821-crater-mango@relay.example.com

# receiver
arvolo recv 4821-crater-mango@relay.example.com
```

The relay address defaults to `https://` — just pass the host. With a configured
default relay (see [Config](#config)) the code is even shorter, just
`4821-crater-mango`. `arvolo ticket ./file` prints a
self-contained `arvc…` ticket instead — no relay needed at all.

With a daemon running, `arvolo code` returns straight away and the daemon holds the
code open for you — through this terminal closing and through the daemon itself
restarting. `arvolo status` shows it again when you need to read it out, and
`arvolo cancel <id>` retires it.

> **No trusted relay needed for P2P.** A pure `arvc` transfer touches no relay at
> all; with `arvolo code`, the relay is only a SPAKE2 rendezvous — it can't read your data
> or MITM the exchange without the code, only deny service. (The one path where a
> relay must be trusted is `arvolo link`, whose browser decryptor the relay serves.)

Relay without TLS (LAN / dev)? Add `--use-http`:

```sh
arvolo code --relay relay.local:6282 --use-http ./photo.jpg
```

**Offline mailbox** — recipient is away; encrypt to their identity and leave it
on a relay until they fetch it:

```sh
arvolo me                                     # recipient shows their public id
arvolo send <id-or-contact> ./report.pdf --deposit   # deposit (HPKE E2E) + print an arvm ticket
arvolo recv arvm…                             # recipient fetches + decrypts (burns on read)
arvolo recv                                   # …or, with no ticket to hand: what's waiting for you
```

**Send to a known recipient** — `send` picks the channel automatically: if
they're online it's delivered live to their daemon; if offline it's deposited on
the relay (mailbox) and an `arvm…` ticket is printed so you can also hand it over.
Each incoming offer shows sender, name, and size; accepted transfers download
transparently (no ticket to copy):

```sh
# receiver: stay online (auto-accept files from saved contacts)
arvolo listen --auto-accept-contacts

# sender: online → live; offline → mailbox + a shareable arvm ticket
arvolo send alice ./photo.jpg
```

**Browser download link** — share a URL anyone can open in a browser to download
and decrypt the file, **no arvolo install and no account**. The key lives only in
the link's `#fragment`, so an **honest** relay stays zero-knowledge. Note: the
browser path runs `dl.js` *served by the relay*, so a **hostile** relay operator
could serve modified code that grabs the key (like Firefox Send) — for
confidentiality against an untrusted relay use a recipient-sealed `arvolo send`, or
only link against a relay you host/trust. See [PROTOCOL.md §7.6](docs/PROTOCOL.md):

```sh
arvolo link ./report.pdf
#   ->  https://relay.example.com/dl/<claim>#<key>

# The link is listed among everything you've left on a relay; cancel it (and
# delete the file from the relay) anytime:
arvolo status
arvolo cancel <id>
```

## Commands

Sending splits on one question: **do you know who gets this?**

If you do, `send` delivers to them. If you don't, you ask for the artefact you
want to hand around yourself — and the command is its name. Multiple paths or a
folder are packed into one archive in every case.

| Command | What it does |
|---|---|
| `arvolo send <who> <paths…>` | Deliver to a saved contact or a public id: **online** → live to their daemon; **offline** → deposited on the relay (mailbox) + an `arvm…` ticket printed so you can also hand it over. |
| &nbsp;&nbsp;`--deposit` | Don't try a live delivery — deposit even if they're online (send-and-forget). |
| &nbsp;&nbsp;`-m/--note "…"` | A short note delivered *with* the file, sealed inside the offer (the relay never sees it). |
| `arvolo link <paths…>` | A public **browser download link**: whoever opens it needs no arvolo and no account (decrypts client-side; no download cap by default). |
| `arvolo code <paths…>` | A short pairing code like `4821-crater-mango` you can read out loud. Needs a relay for the rendezvous; the file still travels P2P. With a daemon running it's hosted in the **background** — the command returns, `arvolo status` shows the code, and it survives a daemon restart. |
| &nbsp;&nbsp;`--keep` | Serve everyone who has the code until you `arvolo cancel` it, instead of retiring it after the first receiver. |
| `arvolo ticket <paths…>` | A self-contained `arvc…` P2P ticket to paste into a chat — no relay needed at all. With a daemon running it's served in the **background** (track with `arvolo status`). |
| &nbsp;&nbsp;`--foreground` | Serve it in **this terminal** instead (blocking, Ctrl-C to stop). Applies to `ticket` and `code` alike. |
| &nbsp;&nbsp;`--ttl --max --password` | Deposit/link tuning: expiry, download cap, E2E password (`send`/`link`). |
| &nbsp;&nbsp;`--relay --use-http --qr` | Relay to use; `http://` for bare hosts (LAN/dev); render the ticket/code/link as a QR. |
| `arvolo recv <ticket\|code\|link> [-o out] [--password]` | Receive from **any** of them — one verb, auto-detected: `arvc…`/pairing-code fetch live P2P (resumes, unpacks folders), `arvm…`/download-link decrypt from the relay. |
| `arvolo recv` (no ticket) | What's **waiting for you**: the sends addressed to your identity, still sealed on the relay — sender, file, size, note — and you pick one, or `d<n>` to refuse it outright. With a daemon it lists the offers it has parked instead. A code, ticket or link never appears here: it *is* the permission to fetch, so nothing on the relay knows one is yours (which is what stops anyone enumerating someone else's) — paste those. |
| `arvolo status [--watch]` | Everything you can still **act on**: with a daemon, live in/out transfers + the offers it parked; without one, the offers read straight from your inbox on the relay (nobody else is watching it). Then either way what you've **left on a relay** (links / sealed deposits, saying whether it merely *arrived* on a device or the recipient actually *took* it) and the **resumable** sends (`--watch` redraws). Take an offer with `arvolo recv`. |
| &nbsp;&nbsp;*shares* | A ticket, a code, or the seeding a finished download turns into is listed as an ongoing **share**, not a transfer — no progress bar, because it isn't progress towards anything. Each carries copies taken, who's downloading now, last pickup and bytes uploaded; aggregates only, since an anonymous ticket carries no identity (copies, not people). |
| &nbsp;&nbsp;`clear` | Closes out what's over — drops completed/cancelled/failed rows, keeping anything still going (a mailbox send awaiting pickup looks done but isn't). Never touches the relay: withdraw with `arvolo cancel <id>`. |
| `arvolo history [--all]` | What already **happened**: the log of finished transfers, 20 most recent by default. Read-only — nothing here can still be acted on, which is what separates it from `status`. |
| &nbsp;&nbsp;`clear` | Forget the log. Leaves the live list, your relay deposits and your resumable sends alone. |
| `arvolo cancel <id>` | Take back anything `status` lists: a running transfer (a number), a file left on a relay (**deleted from the relay**, not just locally), or a resumable send. |
| &nbsp;&nbsp;`<arvm…\|link> --token <t>` | Withdraw something you sent from **another machine**, where there's no local record to hold the token. From the sending machine the id alone is enough. |
| `arvolo pause <id>` / `resume <id>` | Hold a running transfer and restart it. `resume` also replays an interrupted send — by session id, or by the `arvc…` ticket you shared plus its file, so the ticket you handed out keeps working — and finishes an interrupted **download** when given the path to its partial (a pairing code is spent on use, so the path is the way back, not the code). |
| `arvolo listen [--download-dir --auto-accept-contacts --auto-accept-verified]` | Stay reachable **for this session**, deciding offer by offer (Ctrl-C ends it). Attaches to a running daemon as its approver rather than starting a second engine. |
| `arvolo daemon [--download-dir --relay]` | Stay reachable **always**, as a background service: nobody is at the keyboard, so it decides from your trust settings and notifies you about the rest. Also the local control socket every other command drives. See [`docs/DAEMON.md`](docs/DAEMON.md). |
| `arvolo accept <id>` / `reject <id>` | Approve or decline a parked offer (ids from `arvolo status`; needs a running daemon). |
| `arvolo contacts pair [code]` | **Trade public ids over a short code** — show one, or type theirs. Both sides end up saved *and* verified: the code is a SPAKE2 secret, so the channel only forms between two people who both know it, which is what authenticates the key. `--qr` to show it scannable. |
| `arvolo contacts list [--json --no-presence]` | The book, with who's online. Probes run concurrently; a relay that doesn't answer reports **unknown**, never "offline". |
| `arvolo contacts add\|remove\|rename` | `rename` keeps the verified and trusted marks — doing it as remove + add drops them. |
| `arvolo contacts verify\|unverify\|trust\|untrust` | TOFU + out-of-band fingerprint verification; each takes a contact name **or** a raw id. `trust` lets the daemon auto-download that contact's files (default: ask) and refuses an unverified key unless `--force`. |
| `arvolo contacts block\|unblock [who]` | Drop someone's offers on arrival — no prompt, no notification. Syncs to your other devices. No argument lists who is blocked. |
| `arvolo contacts accept-name` | Approve a sender's advertised display name — first use pins it, a later change is quarantined (old name kept) until you approve. |
| `arvolo contacts export\|import` | Move a book between machines you don't want sharing an identity. Verified/trusted marks are **not** imported without `--with-marks`. |
| `arvolo contacts prune` | Drop advertised-name records left behind by removed contacts. |
| `arvolo device pair\|join\|sync\|status` | Use arvolo on more than one device: `pair` shows a code, `join` takes it (sharing one identity), `sync` propagates the address book, `status` reports fingerprint and last sync. See [Multiple devices](#multiple-devices). |
| `arvolo me` | Your public id (on stdout, so it pipes), fingerprint and display name. |
| &nbsp;&nbsp;`name ["…"]` | Show or set your display name — the self-chosen name advertised to recipients inside each sealed offer (a petname claim, never a verified identity; empty clears it). |
| `arvolo completions <shell>` | Shell integration for `<TAB>`. See [Completion](#tab-completion). |

Add `-v` to any command to have it narrate what it's doing (`-vv`, `-vvv` for
more). Run `arvolo <cmd> --help` for the full flag list.

## Multiple devices

Pairing shares **one identity** across your devices, so contacts see a single id
and any device can open what was sent to you. It carries your address book along,
and keeps it in step afterwards.

```sh
# on a device you already use
arvolo device pair
#   ->  arvolo device join 4821-crater-mango

# on the new device — this REPLACES its identity with the shared one
arvolo device join 4821-crater-mango

# later, after adding a contact on one of them
arvolo device sync        # on each device; `listen`/`daemon` also sync automatically
arvolo device status      # fingerprint, contact count, last sync
```

## TAB completion

Completion is computed by arvolo itself rather than baked into a static script,
so `<TAB>` offers **your contact names** after `arvolo send` and the **live ids**
from `arvolo status` after `cancel`, `resume`, `pause`, `accept` and `reject` —
not just the command names.

```sh
arvolo completions zsh  > ~/.zfunc/_arvolo      # and put ~/.zfunc on your fpath
arvolo completions bash > ~/.local/share/bash-completion/completions/arvolo
arvolo completions fish > ~/.config/fish/completions/arvolo.fish
```

Also available for `elvish` and `powershell`. Re-run it after upgrading arvolo:
the shell side and the binary are versioned together. Completion never touches
the network, and falls back to what's on disk if the daemon isn't answering.

## Config

**First run walks you through it.** With no config yet, the first interactive
`arvolo` command asks for your relay and writes `~/.config/arvolo/config.toml`
with every other option listed commented at its default (skipped when
non-interactive; `ARVOLO_NO_WIZARD=1` to disable). See
[`docs/QUICKSTART.md`](docs/QUICKSTART.md).

`~/.config/arvolo/config.toml`:

```toml
relay = "relay.example.com"   # default relay for code / link / send / recv (https assumed)
download_dir = "/srv/arvolo/incoming"   # where accepted files are saved (default: ~/Arvolo)
```

For a relay without TLS, write the scheme explicitly: `relay = "http://relay.local:6282"`.

Every environment variable in the client table below has a matching `config.toml`
key (same name, lowercased without the `ARVOLO_` prefix — e.g. `ARVOLO_SEED` →
`seed`); the env var wins when both are set. Two keys are config-only: `sync`
(keep the address book in step across your linked devices, on by default) and
`display_name` (set it with `arvolo name "…"`). Contacts live in
`~/.config/arvolo/contacts.toml` (managed via `arvolo contacts`).

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
| `ARVOLO_SHARE_DAYS` | unset | Stop a share (ticket, code, or the seeding a finished download becomes) after N days. Unset = it lasts until you stop it by hand, so every file you receive leaves a share that comes back at each restart. |
| `ARVOLO_SHARE_COPIES` | unset | The same bound, counted in copies taken. Either or both may be set. |
| `ARVOLO_SWARM` | on | BitTorrent-style swarm for shared `arvc…` tickets. Set `off`/`0`/`relay-only` to disable (privacy escape hatch). |
| `ARVOLO_CONCURRENCY` | `4` | Parallel chunk fetches (clamped to 1–16). |
| `ARVOLO_IPV4_ONLY` | auto | `1` forces IPv4-only (auto-detected when there's no IPv6 route). |
| `ARVOLO_CC` | `bbr` | QUIC congestion controller. `cubic` restores quinn's default — the way back if BBR misbehaves on a path we haven't measured. |
| `ARVOLO_IROH_RELAY` | n0 public | Self-hosted **iroh** NAT relay for P2P hole-punching. |
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
| `ARVOLO_MAX_BLOB_BYTES` | 0 (unlimited) | Max size of a single deposited file (mailbox/link). The deposit streams to disk, so `0` = unlimited (bounded by disk + `MAX_ENTRIES`), safe on memory. A shared/public relay sets e.g. `536870912` (0.5 GiB). |
| `ARVOLO_MAX_SESSION_RELAY_BYTES` | 0 (unlimited) | Max bytes one P2P transfer may offload to this relay (backfill), keyed on the content-derived swarm id so it's durable across suspend/resume/restart. Past it the sender falls back to direct P2P. `0` = unlimited; a shared/public relay sets e.g. `536870912` (0.5 GiB). |
| `ARVOLO_MAX_TTL` | 30 days | Max mailbox/blob TTL (seconds). |
| `ARVOLO_SEED_TTL` | 24 h | TTL for seeded chunks not yet released (seconds). |
| `ARVOLO_MAX_ENTRIES` | 100 000 | Global row cap. |
| `ARVOLO_MAX_INBOX_ROWS` / `ARVOLO_MAX_PRESENCE_ROWS` / `ARVOLO_MAX_RZ_ROWS` / `ARVOLO_MAX_SEEDED_ROWS` | per-table | Per-table row caps. |

## How it works

- **P2P transport** over [iroh](https://www.iroh.computer/) QUIC (dial by key, not
  IP; automatic hole-punching with relay fallback), with **BBR** congestion
  control: iroh resets the controller on every path event, so on a mobile uplink
  Cubic restarted from slow start every 40–60 s and never reached rate. Measured
  on the same 60 MB link: 225 s under Cubic, 34–53 s under BBR, which is what TCP
  gets on that path (`ARVOLO_CC=cubic` reverts).
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
- **Always-open client**: `listen`/`daemon` keep a client online; `arvolo send`
  delivers offers through a zero-knowledge inbox (proof-of-possession session
  auth) and presence beacons, with a two-phase watchdog that delivers **live P2P**
  when the recipient is online and falls back to the **mailbox** when they aren't.
  A live send whose recipient never connects re-offers on a 30 s→5 min backoff
  rather than every few seconds, and the recipient is told about the *arrival*,
  not about each re-publication.
- **Knowing what happened**: an offer moves along a scale — *pending*, *arrived*,
  *taken* — so "they picked it up" and "it expired" stop being the same answer.
  The recipient's ack leaves a tombstone the sender reads directly; the middle
  state says only that the offer reached a device, never that a person looked at
  it, because the relay sees reads of a slot, not anyone's attention.
- **One identity across devices**: `device pair`/`join` share a single identity
  (SPAKE2 over a short code), so contacts see one id, any device can open what was
  sent to you, and the address book — including the blocklist — stays in step.
- **Browser download links**: `arvolo link` deposits a chunked AES-256-GCM
  container; the relay serves a self-contained page that fetches the ciphertext and
  decrypts it in the browser (key only in the URL `#fragment`), streaming to disk
  without buffering the whole file. Each link is a local **session** whose removal
  revokes the blob on the relay. Zero-knowledge against an *honest* relay only —
  the decryptor is served by the relay, so a hostile operator could exfiltrate the
  fragment key; use `--to` for confidentiality against an untrusted relay.
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
| `arvolo-cli` (`arvolo`) | [`cli/`](cli/) | Command-line client and the daemon. |
| `arvolo-relay` | [`relay/`](relay/) | Self-hostable zero-knowledge relay / mailbox. |
| `arvolo-ipc` | [`arvolo-ipc/`](arvolo-ipc/) | The daemon's local wire protocol + client, shared by both frontends. |
| `arvolo-gui` | [`gui/`](gui/) | Tauri 2 + React desktop app — a thin client of the daemon. |

Build & test: `cargo build && cargo test`. The GUI has its own suite:
`cd gui && npm test`.

## Roadmap

Shipped recently: the **desktop GUI**, **multi-device** support on one shared
identity (paired over a short code, address book kept in step), the always-open
client (`listen`/`daemon`), and browser download links (the Firefox Send heir).
Planned next: relay federation (short codes across independent relays), Windows
support for the daemon IPC and the GUI, and mobile. Post-MVP ideas are tracked in
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
