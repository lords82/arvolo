# Arvolo — Communication Protocol

This document specifies the **wire protocol** of the open-core Arvolo product: the
identities and cryptographic primitives, the capability/ticket formats, the P2P
transport, the relay's HTTP API, and every end-to-end flow (P2P transfer,
short-code pairing, offline mailbox, the always-open presence/inbox model, and
browser download links).

It describes **what goes over the wire and why it is safe**. For the design
rationale and threat model see [`TECHNICAL-OVERVIEW.md`](TECHNICAL-OVERVIEW.md);
for identity binding see [`IDENTITY-VERIFICATION.md`](IDENTITY-VERIFICATION.md).
Managed-identity, federation, and multi-relay features are **not** part of the
open core and are out of scope here.

> Scope note: this covers `arvolo-core`, `arvolo` (CLI), and `arvolo-relay`.
> Section references point at the source of truth, e.g.
> [`core/src/crypto.rs`](../core/src/crypto.rs).

---

## 1. Terminology

| Term | Meaning |
|---|---|
| **Identity** | A long-term seed yielding two keypairs: Ed25519 (signing) + X25519 (KEM). The two public halves are your **id** (no PII). |
| **Contact id** | Another party's two public halves, base32-encoded (64 bytes). What you send **to**. |
| **Fingerprint** | An eight-word BLAKE3 digest (64 bits) of a public id, for out-of-band verification. |
| **Ticket** | A self-describing capability string that lets the holder receive a transfer (`arvc…` / `arvm…`). |
| **Claim** | A random 16-byte capability token addressing one blob on the relay. |
| **Slot** | A per-identity address on the relay. An inbox slot is a per-epoch *blinded public key* (§6.2) — the relay authenticates against it without learning whose it is; a presence slot is a hash. |
| **Content key** | A fresh random 32-byte AEAD key per transfer; encrypts the chunk stream; travels only in the ticket. |
| **Relay** | The self-hostable, zero-knowledge store-and-forward + rendezvous server. It only ever sees ciphertext and opaque tokens. |

---

## 2. Cryptographic primitives

All defined in [`core/src/crypto.rs`](../core/src/crypto.rs).

### 2.1 Identity — two keypairs from one seed
An identity is a 32-byte **seed**, stored locally (0600 on unix), from which two
independent keypairs are derived through separate BLAKE3 contexts:

| half | curve | used for |
|---|---|---|
| signing | Ed25519 | naming you, and proving you own your inbox (§6.2, §7.5) |
| KEM | X25519 | what payloads are encrypted toward (§2.2) |

The **contact id** is the two public halves concatenated, 64 bytes. There is no
certificate, account, or PII — the keys **are** the identity.

The two could have been one: Ed25519 and X25519 are the same curve in different
coordinates and converting between them is standard practice. That was declined.
Sharing a key across a signature scheme and a KEM is believed safe and has been
analysed as such, but it is an assumption this protocol otherwise never asks anyone
to accept; and the conversion drops the sign bit, so `A` and `-A` — two distinct
contact ids — would share one encryption key. The cost of avoiding both is an id of
64 bytes rather than 32. The eight-word fingerprint people read aloud (§2.7) is
unchanged, so that cost is paid by clipboards and QR codes, not by users.

Parsing an id validates it: the signing half must be a canonical Edwards point in
the prime-order subgroup. That check is load-bearing rather than hygienic — the
per-epoch blinding factor (§6.2) is a full-range scalar and does not clear the
cofactor the way a clamped X25519 scalar silently did.

### 2.2 Contact encryption — HPKE auth mode (RFC 9180)
Sends addressed to a specific recipient use **HPKE** with:

- **KEM** X25519-HKDF-SHA256, **KDF** HKDF-SHA256, **AEAD** AES-256-GCM.
- **Auth mode**: the sender's static key is mixed in, so the recipient
  cryptographically learns *who* sent the payload. A wrong sender fails to open,
  a wrong recipient cannot open, and tampering is rejected by the AEAD tag.
- **Info string** `arvolo/hpke/v1` domain-separates the KDF.
- **Base mode** (`seal_anon`/`open_anon`) is used where no sender identity is
  proven — the outer, sealed-sender layer around an inbox offer (§7.5).

On the wire an HPKE output is `(encapped_key, ciphertext)`; the encapsulated key
travels as a header, the ciphertext as the body (§6.1).

### 2.3 Chunk stream — per-transfer content key + AES-256-GCM
The chunked transfer path is an **ephemeral capability model**: whoever holds the
ticket can receive. Each chunk is sealed **independently** with AES-256-GCM under
a fresh random 32-byte content key that travels only inside the ticket:

- **Nonce** (12 bytes): the little-endian chunk index in the first 4 bytes, the
  rest zero. The key is fresh-random per transfer, so every `(key, nonce)` pair is
  used exactly once — the invariant AES-GCM depends on.
- **AAD** (8 bytes): little-endian `index ‖ total_chunks`, so reordering or
  truncation is rejected on open.
- Output is `plaintext ‖ 16-byte tag`. Each ciphertext chunk is self-verifying
  and addressed by its BLAKE3 hash, so out-of-order multi-source fetch and resume
  stay safe.

**One AEAD everywhere.** AES-256-GCM (rather than ChaCha20-Poly1305) is used for
HPKE, the chunk stream, and the password wrap so the exact same cipher can be
decrypted natively in a browser via WebCrypto for download links (§7.6). It is
equivalent in strength and hardware-accelerated (AES-NI) on target platforms.

### 2.4 Optional link password — Argon2id outer wrap
An optional outer layer so that **holding the ticket is not enough** to decrypt:
the payload is additionally wrapped under an **Argon2id**-derived key (context
`arvolo/pw/v1`). A fresh random 16-byte salt travels with the ticket (not secret);
the password is shared out-of-band. A fixed all-zero nonce is safe because the
Argon2id key is unique per salt. Without the password even the intended recipient
cannot decrypt, and the relay can never bypass it.

### 2.5 Content addressing — BLAKE3
Every chunk/blob is identified by its **BLAKE3** hash of the *ciphertext*. The
hash is the manifest: it makes resume and multi-source fetch source-agnostic and
self-verifying. Slots and tokens also use BLAKE3 (keyed / derive-key mode).

### 2.6 Short-code pairing — SPAKE2
A short human code (`4821-crater-mango`) is turned into a shared key with a
**SPAKE2** PAKE keyed on the code ([`core/src/code.rs`](../core/src/code.rs),
[`core/src/pairing.rs`](../core/src/pairing.rs)). The PAKE admits no offline
dictionary attack, so two short words are safe; the relay only sees PAKE messages
and the encrypted ticket.

