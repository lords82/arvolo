# Arvolo — Quick start

The shortest path from zero to a working transfer. Two parts: **stand up a relay**
(once, self-hosted) and **point your client at it**. Everything is end-to-end
encrypted; the relay only ever holds ciphertext.

> The only setting the client *requires* is the relay URL (`ARVOLO_RELAY` or the
> `relay` key in `config.toml`). Without it you can't use pairing codes,
> `arvolo send`, the mailbox, download links, or the swarm. Everything else has a
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

**First run does this for you.** The very first time you run any `arvolo` command
in an interactive terminal with no config yet, a one-question setup asks for your
relay and writes `~/.config/arvolo/config.toml`:

```text
Welcome to Arvolo — no configuration found, quick one-time setup.

Relay URL: brokers pairing codes, `arvolo send`, the mailbox, download
links and the swarm. Leave empty to skip (plain P2P `arvc…` tickets
still work without a relay).
  • Production (TLS):  just the hostname, e.g. relay.example.com
  • LAN/dev (no TLS):  http://host:6282
Relay [none]: relay.example.com
```

The generated file has your `relay` set and **every other option listed,
commented, at its default** — so you can see and tune everything without hunting
through docs. Environment variables (`ARVOLO_*`) always override the file.

```toml
# Production (nginx + TLS): just the hostname, https and port 443 are implicit.
relay = "relay.example.com"
# LAN/dev (plain HTTP relay): write the scheme and port, e.g.
#relay = "http://relay.local:6282"

# ...download_dir, seed, swarm, concurrency, … all shown commented at their default
```

> The wizard is skipped automatically when non-interactive (scripts, systemd) so
> nothing ever blocks headless; set `ARVOLO_NO_WIZARD=1` to always skip it. You
> can also edit `config.toml` by hand, or override per-command with `--relay`.

Your identity is created automatically on first use:

```sh
arvolo me               # show your public id
```

---

## 4. Send & receive

**P2P, both online — share a short code:**

```sh
# sender
arvolo code ./photo.jpg
#   ->  4821-crater-mango          (with a configured relay; else @<relay>)

# receiver
arvolo recv 4821-crater-mango
```

**No relay at all** — `arvolo ticket ./file` prints a self-contained `arvc…`
P2P ticket; the receiver runs `arvolo recv arvc…`.

**Send to a known contact** — the tool picks the channel (live if they're online,
mailbox if not):

```sh
# receiver stays online
arvolo listen --auto-accept-contacts

# sender
arvolo send alice ./photo.jpg
```

**Offline mailbox** — leave an encrypted file on the relay until they fetch it:

```sh
arvolo send <id-or-contact> ./report.pdf --deposit   # prints an arvm… ticket
arvolo recv arvm…                                         # fetches + decrypts (burns on read)
```

The recipient doesn't need the ticket, though — the send was addressed to them,
so a bare `arvolo recv` lists it and takes it:

```sh
arvolo recv                                               # what's waiting for you; pick one
```

**Browser download link** — anyone can open it in a browser, no install; the key
lives only in the URL `#fragment`, so the relay stays zero-knowledge:

```sh
arvolo link ./report.pdf
#   ->  https://relay.example.com/dl/<claim>#<key>
```

Track everything with `arvolo status` (add `--watch` for a live view).

---

## Minimum viable checklist

- [ ] Relay reachable over HTTPS (`curl https://relay.example.com/healthz` → `ok`).
- [ ] Client `relay` set in `config.toml` (or `ARVOLO_RELAY`).
- [ ] `arvolo me` prints your public id.
- [ ] A test `arvolo code ./file` + `arvolo recv <code>` round-trips.

For the full environment-variable reference and every command, see the
[manual](MANUAL.md); for production hardening, [`DEPLOY.md`](DEPLOY.md).
