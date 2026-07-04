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
| **Identity** | A long-term X25519 keypair. The public half is your **id** (no PII). |
| **Contact id** | Another party's X25519 public key, base32-encoded. What you send **to**. |
| **Fingerprint** | A six-word BLAKE3 digest of a public id, for out-of-band verification. |
| **Ticket** | A self-describing capability string that lets the holder receive a transfer (`arvc…` / `arvm…`). |
| **Claim** | A random 16-byte capability token addressing one blob on the relay. |
| **Slot** | An opaque per-identity address on the relay (inbox / presence), derived from a public id so the relay never sees the id. |
| **Content key** | A fresh random 32-byte AEAD key per transfer; encrypts the chunk stream; travels only in the ticket. |
| **Relay** | The self-hostable, zero-knowledge store-and-forward + rendezvous server. It only ever sees ciphertext and opaque tokens. |

---

## 2. Cryptographic primitives

All defined in [`core/src/crypto.rs`](../core/src/crypto.rs).

### 2.1 Identity — X25519
An identity is an X25519 keypair. The 32-byte public key is the contact id;
the 32-byte secret is stored locally (0600 on unix). There is no certificate,
account, or PII — the key **is** the identity.

### 2.2 Contact encryption — HPKE auth mode (RFC 9180)
Sends addressed to a specific recipient use **HPKE** with:

- **KEM** X25519-HKDF-SHA256, **KDF** HKDF-SHA256, **AEAD** AES-256-GCM.
- **Auth mode**: the sender's static key is mixed in, so the recipient
  cryptographically learns *who* sent the payload. A wrong sender fails to open,
  a wrong recipient cannot open, and tampering is rejected by the AEAD tag.
- **Info string** `arvolo/hpke/v1` domain-separates the KDF.
- **Base mode** (`seal_anon`/`open_anon`) is used where no sender identity is
  proven — currently the inbox proof-of-possession challenge (§7.5).

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
`PublicId::fingerprint()` is six words derived from BLAKE3 of the public key
(~48 bits), a display aid for out-of-band verification. The full base32 id remains
authoritative for matching contacts.

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
Produced by `arvolo send`. A base32 postcard of
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
Produced by `arvolo send --to` (mailbox path). A base32 postcard of
[`OfflineTicket`](../core/src/offline.rs): the `relay` URL, the `claim`, the
`sender` public id, and a password `salt` (empty if no `--password`). The file is
HPKE-sealed **directly** to the recipient (no separate content key); the relay
stores that ciphertext under `claim`.

### 4.3 Pairing code — `N-word-word[@relay]`
A leading digit plus two words from a fixed wordlist, optionally `@relay` so the
receiver needs no configuration. It is **not** the ticket; it is the SPAKE2
password used to fetch the ticket from a relay rendezvous (§7.2).