### 2.7 Fingerprint
`PublicId::fingerprint()` is eight words derived from BLAKE3 of the public key
(**64 bits**, context `arvolo/fp/v2`), a display aid for out-of-band verification.
Widened from the original six words (~48 bits), which an active MITM could grind a
matching keypair against. The full base32 id remains authoritative for matching
contacts.

---

## 3. Encodings & conventions

| Where | Encoding |
|---|---|
| Public ids, claims, tokens, slots, header values | **base32 (RFC 4648, no padding)**, matched case-insensitively (headers are upper-cased before decode). |
| Download-link key (URL `#fragment`) | **base64url, no padding** (compact, URL-safe). |
| Structured ticket bodies | **postcard** (compact binary), then base32, with a type prefix. |
| Relay API | HTTP/1.1; bodies are raw bytes or short text; capability tokens travel in `x-arvolo-*` headers or the path. |
| Integers in the link container | little-endian. |

---

## 4. Capability & ticket formats

### 4.1 `arvc…` — chunked P2P ticket
Produced by `arvolo ticket`. A base32 postcard of
[`ChunkTicket`](../core/src/chunked.rs): total size, chunk size (16 MiB), the
ordered list of chunk **BLAKE3 hashes**, one or more provider **iroh
`EndpointAddr`**s, an optional relay backfill descriptor, a suggested name, an
`archive` flag, and the **key delivery**:

- `KeyDelivery::Plain(key)` — the 32-byte content key in the clear (a bearer
  capability: anyone with the ticket can receive).
- `KeyDelivery::Sealed{ encapped_key, ciphertext, sender }` — the content key
  HPKE-sealed to a specific recipient (`--to`), so only they can receive and they
  learn the authenticated sender. AAD `arvolo/chunk-key/v1`.

### 4.2 `arvm…` — offline (mailbox) ticket
Produced by `arvolo send` (mailbox path). A base32 postcard of
[`OfflineTicket`](../core/src/offline.rs): the `relay` URL, the `claim`, the
`sender` public id, and a password `salt` (empty if no `--password`). The file is
HPKE-sealed **directly** to the recipient (no separate content key); the relay
stores that ciphertext under `claim`.

### 4.3 Pairing code — `N-word-word[@relay]`
A leading digit plus two words from a fixed wordlist, optionally `@relay` so the
receiver needs no configuration. It is **not** the ticket; it is the SPAKE2
password used to fetch the ticket from a relay rendezvous (§7.2).

### 4.4 Download link — `https://<relay>/dl/<claim>#<key>`
Produced by `arvolo link`. The `claim` addresses a relay blob (a
chunked AES-256-GCM **container**, §7.6); the `#fragment` carries the 32-byte
content key in base64url. Browsers never send the fragment to the server, so the
relay stays zero-knowledge.

---

## 5. P2P transport (iroh QUIC)

