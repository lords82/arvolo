# Self-hosting on a VPS (full sovereignty)

For production you run **two server processes on your own VPS**, so both the
file data *and* the NAT-traversal coordination stay on your infrastructure:

1. **`arvolo-relay`** — the zero-knowledge **mailbox** (offline delivery). Only
   needed for the store-and-forward path; sees only ciphertext.
2. **`iroh-relay`** — the **NAT-traversal relay** that helps two clients connect
   when a direct P2P path fails. Also only ever carries encrypted QUIC traffic.
   Self-hosting it replaces n0's shared public relays (free, but dev/test only).

> Pure-LAN transfers need **neither** (mDNS discovery, fully local). Remote P2P
> needs only the iroh relay. Offline delivery needs the mailbox.

A 1 vCPU / 1 GB VPS is plenty to start.

## Quick start: the mailbox via Docker

The fastest way to run just the `arvolo-relay` mailbox is the published image
(`linux/amd64` + `linux/arm64`), which reads its config from env vars and stores
state under `/data`:

```sh
docker run -d --name arvolo-relay -p 8787:8787 -v arvolo-data:/data \
  ghcr.io/lords82/arvolo-relay:latest
```

Or with the bundled compose file (includes a `/healthz` healthcheck):

```sh
docker compose up -d
```

Put a TLS reverse proxy (Caddy, see §3) in front for a public deployment. The
sections below cover a from-source systemd deployment and the companion
`iroh-relay` for NAT traversal.

## 1. Build the binaries

On the VPS (or build elsewhere and copy the binaries):

```sh
# rustup toolchain (needs rustc >= 1.88)
curl https://sh.rustup.rs -sSf | sh -s -- -y
git clone <your-repo> && cd arvolo
cargo build --release           # -> target/release/arvolo-relay
sudo install -m755 target/release/arvolo-relay /usr/local/bin/

# the iroh NAT relay (open source, from n0)
cargo install iroh-relay        # -> ~/.cargo/bin/iroh-relay
sudo install -m755 ~/.cargo/bin/iroh-relay /usr/local/bin/
```

## 2. DNS

Point two names at the VPS, e.g.:

- `mailbox.example.com`  → the `arvolo-relay` mailbox HTTP API
- `relay.example.com`    → the `iroh-relay`

## 3. The mailbox (`arvolo-relay`)

`arvolo-relay` speaks plain HTTP; put a TLS reverse proxy (Caddy) in front.

`/etc/systemd/system/arvolo-relay.service`:

```ini
[Unit]
Description=arvolo mailbox
After=network.target

[Service]
Environment=ARVOLO_RELAY_ADDR=127.0.0.1:8787
Environment=ARVOLO_RELAY_DB=/var/lib/arvolo/relay.db
Environment=ARVOLO_RELAY_BLOBS=/var/lib/arvolo/blobs
ExecStart=/usr/local/bin/arvolo-relay
Restart=always
StateDirectory=arvolo

[Install]
WantedBy=multi-user.target
```

`Caddyfile` (automatic HTTPS):

```
mailbox.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

```sh
sudo systemctl enable --now arvolo-relay
sudo systemctl reload caddy
```

## 4. The NAT relay (`iroh-relay`)

`iroh-relay` can terminate TLS itself (Let's Encrypt) or run behind a proxy. See
`iroh-relay --help` for the current flags/config of your version; a typical
self-signed/dev run is `iroh-relay --dev`. For production, give it
`relay.example.com` and a certificate, then run it under systemd like above.

Self-hosted iroh relays are **authenticated by default** — only your project's
endpoints can use them.

## 5. Point clients at your infrastructure

On each device using `arvolo`:

```sh
# use YOUR iroh relay instead of n0's public ones
export ARVOLO_IROH_RELAY=https://relay.example.com

# offline sends target YOUR mailbox
arvolo send-offline file --to <id> --relay https://mailbox.example.com
```

With `ARVOLO_IROH_RELAY` set and your own mailbox URL, **no third-party server is
involved** — data and connection metadata both stay on your VPS.

## Cost & operations

- Storage on the mailbox is bounded by your TTLs (blobs auto-expire and are
  reaped) and the max blob size (`MAX_BLOB_BYTES`, default 2 GiB).
- The mailbox stores ciphertext only; losing the VPS never exposes plaintext.
- Back up `ARVOLO_RELAY_DB` + the blobs dir if you want delivery durability across
  reprovisioning (otherwise undelivered blobs are simply lost — which is fine,
  they would expire anyway).
