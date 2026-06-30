# Arvolo

Secure, cross-platform file sending: **P2P-first** when possible, **store-and-forward via a self-hostable relay with expiry** when the recipient is offline. End-to-end encrypted, zero-knowledge relay.

> Status: **working CLI MVP** — P2P transfer (relay fallback + resume), HPKE
> end-to-end encryption with sender authenticity, and a self-hostable
> expiring mailbox. Desktop GUI, mobile, and browser link-mode are planned.

## Workspace layout

| Crate | Path | Role |
|-------|------|------|
| `arvolo-core` | [`core/`](core/) | Shared engine abstractions and types. The networking engine ([iroh](https://www.iroh.computer/)) lives behind a `Transport` trait. |
| `arvolo-cli` (`arvolo`) | [`cli/`](cli/) | Command-line client. |
| `arvolo-relay` (`arvolo-relay`) | [`relay/`](relay/) | Self-hostable relay / mailbox (built in Milestone 2). |

## Build

Needs a recent Rust (iroh + the RustCrypto rc cohort require ≥1.88; CI uses stable).

```sh
cargo build
cargo test
cargo run -p arvolo-cli -- --help
```

## Usage

**P2P (both devices online):**

```sh
# device A
arvolo send ./photo.jpg          # prints: arvolo recv blob…
# device B
arvolo recv blob…                # fetches it (LAN-direct or relay fallback, with resume)
```

**Offline mailbox (recipient away — store-and-forward with expiry):**

```sh
# recipient: show your public id
arvolo id                         # prints e.g. kpb27rz2…

# run a relay (self-hostable; SQLite + files on disk, auto-expiring)
arvolo-relay                      # listens on 0.0.0.0:8787

# sender: encrypt (HPKE, end-to-end) and deposit
arvolo send-offline ./report.pdf --to <recipient-id> --relay http://relay:8787
#   -> prints: arvolo recv-offline arvm…

# recipient (later): fetch + decrypt; blob is burned/expires on the relay
arvolo recv-offline arvm…
```

The relay only ever sees ciphertext (zero-knowledge); blobs auto-delete at TTL or
after the download budget (burn-after-read).

**Full self-host (production):** run the mailbox *and* your own iroh NAT relay on
a VPS, then point clients with `ARVOLO_IROH_RELAY=https://relay.example.com` so no
third-party server is involved. See [`docs/DEPLOY.md`](docs/DEPLOY.md).

## Roadmap

The v1.0 MVP delivers: P2P-first send (LAN + remote with relay fallback + resume), HPKE end-to-end encryption with sender authenticity, ephemeral code/QR and persistent-identity pairing, a single self-hostable relay with expiring offline mailbox, browser link-mode (Firefox Send heir), and a desktop UI.

Post-MVP ideas (federation, multi-recipient, swarming, mobile, business edition) are tracked in [`docs/ROADMAP-FUTURE.md`](docs/ROADMAP-FUTURE.md).

## Licensing

Open-core. The core (client + single relay) is free software under
**[AGPL-3.0-only](LICENSE)** — you can self-host and modify it, and the AGPL keeps
it open even when run as a network service. A separate **commercial license** is
available for proprietary/embedded use without the AGPL's obligations; business
features (federation, SSO, audit, managed hosting) are commercial.

The AGPL covers the **code**, not the **name**: "Arvolo" is a trademark of the
project owner and may not be used by forks in a way that implies endorsement.
See [`CONTRIBUTING.md`](CONTRIBUTING.md).