Transport is [iroh](https://iroh.computer) — **dial keys, not IPs**, with
automatic hole-punching and a NAT-relay fallback (configurable via
`ARVOLO_IROH_RELAY`). Two application protocols run over it, selected by ALPN:

| ALPN | Purpose |
|---|---|
| `arvolo/chunk/1` | Content-addressed **chunk transfer**: the receiver requests a chunk by BLAKE3 hash; the provider streams the ciphertext chunk. Self-verifying, resumable, multi-source. |
| `arvolo/ctrl/2` | **Control channel**: acks, progress, and relay-backfill coordination between the two endpoints. |

The sender encrypts **on the fly** and stores nothing: reading the source in 16
MiB windows, sealing each chunk, and serving it by hash — bounded memory and no
extra disk regardless of file size. Chunk providers advertised in the ticket may
be the sender's node and/or a relay's blob node (§7.3).

---

## 6. Relay HTTP API

[`relay/src/lib.rs`](../relay/src/lib.rs). The relay is **zero-knowledge**: it
stores opaque ciphertext and opaque tokens, addressed by random claims or by slots
derived from public ids. Metadata lives in SQLite, blobs as files on disk; both
survive restarts and are reaped on TTL.

A fixed 16 MiB request-body limit wraps every control-plane route (rendezvous,
inbox, presence, seed, swarm) so no handler can buffer an unbounded body. The
streaming `/v1/deposit` route is exempt and enforces two caps of its own as it
streams to disk (never through memory): a per-blob cap, `ARVOLO_MAX_BLOB_BYTES`
(default 16 GiB — this is the *functional* bound on the largest file an offline
send or link can carry; `0` lifts it), and an **aggregate** stored-bytes cap,
`ARVOLO_MAX_TOTAL_BLOB_BYTES` (default unlimited with a startup warning — this
is the actual disk-fill guard; set it to the disk budget you lend out). All
unauthenticated **write** routes (deposit, seed, inbox post, presence,
swarm announce) additionally share a per-IP write-rate limit
(`ARVOLO_WRITES_PER_MIN`, default 240; §6.4).

### 6.1 Endpoint reference

| Method | Path | Auth | Purpose |
|---|---|---|---|
| POST | `/v1/deposit?ttl=&max=` | revoke-hash (opt) | Store a ciphertext blob; returns a fresh `claim`. Headers: `x-arvolo-encapped-key` (base32, may be empty), `x-arvolo-revoke-hash` (base32 BLAKE3 of the revoke token, optional). |
| GET | `/v1/fetch/{claim}` | bearer (the claim) | Return the ciphertext + `x-arvolo-encapped-key` header. **Burn-after-read**: the download counter is consumed server-side; the blob is deleted when it reaches the cap. |
| DELETE | `/v1/entry/{claim}` | revoke token | Delete the blob. Header `x-arvolo-revoke-token`; the relay checks its BLAKE3 against the stored hash (constant-time). |
| GET | `/v1/entry/{claim}/status` | none | `pending` (present) / 404 `gone`. The claim is already a secret capability, so existence leaks nothing. |
| GET | `/v1/addr` | none | The relay blob node's iroh address + a seed token (for backfill). |
| POST | `/v1/seed` | seed token | Seed (backfill) a sender's chunks into the relay's blob store so a transfer finishes even if the sender goes offline. Capped per request and in total. |
| POST | `/v1/release/{token}/{hash}` | release token | Release a seeded chunk. |
| POST / GET / DELETE | `/v1/rz/{slot}/{key}` | per-key (below) | **Rendezvous** for short-code pairing (§7.2). Keys match `^[a-z0-9][a-z0-9._-]{0,63}$`; values are size-capped; a slot holds at most 32 rows and TTLs out. Every key is first-writer-wins. Per-IP rate limits (POSTs/min, *distinct slots* touched by GET/min — polling one slot is never throttled, and new v2 claims/hour) blunt nameplate sweeps and pairing griefing. |

Rendezvous keys are a small reserved vocabulary, and who may write which is what
keeps a receiver from answering on the sender's behalf — without the split below, a
hostile receiver could pre-write the reply to its own session and poison it with
two POSTs.

| key | POST | GET |
|---|---|---|
| `own` | anyone, first write claims the slot (v2). Body is a 32-byte `blake3(owner_token)`; the stored row also carries a creation stamp for the absolute lifetime cap | anyone → the fixed marker `2`, **never** the stored verifier |
| `sessions` | never (405) | owner token only; long-polls up to `?wait=30` and renews the slot's lease as a side effect — there is no separate keepalive |
| `r.{sid}`, `c.{sid}` | anyone, first-writer-wins, requires a claimed slot | owner token only (the list of live session ids must not leak) |
| `s.{sid}`, `t.{sid}` | **owner token only** | anyone; reading `t.` deletes that session's `r./s./c./t.` and nothing else |
| `b.{sid}` | anyone, first-writer-wins, requires a claimed slot | owner token only; reading it burns that one row |
| `ms`, `mr`, `tkt` | v1, unchanged | v1, unchanged — except `ms` answers **410 Gone** on a v2 slot, so an old client fails immediately instead of polling for its full two-minute timeout |
| `own` (DELETE) | — | owner token only: retires the code and frees the nameplate |

The owner token travels in `Authorization: Bearer`, never a query parameter —
query strings are logged verbatim by reverse proxies.

**The reply key (`b.`) — the one direction the rendezvous did not have.** Every
other key moves the payload one way: the owner fills the slot, a receiver empties
it. That cannot express an *exchange*, where both sides must come away holding
something of the other's — which is what `arvolo contacts pair` needs, since
neither participant is more the sender than the other.

`b.{sid}` carries a sealed reply back. Public write, because the receiver holds no
owner token and cannot be asked for one; owner read, for the same reason `r.`/`c.`
are. That split is safe because the value is sealed under a key derived from the
**completed** PAKE and domain-separated from the payload's
(`arvolo/code/reply-key/v2` vs `…/ticket-key/v2`): only a party that already
proved it knew the code can produce one the owner will open, the relay never sees
inside, and the two directions cannot be crossed. The receiver writes it only
*after* opening the payload, so a wrong code writes nothing at all.

A relay without the key rejects the write (the prefix is unknown, so it falls
through to the v1 branch and 409s). `arvolo contacts pair` treats that as fatal —
it checks the relay's version before showing a code, and refuses a half-completed
trade — while the primitive itself only reports it, leaving the policy to callers
that may legitimately not need a reply.
| POST / GET | `/v1/inbox/{slot}` | session (GET) | Post a sealed-sender **offer** to a recipient's inbox / long-poll for offers. |
| POST | `/v1/inbox/{slot}/session` | none (the challenge is public) | Start a read session: returns a random nonce + a relay MAC over `(slot, nonce, exp)`. Useless to anyone who cannot sign it. |
| DELETE | `/v1/inbox/{slot}/{id}` | poster token | Retract your own offer. |
| GET | `/v1/inbox/{slot}/{id}/status` | poster token | How far the offer got: `pending` (no client of theirs has read it) → `fetched` (it reached one of their devices — see below) → `taken` (they fetched the file and acked). `gone` means it ended without being taken: retracted, or lapsed. |
| POST / GET | `/v1/presence/{slot}` | none | Publish / read a **presence beacon** (is this identity online right now?). Unauthenticated by design; see the presence caveat in §7.5. |
| POST | `/v1/swarm/{swarm_id}/announce` | none | **Swarm tracker**: announce this peer (iroh addr + piece bitfield) for a shared `arvc…` transfer so co-downloaders find each other. The swarm id is derived from ticket content — a capability; the relay learns node addresses and bitfields, never keys or plaintext. Rows TTL out (~60 s); peers/swarms are capped. |
| GET | `/v1/swarm/{swarm_id}/peers` | none | List the live peers announced for a swarm. A poisoned/fake announce can only waste a dial: every chunk is verified by BLAKE3 hash + AEAD, so bad providers are detected, not trusted. |
| GET | `/dl/{claim}` | none | The browser **download page** (static HTML, strict CSP). It stays a static file and translates itself in the browser (en/it/fr/de, off `navigator.languages`, English fallback). The 403 page served when links are disabled ships no script, so that one picks its language from `Accept-Language`. |
| GET | `/dl.js`, `/arvolo-sw.js` | none | The download script and streaming service worker (embedded in the binary). |
| GET | `/v1/features` | none | Advertise optional features so a client can fail fast: `{"links":true,"rz2":true}`. The two are read with **opposite defaults** on purpose — an unreachable or older relay means "links allowed" (worst case, a deposit is refused later) but "no rz2", because minting a v2 code no relay can host would strand whoever types it. |
| GET | `/healthz` | none | Liveness. |

### 6.2 Slots (per-epoch blinded keys)
An inbox slot is not a hash of your public id: it is a **blinded public key**
derived from the signing half of your identity, one per epoch.

```
h             = blake3_derive_key("arvolo/inbox/blind/v1", ed_pubkey ‖ le64(epoch))
inbox_slot    = base32( h · A )          # A = your Ed25519 half; epoch = unix / 604800 (7 days)
presence_slot = base32( blake3_derive_key("arvolo/presence/slot/v2",
                                          pubkey ‖ le64(unix / 3600)) )   # 1 hour
```

This is Tor's v3 onion-key blinding. It satisfies two requirements that pull
against each other: **anyone holding your contact id can compute `h·A`**, so a
sender can address an inbox it cannot read; and **`h·A` reveals nothing about
`A`**, so the relay cannot tie one epoch's slot to the next.

The presence beacon keeps a plain hash: it is unauthenticated by design (§7.5),
so there is nothing for a key to prove there.

**Why it rotates.** A slot appears in the *request path*, so it lands in the
access log of the relay and of any reverse proxy in front of it — and a client
polls its own inbox continuously and refreshes a presence beacon every 30s. A
slot that never changed put one unchanging string in those logs for the life of
the identity, which groups a user's entire history together for anyone reading
them, across IP changes, without ever learning who they are.

**Reading across a boundary.** A reader looks in the current epoch's slot and the
previous one, current first, and signs for whichever it is talking to — the
previous epoch's slot needs the previous epoch's blinded key. Inbox row TTLs are
clamped client-side to one epoch, which makes the guarantee total: a row that is
still alive is always in a slot its owner reads. Only the current slot is
long-polled — older slots are drained with `wait=0`, every 5 minutes in a poll
loop and always on a one-shot read — so steady-state request volume is
essentially unchanged. A presence check spends a second request only within 120s
of a boundary, where a beacon published just before it can still be alive.

**What the relay knows.** Nothing that identifies a reader. It never receives a
public key (§7.5), and it holds no opinion about which epoch a slot belongs to —
there is nothing to compare a slot against, so there is no window to be right
about and no clock skew to tolerate. An earlier design hashed the public id and
had the reader POST its key so the relay could seal a challenge to it; the relay
then learned every reader's long-term identity, and with it every slot that
identity would ever have, which left the rotation protecting the access logs and
nothing else.

**Upgrade order.** Blinded slots are a different address space from the hashed
ones, and the identity format changed with them (§2.1), so clients and relay must
cross together. A client of the old shape and one of the new simply do not meet —
offers and beacons are missed rather than mis-delivered, and a sealed `arvm…`
ticket handed over directly still works throughout.

### 6.3 Capability tokens
- **claim** — 16 random bytes; the bearer capability for one blob.
- **revoke token** — sender-held secret; the relay stores only its BLAKE3 hash and
  compares in constant time on DELETE.
- **poster token** — retract/status secret for a posted inbox offer; same
  hash-only, constant-time pattern.
- **session token** — a relay MAC (keyed by a per-process secret) over
  `(slot, nonce, exp)`, presented to authorize inbox reads for a while (§7.5).

### 6.4 Operating a public relay (hardening)

End-to-end encryption means a hostile network or relay never reads content — but
an **open** relay is still an unauthenticated public service, and its
availability guards need configuring:

- **TLS**: terminate HTTPS in front of the relay (e.g. nginx + certbot). Session
  tokens travel as `Authorization: Bearer`, and claims/tickets in URLs/headers,
  so plaintext HTTP lets an on-path attacker burn download counters and grief
  transfers (never read them). Bare hostnames in codes/config default to
  `https://`.
- **Finite caps**: set `ARVOLO_MAX_TOTAL_BLOB_BYTES` to the disk budget you are
  willing to lend — it is the real disk-fill guard (unlimited by default; the
  relay warns at startup). `ARVOLO_MAX_BLOB_BYTES` (default 16 GiB, `0` lifts
  it) only bounds a *single* deposit — i.e. the largest file an offline send or
  link can carry — not the total. Set `ARVOLO_MAX_SESSION_RELAY_BYTES` to meter
  seed/backfill offload (unlimited by default, warned at startup). The row caps
  (entries, rendezvous, inbox, presence, seeded chunks) default on, but note
  they are *global*: a flood can fill them and deny service to legitimate users
  until TTLs reap — that is the accepted failure mode of an account-less relay
  (degraded availability, never confidentiality).
- **Rate limiting**: the relay itself rate-limits per client IP — the rendezvous
  routes (`ARVOLO_RZ_POSTS_PER_MIN`, `ARVOLO_RZ_SLOTS_PER_MIN`) and all
  unauthenticated writes (`ARVOLO_WRITES_PER_MIN`, default 240, shared across
  deposit/seed/inbox-post/presence/swarm-announce). Set `ARVOLO_TRUST_PROXY=1`
  behind a reverse proxy so the limiters key on `X-Forwarded-For` — only then,
  or clients could spoof it. Proxy-level rate limits on top add defense in
  depth.
- **Abuse surface**: `/v1/deposit` is unauthenticated by design (anyone may
  leave you a sealed file), which also makes an open relay free anonymous
  ciphertext storage. TTLs (default 7 days, capped at 30) and the blob cap bound
  it; a private relay can additionally sit behind network-level allow-lists.
- **Links**: `ARVOLO_DISABLE_LINKS=1` if you don't want to serve the browser
  download path (§7.6).

---

## 7. Protocol flows

### 7.1 P2P transfer (ticket)
```
sender:  arvolo ticket file         → prints arvc… (providers = sender's iroh addr)
receiver: arvolo recv arvc…
  1. parse ticket: chunk hashes, providers, key delivery
  2. if Sealed → HPKE-open the content key with your identity (learns sender)
  3. dial a provider by key (arvolo/chunk/1); request each chunk by BLAKE3 hash
  4. verify hash, AES-256-GCM-open (nonce=index, aad=index‖total), write in place
  5. resume across and within chunks on interruption; unpack if archive
```
Both online ⇒ data flows **directly**; the relay is never touched.

> **No trusted relay required for P2P.** A pure `arvc` transfer (ticket shared
> out-of-band, both peers online) involves **no relay at all** — there is nothing to
> trust. With short-code pairing (§7.2) a relay is used only as a **rendezvous**: the
> SPAKE2 PAKE means a malicious relay can neither read the data nor MITM the exchange
> without the code — it can only deny service. So the relay never needs to be trusted
> for confidentiality or integrity on the P2P paths; the hostile-relay caveat in §7.6
> is specific to the **browser link** path, where the relay serves the decryptor.

### 7.2 Short-code pairing (SPAKE2 over rendezvous)

The code a user types is the same in both protocol versions — `N-word-word` with
an optional `@relay` — and nothing in it says which one it speaks. The receiver
works that out from the slot: a **v2** slot carries an `own` marker, a **v1** slot
carries the sender's SPAKE2 message under `ms`. Whichever appears first wins, so a
receiver that types the code before the sender has finished claiming still lands in
the right place.

#### 7.2.1 v1 — one exchange, in memory (legacy)
```
sender:  arvolo code --relay R --foreground file  → 4821-crater-mango[@R]
  - run SPAKE2 (password = code); PUT the sender message + the SPAKE2-encrypted
    ticket to /v1/rz/{slot}/{key}
receiver: arvolo recv 4821-crater-mango[@R]
  - GET the rendezvous, finish SPAKE2, decrypt the ticket, then proceed as §7.1
```
The relay sees only PAKE messages and the encrypted ticket; a wrong code derives a
different key and simply fails — no offline dictionary attack.

Three limits follow from modelling the code as a single live handshake: it serves
exactly one receiver, it dies with the process that holds the SPAKE2 scalar, and
the relay's slot TTL bounds it to minutes. All three are why a v1 code cannot be
hosted in the daemon.

#### 7.2.2 v2 — a long-lived slot, one sub-session per receiver
```
sender:   POST /v1/rz/{slot}/own      body = blake3(owner_token)
          (409 → nameplate taken, pick another; the token never leaves the sender)
          GET  /v1/rz/{slot}/sessions?wait=30   Authorization: Bearer <owner_token>
          → the session ids waiting for an answer; renews the lease as a side effect
receiver: POST /v1/rz/{slot}/r.{sid}  body = its SPAKE2 message   (sid: 128 random bits)
sender:   POST /v1/rz/{slot}/s.{sid}  body = a FRESH SPAKE2 message for this receiver
receiver: POST /v1/rz/{slot}/c.{sid}  body = key confirmation (below)
sender:   POST /v1/rz/{slot}/t.{sid}  body = the payload sealed to this session
receiver: GET  /v1/rz/{slot}/t.{sid}  → burns this session's four keys, not the slot
```
The sender's whole state is `(slot, code, relay, owner_token)` — four values that
fit in a file. That is what makes a v2 code survive the sender restarting, and it
is the only reason the daemon can host one.

**Key confirmation.** Before sealing anything, the sender requires
`blake3::keyed_hash(derive_key("arvolo/code/confirm-recv/v2", pake_key), slot ‖ 0x00 ‖ sid)`
from the receiver. Only that direction is explicit: the sender's own confirmation
is already the AEAD tag on the payload, which a wrong key cannot open. The point of
the receiver's is that it lets the sender **count** wrong codes — without it, every
guesser is handed ciphertext and the sender never learns anything. Payloads are
sealed under `derive_key("arvolo/code/ticket-key/v2", pake_key ‖ 0x00 ‖ slot ‖ 0x00 ‖ sid)`,
so no two sessions — or protocol versions — share a key. A wrong confirmation is
answered with noise rather than silence, so a mistyped code fails at once instead
of waiting out a timeout.

**Reflection.** `start_symmetric` uses M=N, so an attacker can echo the sender's
own message back as a receiver's. It learns nothing (the sender derives `g^{x²}`,
which the attacker cannot compute) but without a guard it would burn a third of the
guess budget for two POSTs. The sender remembers every message it has posted for a
slot and refuses a byte-identical one **without** charging it as a guess.