### 4.4 Download link — `https://<relay>/dl/<claim>#<key>`
Produced by `arvolo send --link`. The `claim` addresses a relay blob (a
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

A global request-body limit (default 256 MiB, `ARVOLO_MAX_BLOB_BYTES`) wraps every
route so no handler can buffer an unbounded body.

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
| POST / GET | `/v1/rz/{slot}/{key}` | none | **Rendezvous** for short-code pairing: the sender PUTs its SPAKE2 message and the encrypted ticket; the receiver GETs them. Values are size-capped; slots TTL out. |
| POST / GET | `/v1/inbox/{slot}` | session (GET) | Post a sealed **offer** to a recipient's inbox / long-poll for offers. |
| POST | `/v1/inbox/{slot}/session` | proof-of-possession | Start a read session: returns a nonce sealed to the slot owner + a relay MAC over `(slot, nonce, exp)`. |
| DELETE | `/v1/inbox/{slot}/{id}` | poster token | Retract your own offer. |
| GET | `/v1/inbox/{slot}/{id}/status` | poster token | `pending` vs `fetched` (was the offer picked up by a live reader?). |
| POST / GET | `/v1/presence/{slot}` | none | Publish / read a **presence beacon** (is this identity online right now?). |
| GET | `/dl/{claim}` | none | The browser **download page** (static HTML, strict CSP). |
| GET | `/dl.js`, `/arvolo-sw.js` | none | The download script and streaming service worker (embedded in the binary). |
| GET | `/v1/features` | none | Advertise optional features so a client can fail fast, e.g. `{"links":true}`. |
| GET | `/healthz` | none | Liveness. |

### 6.2 Slots (opaque per-identity addresses)
So the relay never sees a public id, inbox and presence are addressed by a
**derived slot**:

```
inbox_slot    = base32( blake3_derive_key("arvolo/inbox/slot/v1",    pubkey) )
presence_slot = base32( blake3_derive_key("arvolo/presence/slot/v1", pubkey) )
```

A slot is a stable, opaque handle: anyone who knows your public id can compute
your slot (to send you an offer or check presence), but the relay cannot invert it
to a public id.

### 6.3 Capability tokens
- **claim** — 16 random bytes; the bearer capability for one blob.
- **revoke token** — sender-held secret; the relay stores only its BLAKE3 hash and
  compares in constant time on DELETE.
- **poster token** — retract/status secret for a posted inbox offer; same
  hash-only, constant-time pattern.
- **session token** — a relay MAC (keyed by a per-process secret) over
  `(slot, nonce, exp)`, presented to authorize inbox reads for a while (§7.5).

---

## 7. Protocol flows

### 7.1 P2P transfer (ticket)
```
sender:  arvolo send file           → prints arvc… (providers = sender's iroh addr)
receiver: arvolo recv arvc…
  1. parse ticket: chunk hashes, providers, key delivery
  2. if Sealed → HPKE-open the content key with your identity (learns sender)
  3. dial a provider by key (arvolo/chunk/1); request each chunk by BLAKE3 hash
  4. verify hash, AES-256-GCM-open (nonce=index, aad=index‖total), write in place
  5. resume across and within chunks on interruption; unpack if archive
```
Both online ⇒ data flows **directly**; the relay is never touched.

### 7.2 Short-code pairing (SPAKE2 over rendezvous)
```
sender:  arvolo send --code --relay R file  → 4821-crater-mango[@R]
  - run SPAKE2 (password = code); PUT the sender message + the SPAKE2-encrypted
    ticket to /v1/rz/{slot}/{key}
receiver: arvolo recv 4821-crater-mango[@R]
  - GET the rendezvous, finish SPAKE2, decrypt the ticket, then proceed as §7.1
```
The relay sees only PAKE messages and the encrypted ticket; a wrong code derives a
different key and simply fails — no offline dictionary attack.

### 7.3 Relay backfill (sender may go offline)
With `--seed-relay`, the sender also seeds its chunks to the relay's blob node
(`/v1/addr` → `/v1/seed`), and the ticket lists the relay as an **additional
provider**. The receiver fetches from whichever provider answers (chunk protocol
over iroh); if the sender drops, the relay finishes the transfer. Seeded chunks
are released/reaped afterwards.

### 7.4 Offline mailbox (recipient away)
```
sender:  arvolo send file --to R --ticket
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

### 7.5 Always-open client (presence + inbox)
The always-open model lets a sender **push** a file to a contact who is running
`listen`, delivering live when possible and via the mailbox otherwise.

**Presence.** A listening client periodically publishes a short-lived beacon to
`/v1/presence/{presence_slot}` (TTL ~30 s). A sender checks the beacon to decide
whether the recipient is online.

**Inbox read auth (proof of possession).** Reading an inbox must be limited to its
owner, but the relay has no account for them. Instead of a signature it uses a
one-time **proof-of-possession** handshake, then a cheap session token:

```
1. reader: POST /v1/inbox/{slot}/session
2. relay:  seals a random nonce to the slot's public key (base-mode HPKE) and
           returns it + a MAC over (slot, nonce, exp) + exp   [context arvolo/inbox/session/v1]
3. reader: HPKE-opens the seal with its identity → recovers the nonce → assembles
           a session token; only the key owner can do this
4. reader: long-polls GET /v1/inbox/{slot} with the session token until it expires
```

**Offers.** A sender posts an **offer** to the recipient's `inbox_slot`: a sealed
record of `{ name, size, chunks, ticket }`, plus a **poster token** (hash stored)
so the sender can retract it or query whether a live reader fetched it. The
recipient's `listen` surfaces each offer (sender, name, size) and, on accept,
runs the underlying transfer (an `arvc` ticket over P2P, or an `arvm` fetch)
transparently.

**Two-phase watchdog (`send --to`).** `arvolo send --to` decides live-vs-mailbox on a real
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
`arvolo send --link` produces a URL any browser can open — no arvolo, no
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
Because the key is only in the fragment and decryption is client-side, the relay
serves the page and the ciphertext but **never** sees the key or plaintext — the
same end-to-end property as every other path.

**Download caps.** A link has **no download limit by default** (`max` unlimited),
so it works for many recipients and tolerates retries; `--max N` sets a burn count.
A link expires only when its session is removed (§8) or the TTL lapses.

**Disabling links per relay.** A relay administrator can turn public download
links off entirely by starting the relay with `ARVOLO_DISABLE_LINKS=1`. Then the
relay (a) reports `{"links":false}` on `/v1/features`, (b) serves `403` for the
`/dl` page, script, and service worker, and (c) refuses HPKE-less (link) deposits
with `403`. The client checks `/v1/features` **before** encrypting, so
`send --link` fails immediately with a message explaining the
administrator disabled the feature; recipient-sealed sends (`--to`) are
unaffected. An older relay without `/v1/features` is treated as allowing links.

---

## 8. Lifecycle, sessions, and expiry

Every relay deposit (a sealed `arvm` ticket **or** a public link) is recorded as a
local **deposit session** ([`cli/src/deposits.rs`](../cli/src/deposits.rs), 0600 —
it holds the revoke token) with its relay, claim, name, size, download cap, and
expiry. `arvolo sessions list` shows each with a **live relay status** (polls
`/v1/entry/{claim}/status`): `present`, `gone (downloaded / expired / revoked)`, or
`unknown (relay unreachable)`, plus whether it has locally expired.

A blob is deleted when the **first** of these happens:

1. **Download cap reached** — burn-after-read (`--max`; 1 for a sealed send,
   unlimited for a link).
2. **TTL lapses** — the relay's reaper deletes expired entries (default 7 days for
   a deposit; capped at 30 days).
3. **Revoke** — `arvolo sessions rm <id>` (or `revoke` / `revoke-link`) sends the
   revoke token to `DELETE /v1/entry/{claim}` **before** dropping the local record,
   so removing the session deletes the file and kills the link.

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
| Zero-knowledge relay | Content key is out-of-band (ticket / URL fragment); relay stores opaque ciphertext and derived slots |
| Reorder/truncation resistance | Per-chunk AAD binds `index ‖ total` |
| Short-code safety | SPAKE2 PAKE — no offline dictionary attack |
| Inbox read authorization | HPKE proof-of-possession → relay-MAC session token |
| Link-leak resistance (opt-in) | Argon2id password wrap the relay cannot bypass |
| Revocability | Revoke token (hash-only at the relay), constant-time checked |
| Data minimization | Dial keys not IPs; random claims; derived slots; TTL auto-expiry; no PII in identities |

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
  one: a **six-word fingerprint** you compare once (in person, by phone), plus
  **key-change warnings** if a saved contact's key ever changes (exactly Signal's
  model). There is no cryptographic way to remove this step.

**2. The relay sees sizes and timing (metadata), never content.**
The relay only ever holds opaque ciphertext — no plaintext, no keys, not even your
public id (it sees a derived slot). But it unavoidably sees *how big* a blob is
and *when* it moves.

- *Example:* an operator can note "a ~4 MB blob appeared at 15:03 and was fetched
  at 15:07" and infer activity patterns, without ever reading a byte.
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
  a short **`--ttl`**, and **`arvolo sessions rm <id>`** to revoke it the moment
  you're done — controls most link services don't even offer.
