# Arvolo — Quick start

The shortest path from zero to a working transfer. Two parts: **stand up a relay**
(once, self-hosted) and **point your client at it**. Everything is end-to-end
encrypted; the relay only ever holds ciphertext.

> The only setting the client *requires* is the relay URL (`ARVOLO_RELAY` or the
> `relay` key in `config.toml`). Without it you can't use pairing codes,
> `send --to`, the mailbox, download links, or the swarm. Everything else has a
> sane default.

---

## 1. Install

```sh
curl -fsSL https://raw.githubusercontent.com/lords82/arvolo/main/install.sh | sh
```

Installs `arvolo` (client) and `arvolo-relay` (server) into `/usr/local/bin`. Or
grab prebuilt binaries from the [latest release](https://github.com/lords82/arvolo/releases),
or `cargo install arvolo-cli arvolo-relay` (Rust ≥ 1.88).

---

## 2. Run the relay

The relay listens on **HTTP, port 6282** by default and stores nothing in the
clear. How you expose it depends on the environment.

### A. LAN / dev — expose the port directly (plain HTTP)

Fastest way to try it on a trusted network:

```sh
docker run -d --name arvolo-relay -p 6282:6282 -v arvolo-data:/data \
  ghcr.io/lords82/arvolo-relay:latest
```

Clients then use `--relay <host>:6282 --use-http` (see §3). **Do not** expose
plain HTTP to the public internet — it's for LAN/dev only.

### B. Production — behind nginx with TLS (recommended)

Never publish 6282 to the internet in the clear. Put a TLS-terminating reverse
proxy in front; the relay stays plain HTTP, reachable only by the proxy.

```
client ──https:443──> nginx (TLS) ──http:6282──> arvolo-relay
```

**Keep the relay on localhost** (systemd) or on the internal Docker network:

```ini
# /etc/systemd/system/arvolo-relay.service
[Service]
Environment=ARVOLO_RELAY_ADDR=127.0.0.1:6282
Environment=ARVOLO_RELAY_DB=/var/lib/arvolo/relay.db
Environment=ARVOLO_RELAY_BLOBS=/var/lib/arvolo/blobs
Environment=ARVOLO_RELAY_BLOBSTORE=/var/lib/arvolo/blobstore
ExecStart=/usr/local/bin/arvolo-relay
DynamicUser=yes
StateDirectory=arvolo
Restart=always

[Install]
WantedBy=multi-user.target
```

**nginx site** — terminate TLS on 443 and forward to the relay. Use certbot for
the certificate:

```nginx
server {
    listen 443 ssl;
    listen [::]:443 ssl;
    server_name relay.example.com;

    ssl_certificate     /etc/letsencrypt/live/relay.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/relay.example.com/privkey.pem;

    # Downloads and blob uploads can be large — don't let nginx cap them.
    client_max_body_size 0;

    location / {
        proxy_pass         http://127.0.0.1:6282;
        proxy_http_version 1.1;
        proxy_set_header   Host              $host;
        proxy_set_header   X-Real-IP         $remote_addr;
        proxy_set_header   X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header   X-Forwarded-Proto $scheme;
        proxy_request_buffering off;   # stream uploads through, don't buffer to disk
        proxy_read_timeout 3600s;      # long-lived transfers
    }
}
```

```sh
sudo certbot --nginx -d relay.example.com   # one-time: obtain + wire up the cert
sudo systemctl enable --now arvolo-relay
sudo nginx -t && sudo systemctl reload nginx
```

Check it's alive: `curl -fsS https://relay.example.com/healthz` → `ok`.

> Running the relay as a Docker container behind nginx instead of systemd? Don't
> publish the port (`docker-compose.yml` ships with the host port commented out);
> put nginx and the relay on the same Docker network and `proxy_pass` to
> `http://arvolo-relay:6282`. See [`DEPLOY.md`](DEPLOY.md).

### (Optional) NAT relay for P2P hole-punching

P2P transfers use [iroh](https://www.iroh.computer/) and, by default, n0's public
relays only to hole-punch (never for your data). To self-host that too, run an
iroh relay and set `ARVOLO_IROH_RELAY` on the clients. Not required to get started.

---

## 3. Configure the client

Point the client at your relay — once, in `~/.config/arvolo/config.toml`:

```toml
# Production (nginx + TLS): just the hostname, https and port 443 are implicit.
relay = "relay.example.com"

# LAN/dev (plain HTTP relay): write the scheme and port explicitly.
# relay = "http://relay.local:6282"

# Optional: where received files land (default: ~/Arvolo).
# download_dir = "/srv/arvolo/incoming"
```

Or per-invocation with `--relay <url>` / the `ARVOLO_RELAY` env var. First run
creates your identity automatically:

```sh
arvolo id        # show your public id (created on first use)
```

---

## 4. Send & receive

**P2P, both online — share a short code:**

```sh
# sender
arvolo send --code ./photo.jpg
#   ->  4821-crater-mango          (with a configured relay; else @<relay>)

# receiver
arvolo recv 4821-crater-mango
```

**No relay at all** — plain `arvolo send ./file` prints a self-contained `arvc…`
P2P ticket; the receiver runs `arvolo recv arvc…`.

**Send to a known contact** — the tool picks the channel (live if they're online,
mailbox if not):

```sh
# receiver stays online
arvolo listen --auto-accept-contacts

# sender
arvolo send ./photo.jpg --to alice
```

**Offline mailbox** — leave an encrypted file on the relay until they fetch it:

```sh
arvolo send ./report.pdf --to <id-or-contact> --ticket   # prints an arvm… ticket
arvolo recv arvm…                                         # fetches + decrypts (burns on read)
```

**Browser download link** — anyone can open it in a browser, no install; the key
lives only in the URL `#fragment`, so the relay stays zero-knowledge:

```sh
arvolo send ./report.pdf --link
#   ->  https://relay.example.com/dl/<claim>#<key>
```

Track everything with `arvolo transfers` (add `--watch` for a live view).

---

## Minimum viable checklist

- [ ] Relay reachable over HTTPS (`curl https://relay.example.com/healthz` → `ok`).
- [ ] Client `relay` set in `config.toml` (or `ARVOLO_RELAY`).
- [ ] `arvolo id` prints your public id.
- [ ] A test `arvolo send --code ./file` + `arvolo recv <code>` round-trips.

For the full environment-variable reference and every command, see the
[README](../README.md); for production hardening, [`DEPLOY.md`](DEPLOY.md).
