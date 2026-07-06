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
docker run -d --name arvolo-relay -p 6282:6282 -v arvolo-data:/data \
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
Environment=ARVOLO_RELAY_ADDR=127.0.0.1:6282
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
    reverse_proxy 127.0.0.1:6282
}
```

```sh
sudo systemctl enable --now arvolo-relay
sudo systemctl reload caddy
```

### Abuse hardening (optional)

The relay already bounds every endpoint (body-size, entry/row/TTL caps; inbox
offers are ≤512 KiB, ≤64 per slot, capped globally by `ARVOLO_MAX_INBOX_ROWS`).
Inbox **reads and deletes** require a proof-of-possession session token, so only
the slot owner can drain or enumerate their inbox.

Inbox **deposit** (`POST /v1/inbox/{slot}`) stays open by design — anyone must be
able to offer you a file — so a peer who knows your public id can post junk
offers up to the per-slot cap. An online client drains undecryptable offers on
every poll, so the impact is a transient nuisance, not data loss. To blunt it
further, rate-limit the deposit path at the proxy. With the
[caddy-ratelimit](https://github.com/mholt/caddy-ratelimit) module:

```
mailbox.example.com {
    @inbox_deposit {
        method POST
        path /v1/inbox/*
        not path /v1/inbox/*/session
    }
    rate_limit @inbox_deposit {
        key    {client_ip}
        events 30
        window 1m
    }
    reverse_proxy 127.0.0.1:6282
}
```

(nginx equivalent: a `limit_req_zone` keyed on `$binary_remote_addr` scoped to a
`location /v1/inbox/`.) Tune the budget to your expected legitimate offer rate.

**Rate-limit every unauthenticated route, not just inbox.** All control-plane
endpoints (`/v1/deposit`, `/v1/seed`, `/v1/rz/*`, `/v1/presence/*`,
`/v1/swarm/*`, `/v1/inbox/*`) are unauthenticated by design. The per-endpoint row
caps bound **disk**, but an attacker can still cheaply fill a *global* cap
(`ARVOLO_MAX_INBOX_ROWS`, `ARVOLO_MAX_RZ_ROWS`, `ARVOLO_MAX_PRESENCE_ROWS`, …) and
deny service to legitimate users. On a public relay, apply a per-IP `limit_req` /
`rate_limit` (and a `limit_conn` / connection cap) at the proxy across the whole
`/v1/` prefix — the relay itself ships **no** in-process rate limiter. nginx:

```nginx
limit_req_zone  $binary_remote_addr zone=arvolo:10m rate=10r/s;
limit_conn_zone $binary_remote_addr zone=arvolo_conn:10m;
server {
    # … TLS + proxy_pass to 127.0.0.1:6282 …
    location /v1/ {
        limit_req  zone=arvolo burst=40 nodelay;
        limit_conn arvolo_conn 32;
        proxy_pass http://127.0.0.1:6282;
    }
    location = /healthz { proxy_pass http://127.0.0.1:6282; }  # keep liveness un-throttled
}
```

**Cap blob and seed sizes.** By default the relay meters neither a single blob nor
the per-transfer seed offload (`ARVOLO_MAX_BLOB_BYTES=0` and
`ARVOLO_MAX_SESSION_RELAY_BYTES=0` both mean *unlimited*), so an unauthenticated
client can disk-fill or make the relay pull large amounts over the (also
unauthenticated) `/v1/seed` path. On a public relay set finite values, e.g.:

```ini
Environment=ARVOLO_MAX_BLOB_BYTES=536870912          # 512 MiB per blob
Environment=ARVOLO_MAX_SESSION_RELAY_BYTES=536870912 # 512 MiB offload per transfer
```

The relay logs a startup **warning** whenever either is left unlimited, or when it
binds a non-loopback address in plaintext without `ARVOLO_INSECURE=1`.

**Never publish the plaintext port.** The relay speaks plain HTTP; only the reverse
proxy should be reachable from the network. Bind the relay to `127.0.0.1` (as the
unit above does) so capability tokens (claim / revoke / inbox session) never travel
in cleartext. If you *intentionally* run plaintext behind an upstream TLS
terminator on another host, set `ARVOLO_INSECURE=1` to acknowledge it and silence
the warning — do **not** set it merely to quiet the log on a directly-exposed relay.

### Disabling browser download links (optional)

Public browser download links (`send --link`) let anyone with the link
fetch a file with no account — convenient, but some deployments prefer to allow
only recipient-sealed sends. Start the relay with:

```
Environment=ARVOLO_DISABLE_LINKS=1
```

Then the relay reports `{"links":false}` on `GET /v1/features`, serves `403` for
the `/dl` page and its assets, and refuses link deposits. `arvolo send
--link` fails **immediately** (before uploading) with a message telling the user
the administrator disabled the feature; normal `send --to` and all other
paths are unaffected.

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

# sends to a contact use YOUR relay (mailbox when they're offline; --use-http for plaintext)
arvolo send file --to <id> --relay mailbox.example.com
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