> **Active-attack model (what a short code buys you).** The nameplate (the digit
> part) *is* the rendezvous slot, so an attacker can sweep the 10k-nameplate space
> to find an in-flight pairing, then race the real receiver with a *guessed* word
> pair. SPAKE2 makes that **one online guess per exchange** (1 in 65,536 with the
> 256-word list). Under v1 the slot burns on first fetch, so that is one guess,
> full stop. Under v2 a code is reusable, so the bound is instead **one guess per
> session, at most three per code**: the third wrong key confirmation retires the
> code outright, and the counter is persisted, so restarting the sender does not
> hand the budget back.
>
> The cost of that bound is a cheap grief: three hostile sessions kill a code, and
> the user is told so and has to make a new one. That is the same severity as v1's
> fetch-and-burn of `tkt`, so it is not a new class of problem — but it is now
> deliberate, and the error message says what happened. Failure stays **visible**
> either way: the honest receiver sees `409 CONFLICT`, a payload that won't decrypt,
> or a retired code — never a silent hijack. The worst realistic outcome is denial
> of service of that one pairing, not disclosure — the same model as
> magic-wormhole.
>
> **Nameplate exhaustion.** A v2 slot outlives its v1 ancestor and renews for free,
> which would make squatting all 10,000 nameplates a one-off purchase of 10k POSTs
> and deny `arvolo code` relay-wide. Two countermeasures: a per-IP budget on **new
> claims** (`ARVOLO_RZ_CLAIMS_PER_HOUR`, default 10), and an absolute lifetime no
> amount of renewing can pass (24h). Cancelling a code deletes its slot rather than
> leaving it squatted for the rest of the lease.

