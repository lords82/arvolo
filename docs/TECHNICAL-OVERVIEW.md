# Arvolo — Technical Overview

Why Arvolo is a **sound security design** and a **convenient** way to send files.
This document covers the **open-core** product (the public AGPL-3.0 repo) only:
the client (`arvolo`) and the single self-hostable relay (`arvolo-relay`).

> Managed-identity and multi-relay features (directory, SSO, federation, key
> transparency) are **not** covered here — they are part of the separate
> commercial edition. See [`COMMERCIAL.md`](COMMERCIAL.md).

---

## 1. In one paragraph

Arvolo sends files **P2P-first** — directly between two devices when both are
online — and falls back to a **self-hostable, expiring, zero-knowledge relay**
when the recipient is away. Every transfer is **end-to-end encrypted**: the relay
and the network only ever see ciphertext. Your data never leaves your
infrastructure, or the EU, and there is no third-party cloud in the middle that
can read, retain, or be compelled to hand over your files.

---

## 2. Architecture at a glance

```
  sender  ───────── direct P2P (LAN or hole-punched) ─────────▶  receiver
    │  (both online: data flows directly, relay untouched)          ▲
    │                                                               │
    │  receiver away? deposit ciphertext                claim later │
    ▼                                                               │
  ┌──────────────────────────────────────────────────────────────────┐
  │  arvolo-relay  —  zero-knowledge, opaque ciphertext + TTL reaping  │
  │  (self-hosted; only ever holds ciphertext, never plaintext/keys)   │
  └──────────────────────────────────────────────────────────────────┘
```

