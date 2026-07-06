# arvolo — Recipient identity verification (open core)

> **The question this document answers:** *how do I know that whoever receives
> the file is really the person I expect?*
>
> Scope: what is (or will be) **in the public AGPL-3.0 repo**. Developments that
> require managed identity infrastructure (directory + SSO, key transparency,
> federation with domain-proof) are **out of the open core**: they are part of the
> commercial edition — see [`COMMERCIAL.md`](COMMERCIAL.md).

---

## What the cryptography guarantees today (current state)

Encryption answers *"only the intended recipient can decrypt"*, with different
guarantees depending on the send mode.

| Mode | Cryptographic guarantee | What binds to the recipient | What you must do |
|---|---|---|---|
| **Contact (HPKE auth)** | Only the private-key holder can read; the recipient learns *who* the sender is (auth mode) | An **X25519 public key** | Verify **once** that the key belongs to the right person |
| **Code (SPAKE2)** | Only whoever knows the code derives the key and decrypts the ticket | **Knowledge of the code** | Deliver the code over the right channel |
| **`arvc…` ticket** | Anyone holding the ticket receives (pure capability) | **Possession of the link** | Protect the link (+ optional password) |

Code references:
[`core/src/crypto.rs`](../core/src/crypto.rs) (HPKE auth, fingerprint, password-wrap),
[`core/src/code.rs`](../core/src/code.rs) (SPAKE2 / pairing).

### The uncovered gap
The cryptography guarantees *"only the holder of the private key / the secret can
read"*. The question *"is it **really them**, the person I think"* stays open:
today there is no automatic **key ↔ person** binding. It is closed only by
**out-of-band verification** of the fingerprint. This is the gap the open core
must fill **without** introducing a central authority (that is the Enterprise's
job instead).

---

## Open-core model: TOFU + manual verification (Signal safety-number style)

Philosophy: **no central trust anchor**. Trust is established by the user, once,
out-of-band; the software makes it convenient, repeatable, and tamper-evident
after the fact. It is the right model for consumer P2P and for self-hosting
without an IdP.

### 1. Human-readable fingerprint (already present)
- 8 words derived via BLAKE3 from the public key (**64 bits**) —
  [`PublicId::fingerprint`](../core/src/crypto.rs). Widened from the original six
  words (~48 bits), which were grindable: an active MITM could brute-force a
  substitute keypair whose words matched and defeat the check.
- It is a *display aid* for the out-of-band comparison ("read me your eight words").
  The full base32 remains the authoritative value for matching.

### 2. Explicit verify command (to do)
- `arvolo contact verify <name>` that shows the fingerprint and, after the user
  confirms, **marks the contact as verified** in the contact book
  ([`cli/src/book.rs`](../cli/src/book.rs)).
- Per-contact persisted state: `unverified` / `verified` (+ timestamp).
- When sending to an `unverified` contact, a non-blocking warning.

### 3. Key-change detection / after-the-fact anti-MITM (to do)
- The contact book stores the verified key. If a contact's key **changes**,
  `send`/`recv` flag it prominently ("X's key changed — re-verify the fingerprint
  before continuing"), Signal-style.
- Closes the attack where someone swaps the key of an already-known contact.

### 4. Verified exchange via in-person pairing (to do)
- When two people pair in person / on the same LAN, the SPAKE2 flow can **also**
  exchange and pin the long-term identities (contact verified "for free", no
  manual word comparison).
- Uses the existing PAKE channel ([`core/src/code.rs`](../core/src/code.rs)); no
  new infrastructure needed.

### 5. QR verification (to do, optional)
- Show the fingerprint / key as a **QR**; scanning it in person is equivalent to
  comparing the eight words but without typos. Useful for the future GUI/mobile.

---

## Explicit boundary (what does NOT enter the open core)

Anything that requires a **managed trust anchor** for identity belongs to the
commercial edition (see [`COMMERCIAL.md`](COMMERCIAL.md)), **not** this repo:
authenticated directory, SSO/SAML/OIDC + SCIM, key transparency, federation with
domain verification, centralized revocation and audit of the identity binding.

Reason for the boundary: the open core stays **without trusted parties for
identity** (the user provides the trust). Introducing a directory means
introducing a managed trust anchor → which is exactly what a company pays for and
governs, and it lives in the separate commercial repo.

---

## Suggested priority (open core)
1. `contact verify` + verified state in the contact book (#2).
2. Key-change detection (#3) — the biggest security gain for the cost.
3. In-person pairing that pins identities (#4).
4. QR (#5) — when the GUI/mobile arrives.

To be built driven by real usage, consistent with the philosophy of
[`ROADMAP-FUTURE.md`](ROADMAP-FUTURE.md).