### 7.3 Relay backfill (sender may go offline)
When a relay is configured, the sender also seeds its chunks to its blob node
(`/v1/addr` → `/v1/seed`), and the ticket lists the relay as an **additional
provider**. The receiver fetches from whichever provider answers (chunk protocol
over iroh); if the sender drops, the relay finishes the transfer. Seeded chunks
are released/reaped afterwards.

### 7.4 Offline mailbox (recipient away)
```
sender:  arvolo send R file --deposit
  1. HPKE-seal the file to R (auth mode; optional Argon2id password wrap)
  2. POST /v1/deposit (encapped key + optional revoke-hash) → claim
  3. print arvm… (relay, claim, sender, salt) + save a local deposit session (§8)
receiver: arvolo recv arvm…
  1. GET /v1/fetch/{claim} (+ encapped-key header)
  2. optional password unwrap → HPKE-open with your identity (verifies sender)
  3. burn-after-read: the blob is gone once the download cap is reached
```
The sender can later confirm delivery via `/v1/entry/{claim}/status` (gone ⇒
fetched) or delete it early via `/v1/entry/{claim}` with the revoke token.

> Share the `arvm…` ticket over a reasonably private channel. Its contents are
> HPKE-sealed — only the recipient's key can ever decrypt — but the claim inside
> is the *fetch* capability: someone who intercepts the ticket can download the
> ciphertext (useless to them) and thereby **burn** the one-shot download before
> the recipient gets it. A denial-of-service, never a disclosure; prefer
> `arvolo send` over `listen`/pairing when no private channel exists.

### 7.5 Always-open client (presence + inbox)
The always-open model lets a sender **push** a file to a contact who is running
`listen`, delivering live when possible and via the mailbox otherwise.

**Presence.** A listening client periodically publishes a short-lived beacon to
`/v1/presence/{presence_slot}` (TTL ~30 s). A sender checks the beacon to decide
whether the recipient is online.

> **Presence caveat.** The beacon is unauthenticated in both directions, on
> purpose. *Reads*: anyone who knows your public id can compute your presence
> slot and poll it — running `listen` broadcasts an online/offline signal to
> every contact (and anyone they share your id with). Don't run `listen` if that
> metadata matters to you; content is unaffected either way. *Writes*: anyone
> can spoof "online" for a slot, but a fake beacon only makes a sender attempt a
> live offer that no one fetches — the two-phase watchdog (below) falls back to
> the mailbox on the real signal, and nobody can force you to *appear offline*
> (there is no beacon delete).