- **Transport**: [iroh](https://iroh.computer) behind a `Transport` trait
  ([`core/src/lib.rs`](../core/src/lib.rs)) — "dial keys, not IPs". P2P-first:
  a direct/hole-punched path when possible, a live-forwarding relay path (no
  storage) when direct fails, and store-and-forward only as the last resort.
- **Content addressing**: every blob/chunk is identified by its **BLAKE3** hash.
  The hash *is* the manifest, which makes resume and multi-source fetch
  source-agnostic and self-verifying ([`core/src/lib.rs`](../core/src/lib.rs)).
- **Chunked streaming**: files are split into 16 MiB content-addressed chunks
  ([`core/src/chunked.rs`](../core/src/chunked.rs)) served over P2P, with a
  bidirectional control channel for acks and relay coordination.
- **Engine isolation**: iroh lives behind a trait, so the crypto/mailbox/TTL
  logic never depends on the networking engine and stays swappable.

---

## 3. Cryptographic design

### 3.1 Contact sends — HPKE auth mode (RFC 9180)
[`core/src/crypto.rs`](../core/src/crypto.rs) encrypts toward the recipient's
public key **and** binds the sender's identity, using HPKE in **auth mode**:

- Ciphersuite: **X25519-HKDF-SHA256** KEM, **HKDF-SHA256** KDF,
  **ChaCha20-Poly1305** AEAD.
- Auth mode means the recipient cryptographically learns *who* sent the payload —
  closing the gap of encrypt-only schemes (e.g. plain `age`). A wrong sender fails
  to open; a wrong recipient cannot open; tampering is rejected (AEAD tag).
- An identity is a long-term X25519 keypair; the public part is the contact id
  others encrypt toward — **no PII**.

### 3.2 Chunk stream — per-transfer content key
The chunked path (`arvc…` tickets) is an **ephemeral capability model**: whoever
holds the ticket may receive. To keep the relay zero-knowledge, each chunk is
sealed independently under a **fresh random 32-byte content key** that travels
only inside the ticket (out-of-band), with **ChaCha20-Poly1305**:

- The nonce is derived from the chunk index; the key is fresh-random per transfer,
  so each `(key, nonce)` pair is used exactly once.
- The AAD binds the chunk's **index and the total count**, so reordering or
  truncation is rejected on open.
- Every ciphertext chunk is self-verifying (AEAD tag) and addressed by its BLAKE3
  hash → out-of-order multi-source fetch and resume stay safe.

### 3.3 Optional link password — Argon2id outer wrap
An optional outer layer so that **holding the link is not enough** to decrypt
([`core/src/crypto.rs`](../core/src/crypto.rs)): the payload is additionally
wrapped under a key derived from a shared password via **Argon2id**. The salt
travels in the ticket (not secret); the password is shared out-of-band. Without
the password the inner ciphertext cannot be recovered — so **even the relay can
never bypass it**, and neither can someone who merely intercepts the link.

### 3.4 Short-code pairing — SPAKE2 PAKE
Instead of copying a ~1000-char ticket, the sender shows a short human code like
`4821-crater-mango` ([`core/src/code.rs`](../core/src/code.rs)). The ticket is
exchanged over a relay **rendezvous** protected by a **SPAKE2** PAKE keyed on the
code:

- The relay only ever sees PAKE messages and the **encrypted** ticket → it stays
  zero-knowledge.
- A short code is safe: a PAKE admits **no offline dictionary attack**; a wrong
  code simply derives a different key and fails to decrypt.

### 3.5 Human-verifiable fingerprint
Each identity has an **eight-word fingerprint** derived via BLAKE3 from the public
key (**64 bits**) ([`core/src/crypto.rs`](../core/src/crypto.rs)). It is a display
aid for **out-of-band** verification ("read me your eight words"); the full base32
id remains authoritative. (Widened from six words / ~48 bits, which an active MITM
could brute-force a matching keypair against.)

---

## 4. The zero-knowledge relay

[`relay/src/lib.rs`](../relay/src/lib.rs) is store-and-forward that holds **opaque
ciphertext** only — never plaintext or keys:

- Blobs are addressed by a **random claim token**, each with a **TTL** after which
  it is reaped automatically. Metadata in SQLite, ciphertext as files on disk;
  survives restarts.
- **Burn-after-read**: a per-deposit **max-downloads** cap (capped at 10,000).
- **Abuse/DoS guards** (v0.2.0): caps on blob size (default 256 MiB), stored
  entries, per-deposit TTL (default 30 days, prevents immortal entries and integer
  overflow), rendezvous value size and row count, and the seeded-chunk footprint.
- **Self-hostable** in one command (Docker), so you keep 100% control of where
  ciphertext rests.

Because the content key travels out-of-band (in the ticket) and the relay stores
only ciphertext, a relay operator — or anyone who compromises the relay — learns
**nothing** about your file contents.

---

## 5. Security properties (what you get)

| Property | How it is achieved |
|---|---|
| **End-to-end confidentiality** | HPKE / ChaCha20-Poly1305; relay & network see only ciphertext |
| **Integrity & tamper-evidence** | AEAD tags on every chunk + BLAKE3 content addressing |
| **Sender authentication** | HPKE **auth mode** binds the sender's identity |
| **Zero-knowledge relay** | Content key is out-of-band; relay stores opaque ciphertext only |
| **Reorder/truncation resistance** | Per-chunk AAD binds index + total count |
| **Short-code safety** | SPAKE2 PAKE — no offline dictionary attack |
| **Link-leak resistance (opt-in)** | Argon2id password wrap the relay cannot bypass |
| **Data minimization** | Dial keys not IPs; random claim tokens; TTL auto-expiry; no PII in identities |
| **Data sovereignty** | Self-hosted relay; ciphertext never leaves your infra / the EU |

### Honest threat-model boundaries
- **Key ↔ person binding.** Cryptography guarantees *only the key holder can
  decrypt*; it does **not** by itself prove the key belongs to the right person.
  In the open core you close this with **one out-of-band fingerprint check** (TOFU
  + Signal-style key-change warnings). See
  [`IDENTITY-VERIFICATION.md`](IDENTITY-VERIFICATION.md).
- **Metadata at the relay.** A relay operator can still observe ciphertext
  **sizes and timing**. Contents, sender, and recipient identities are not exposed
  by the payload, but traffic analysis is out of scope for the MVP.
- **Endpoint trust.** Arvolo protects data in transit and at rest on the relay,
  not a compromised sender/receiver device.

---

## 6. Why it is convenient to use

- **P2P-first, no middleman.** When both sides are online the data flows directly
  — nothing transits a third party at all. A relay is used only to bootstrap the
  code exchange or to hold ciphertext while the recipient is away.
- **Short, shareable codes.** `4821-crater-mango@relay.example.com` instead of a
  giant ticket; with a configured default relay, just `4821-crater-mango`. Or a
  fully self-contained `arvc…` ticket with **no relay at all**.
- **Resumable, robust transfers.** Content addressing + per-chunk verification
  give free resume, out-of-order/multi-source fetch, and anti-double-send between
  the direct and relay paths.
- **Self-host in one command.** `docker run … arvolo-relay` — your servers, your
  datacenter, EU soil. No vendor in the middle, no US SaaS, nothing to subpoena
  under the CLOUD Act; GDPR data-residency requirements become straightforward.
- **Scriptable.** A clean CLI (with an expiring zero-knowledge mailbox, folders,
  and send-to-a-contact) makes machine-to-machine transfer in CI/CD pipelines a
  first-class use case, not an afterthought.

---

## 7. Status & scope

Working CLI (v0.x): P2P + relay-backfill transfer with resume, per-chunk E2E
encryption, short pairing codes, send-to-a-contact, folders, and an expiring
zero-knowledge mailbox. Desktop GUI, browser link-mode, and further hardening are
on the open-core [`ROADMAP-FUTURE.md`](ROADMAP-FUTURE.md).

Everything in this document is open-core and self-hostable. For the commercial
edition boundary, see [`COMMERCIAL.md`](COMMERCIAL.md) and
[`../LICENSING.md`](../LICENSING.md).