**Inbox read auth (proof of possession).** Reading an inbox must be limited to its
owner, but the relay has no account for them — and, deliberately, no way to name
them either. The slot *is* a public key (§6.2), so a signature settles it:

```
1. reader: POST /v1/inbox/{slot}/session   — no body
2. relay:  returns a random nonce + exp + a MAC over (slot, nonce, exp)
           (the challenge is public; the relay hands one to whoever asks)
3. reader: signs  "arvolo/inbox/session/v2" ‖ slot ‖ nonce ‖ exp
           with the *blinded* secret for that slot's epoch, and assembles a
           session token = (nonce, exp, mac, sig)
4. relay:  MAC → it issued this challenge for this slot; signature verified
           against the key the slot encodes → the presenter owns it
5. reader: long-polls GET /v1/inbox/{slot} with the token until it expires
```

Two checks doing two different jobs: the MAC makes the session stateless and
bounded, the signature proves ownership — which the MAC cannot, since step 2 is
public.

Earlier versions did this with HPKE instead: the reader sent its public key, the
relay sealed the nonce to it, and opening the seal was the proof. It worked, and it
handed the relay the long-term identity of every reader — from which every slot
that identity would ever have could be recomputed, in any epoch. The nonce survives
for freshness; the part that named you is gone, and with it the relay's need to
agree with anyone about what time it is.

**Offers (sealed sender).** A sender posts an **offer** to the recipient's
`inbox_slot`: a record of `{ name, size, chunks, ticket, note, sender_name }`
sealed in **two layers** —

1. an **inner auth-mode HPKE seal** to the recipient (AAD `arvolo/offer/v1`),
   which cryptographically proves *who* sent it, wrapped together with the
   sender's public id in
2. an **anonymous (base-mode) outer seal** to the same recipient (AAD
   `arvolo/offer/env/v1`), so the sender's public id never appears on the wire.

The relay therefore sees only *which slot* received an offer and when — never
who sent it (the same goal as Signal's sealed sender). The recipient unwraps the
outer layer, reads the sender id, and verifies it against the inner auth-mode
seal; a forged sender fails to open. The POST also carries a **poster token**
(hash stored) so the sender can retract the offer or ask how far it got. The
recipient's `listen` surfaces each offer (sender, name, size) and, on accept, runs
the underlying transfer (an `arvc` ticket over P2P, or an `arvm` fetch)
transparently.

**How far an offer got, and what each answer can support.** Three states, because
three different things are true at different moments and only one of them is about
a person:

| state | set by | what it supports |
|---|---|---|
| `pending` | the POST | nothing has read it |
| `fetched` (`Arrived`) | *any* authenticated read of the slot | the offer reached one of their devices. An `arvolo recv`/`status` listing sets this exactly as their daemon's poll does, so it can never mean "they have the file" — nor even "they looked" |
| `taken` | the recipient's DELETE, sent once the file is saved | they took it — the only state that reports a person acting |

The ack does not delete the row; it leaves a **tombstone** (payload dropped,
`expires_at` untouched, reaped on the offer's own TTL). Without it, `taken` and
"lapsed unread" were the same answer, `gone`, and the sender's only positive signal
was `fetched` — which is why that one had to stop being read as a delivery. The
wire keeps the word `fetched` for the middle state so that clients older than
`taken` keep reading a newer relay correctly; internally it is called `Arrived`.

There is no state for *a person saw it*, deliberately: the relay observes reads of
a slot, never attention, and only the recipient's own client could claim otherwise.
So no name here may imply one — which is also why the middle state is not called
`Seen` (it claims a person) or `Received` (in a tool whose verb for taking a file
is `recv`, it is the one word certain to be read as the state it isn't). `gone` now means only: retracted, or lapsed without being
taken (as it also does for an offer taken on a relay that predates `taken`).

**Two-phase watchdog (`arvolo send`).** `arvolo send` decides live-vs-mailbox on a real
signal rather than a blind timer:

```
phase 1 (~12 s): did a live reader fetch the offer? (poll offer status)
  no  → nobody is really listening → fall back to the mailbox immediately
  yes → the recipient is live
phase 2 (~90 s): wait for the receiver to connect and serve P2P to completion;
  if it never connects → fall back to the mailbox
```
Mailbox fallback deposits the file as in §7.4 and the sender can confirm delivery
later via claim status.

### 7.6 Browser download link
`arvolo link` produces a URL any browser can open — no arvolo, no
account — while keeping the relay zero-knowledge.

**Container.** [`core/src/link.rs`](../core/src/link.rs) encrypts the file into a
self-describing container under a fresh random 32-byte key, chunked with the same
AES-256-GCM scheme as §2.3 (1 MiB chunks). Little-endian layout:

```
[8]   magic "ARVLNK01"
[4]   chunk_size (u32)
[8]   total_size (u64)          # also fixes the chunk count
[4]   meta_len (u32)
[meta_len] meta_ct              # AES-256-GCM of the UTF-8 file name, sealed as the
                                #   reserved chunk index 0xFFFFFFFF (never a data chunk)
repeat for i in 0..ceil(total_size/chunk_size):
  [4]   ct_len (u32)
  [ct_len] chunk_ct             # seal_chunk(key, i, total_chunks, plaintext_chunk)
```
The **file name is encrypted** (in `meta_ct`), so the relay learns only the
ciphertext length. The blob is deposited via `POST /v1/deposit` with an **empty**
encapped-key header (there is no HPKE recipient) and an optional revoke-hash.

**Delivery.** The relay serves `/dl/{claim}` (static, self-contained, strict CSP —
`default-src 'none'`, only same-origin script/worker/fetch). The page:

```
1. reads the 32-byte key from location.hash (never sent to the server)
2. imports it as an AES-GCM key via WebCrypto
3. fetch()es /v1/fetch/{claim} as a stream, parses the container header
4. decrypts meta → shows the real file name and size
5. decrypts each chunk (iv = index, additionalData = index‖total) and writes it to
   a sink, streaming to disk without buffering the whole file where possible:
     - File System Access API (Chromium), or
     - a same-origin service worker with back-pressure (Firefox/Edge)
   otherwise it buffers in memory and shows a "not fully compatible" notice.
```
Because the key is only in the fragment and decryption is client-side, an **honest**
relay serves the page and the ciphertext but never sees the key or plaintext.

> **Trust caveat (honest-relay assumption).** Unlike the CLI paths (`arvc` / `arvm` /
> `--to`), where the code that touches the key runs on your own machine, the browser
> link path runs JavaScript **served by the relay itself**. A *malicious* relay could
> serve a modified `dl.js` that reads the fragment key from `location.hash` and posts
> it back — the CSP (`connect-src 'self'`) still permits requests to the relay's own
> origin. So the browser link is zero-knowledge only against an **honest** relay; it
> is not a defense against a hostile relay operator, exactly like Firefox Send and any
> other "server serves the decryptor" scheme. **For confidentiality against a hostile
> relay, use a recipient-sealed send (`--to`)**, whose key never reaches the relay.

**Download caps.** A link has **no download limit by default** (`max` unlimited),
so it works for many recipients and tolerates retries; `--max N` sets a burn count.
A link expires only when its session is removed (§8) or the TTL lapses.

**Disabling links per relay.** A relay administrator can turn public download
links off entirely by starting the relay with `ARVOLO_DISABLE_LINKS=1`. Then the
relay (a) reports `{"links":false}` on `/v1/features`, (b) serves `403` for the
`/dl` page, script, and service worker, and (c) refuses HPKE-less (link) deposits
with `403`. The client checks `/v1/features` **before** encrypting, so
`arvolo link` fails immediately with a message explaining the
administrator disabled the feature; recipient-sealed sends (`--to`) are
unaffected. An older relay without `/v1/features` is treated as allowing links.

---

## 8. Lifecycle, deposits, and expiry

Every relay deposit (a sealed `arvm` ticket **or** a public link) is recorded
locally ([`cli/src/deposits.rs`](../cli/src/deposits.rs), 0600 — it holds the
revoke token) with its relay, claim, name, size, download cap, and expiry. This
happens whoever made it: the one-shot CLI writes the record directly, and the
engine's front-ends write it from the `Deposited` event (the engine itself sits
below this store), so a mailbox send is listed and withdrawable whether or not a
daemon was running.

`arvolo status` lists them under **left on relay**, each with a **live relay
status** (polls `/v1/entry/{claim}/status`): `present`, `gone (downloaded /
revoked)`, or `unknown (relay unreachable)`, plus whether it has locally expired.
The local record is a *receipt*, not a status — nothing reports a download back —
so the status is asked of the relay when the list is built, and an unreachable
relay reads as `unknown` rather than inventing an answer.

A blob is deleted when the **first** of these happens:

1. **Download cap reached** — burn-after-read (`--max`; 1 for a sealed send,
   unlimited for a link).
2. **TTL lapses** — the relay's reaper deletes expired entries (default 7 days for
   a deposit; capped at 30 days).
3. **Revoke** — `arvolo cancel <id>` (or `revoke <arvm…|link>`, for one sent from
   another machine) sends the revoke token to `DELETE /v1/entry/{claim}` **before**
   dropping the local record, so taking a deposit back deletes the file and kills
   the link. A deposit the engine made is withdrawn *through* the engine, which
   also retracts the offer it left in the recipient's inbox — otherwise they'd be
   pointed at a blob that is no longer there.

The **local record** is swept an hour after its TTL, when the list is next built.
Expiry is the only trigger: the record exists to hold the revoke token, so it dies
with the blob, and of the three ways a blob dies only the TTL can be known locally,
for free, and for certain. A download proves nothing (a link is unlimited by
default; even a sealed send takes `--max N`), and an unreachable relay's silence
must never be read as absence. Keeping a dead record costs a stale row; binning a
live one costs the token — so the sweep waits for the one signal that can't be
wrong, with an hour's slack.

No clock is exchanged with the relay, and none is needed: a TTL is a duration, so
both sides deadline at `created + ttl` from the same moment by their own clock, and
a standing offset cancels. The hour of slack covers the residue — our clock drifting
or jumping (an NTP step, a suspended VM) *between* the deposit and the check.

Rendezvous slots, inbox offers, and presence beacons each have their own short
TTLs and row caps (abuse/disk-fill guards); see the constants in
[`relay/src/lib.rs`](../relay/src/lib.rs).

---

## 9. Security properties (summary)

| Property | Mechanism |
|---|---|
| End-to-end confidentiality | HPKE / AES-256-GCM; relay & network see only ciphertext |
| Integrity & tamper-evidence | AEAD tag on every chunk + BLAKE3 content addressing |
| Sender authentication | HPKE **auth mode** binds the sender's identity (contact/offline sends) |
| Zero-knowledge relay | Content key is out-of-band (ticket / URL fragment); relay stores opaque ciphertext and derived slots. *Caveat:* the **browser download link** trusts the relay to serve honest `dl.js` — see §7.6 and §9.1(5). CLI paths do not. |
| Sender privacy vs relay | Inbox offers are **sealed-sender**: the sender's id travels only inside an anonymous outer seal to the recipient, so the relay never sees who is sending — only which slot receives (§7.5). |
| Presence privacy (opt-in surface) | Running `listen` publishes an online beacon readable by anyone who knows your public id; spoofable "online", unforgeable "offline" — see the caveat in §7.5. |
| Reorder/truncation resistance | Per-chunk AAD binds `index ‖ total` |
| Short-code safety | SPAKE2 PAKE — no offline dictionary attack |
| Inbox read authorization | Signature by the slot's blinded key → relay-MAC session token. The relay never receives a public key, so it cannot link a reader's slots across epochs. |
| Link-leak resistance (opt-in) | Argon2id password wrap the relay cannot bypass |
| Revocability | Revoke token (hash-only at the relay), constant-time checked |
| Data minimization | Dial keys not IPs; random claims; derived slots that **rotate** per epoch (§6.2); TTL auto-expiry; no PII in identities |
| Client address vs relay (opt-in) | `proxy` / `ARVOLO_PROXY` routes the **whole** relay HTTP surface through a proxy (`socks5h://…` for Tor), so the relay logs the exit's address. Fails closed: a misconfigured proxy makes requests fail rather than go direct. Does **not** cover P2P (QUIC) — see §9.1(2). |

### 9.1 Honest boundaries (and why they are universal)

The points below can read like weaknesses, but they are **not specific to
Arvolo** — they are fundamental limits that *every* end-to-end encrypted system
shares, because cryptography solves some problems and provably cannot solve
others. We state them plainly instead of implying magic. See also
[`TECHNICAL-OVERVIEW.md`](TECHNICAL-OVERVIEW.md) §5.

**1. A key proves math, not a person (key ↔ identity binding).**
Cryptography guarantees "only the holder of this private key can decrypt." It
cannot, by itself, prove that key belongs to the human you think it does.

- *Example:* Alice sends you her id `if2x…`. A man-in-the-middle could instead
  hand you *his* key while claiming to be Alice; every message would still be
  "perfectly encrypted" — just to the wrong person.
- *Same everywhere:* this is the classic public-key authentication problem.
  **Signal** makes you compare **safety numbers**, **WhatsApp** a **security
  code**, **PGP** uses key-signing / the web of trust, and browsers rely on
  certificate authorities. All of them default to trust-on-first-use in practice.
- *Not solvable by math:* the only fix is an **out-of-band trust anchor** — a
  channel the attacker doesn't control. Arvolo gives you the smallest possible
  one: an **eight-word fingerprint** (64 bits, §2.7) you compare once (in person,
  by phone), plus
  **key-change warnings** if a saved contact's key ever changes (exactly Signal's
  model). There is no cryptographic way to remove this step.

**2. The relay sees sizes, timing and your address (metadata), never content.**
The relay only ever holds opaque ciphertext — no plaintext, no keys, not even your
public id (it sees a derived slot). But it unavoidably sees *how big* a blob is,
*when* it moves, and *what address* it came from.

- *Example:* an operator can note "a ~4 MB blob appeared at 15:03 and was fetched
  at 15:07" and infer activity patterns, without ever reading a byte.
- *The address is not incidental:* a public relay **must** group requests by
  client IP — that is what the per-IP rate limits are — and any reverse proxy in
  front logs it besides. Two things reduce it. Slots are **per-epoch blinded keys**
  (§6.2), so a request path carries no identifier that outlives an epoch, and the
  relay is never told a public key it could use to link them — the handshake that
  used to carry one is gone (§7.5). And `proxy` in `config.toml` (or
  `ARVOLO_PROXY`) routes every relay request through a proxy —
  `socks5h://127.0.0.1:9050` puts the whole HTTP surface over Tor.

  What remains, and it is not small: the relay still sees **when** you poll and
  **from where**, and one address polling one slot for a week is a thread of its
  own. Rotation ends that thread at each boundary; the proxy cuts the address; only
  both together leave the relay with sizes and timing. And the proxy cannot carry
  **P2P** traffic, which is QUIC/UDP.
- *The P2P path has its own three knobs,* because a direct transfer reveals your
  address to the peer by construction and no proxy can change that:
  `iroh_relay` (your own NAT relay, or `off` for none), **`iroh_discovery`**, and
  `p2p`. The discovery one is the least obvious and leaks the most: iroh's n0
  preset installs a *publisher* that writes a signed `EndpointId → your addresses`
  record into a third party's DNS, refreshed as you move networks — a stable public
  key tied to a moving address, held by someone else. Arvolo never needs it to dial
  (every connect takes a full `EndpointAddr` from a ticket or the swarm tracker),
  so `iroh_discovery = "resolve"` drops the publishing and keeps the lookup; the
  one thing it costs is re-serving an *old* ticket after your address changed
  without a NAT relay to route by id. And `p2p = false` is the end of the line:
  everything goes through the mailbox, which is HTTP, hence proxyable — the only
  configuration where neither the relay nor the recipient learns your address.
- *Same everywhere:* **Signal**'s servers still see message timing, size, and IPs
  (Sealed Sender hides the *sender label*, not the timing); **HTTPS/TLS** leaks the
  size and timing of every page you load; **Tor** exists precisely because hiding
  metadata is a separate, much harder problem than hiding content.
- *Not solvable for free:* the only defenses are heavy **traffic padding + cover
  traffic + mixing**, which cost enormous bandwidth and latency and are out of
  scope for a file-transfer tool. The mitigation you *do* have is structural:
  **self-host the relay**, so even this thin metadata stays on your own
  infrastructure and never reaches a third party.

**3. A compromised endpoint is game over (for everyone).**
Arvolo protects data **in transit** and **at rest on the relay**. It cannot
protect a device that is already compromised.

- *Example:* if malware or a keylogger is running on the sender's or recipient's
  machine, the file is plaintext there by necessity — it has to be, for the user
  to open it.
- *Same everywhere:* malware on your phone defeats **Signal**, **WhatsApp**, and
  **PGP** identically — the decrypted message is on the screen. No end-to-end
  system claims otherwise.
- *Not solvable by a transfer protocol:* endpoint security (full-disk encryption,
  OS hardening, not running malware) is a different layer. E2E encryption secures
  the channel; it was never meant to secure a broken endpoint, and no protocol can.

**4. A public link is a bearer capability (by design).**
A download link (`…/dl/<claim>#<key>`) carries its key in the URL. Anyone with the
**full** link can download the file — the link *is* the credential, exactly as you
asked for when you chose a "share by link" flow.

- *Example:* paste the link into an insecure chat and whoever reads that chat can
  fetch the file, just like leaking a password.
- *Same everywhere:* this is true of **Firefox Send**, **Dropbox / Google Drive
  "anyone with the link"**, and **magic-wormhole** codes — any "no account needed"
  share is a bearer token by definition. (A recipient-sealed `arvm` send, by
  contrast, can *only* be opened by one identity's key.)
- *Mitigations (not a fix, a choice):* set **`--max N`** (burn after N downloads),
  a short **`--ttl`**, and **`arvolo cancel <id>`** to revoke it the moment
  you're done — controls most link services don't even offer.

**5. The browser download link trusts the relay to serve honest code.**
The `--link` path is the *only* one where the code that handles the key runs on a
page **served by the relay** (`dl.js`). The key lives in the URL fragment, so an
*honest* relay never sees it — but a *malicious* relay could serve a modified `dl.js`
that exfiltrates `location.hash` to its own origin (permitted by `connect-src 'self'`).

- *Example:* an operator who wants your plaintext replaces the download script; every
  link opened against that relay leaks its key, while looking identical to the user.
- *Same everywhere:* this is inherent to "the server serves the decryptor" — **Firefox
  Send** had the exact caveat; any web-based E2E download does. Only an out-of-band,
  independently-obtained client (a native app / extension) removes it.
- *Not solvable for the browser path:* the relay serves the page, the script, and the
  ciphertext, so SRI / pinning can't help (the attacker controls all three). **The fix
  is to choose the channel:** a recipient-sealed send (`--to`) keeps the key entirely
  off the relay and is safe even against a hostile operator; reserve `--link` for when
  the relay is one you trust (e.g. self-hosted).
