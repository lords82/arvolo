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

### 2. Explicit verify command (shipped)
- `arvolo contacts verify <name-or-id>` shows the fingerprint and, after the user
  confirms, **marks the identity verified** in the contact book
  ([`cli/src/book/verified.rs`](../cli/src/book/verified.rs)). `arvolo contacts
  unverify` undoes it.
- Per-contact persisted state: `unverified` / `verified`, **with the date the mark
  was made** — the ledger is `id = <unix-seconds>`
  ([`Verified`](../cli/src/book/verified.rs)), so `arvolo contacts list` can answer
  "how long ago did I actually check this?" and `verify` can tell a first check
  from a re-check.
- The date travels between your devices with the mark (`MarkEntry::since`), so it
  means the same thing on each. It is **not** an input to the CRDT merge: the
  Lamport clock decides which write wins and the date rides along, or convergence
  would depend on two machines agreeing about the time.
- A mark made before this existed reads back as verified with **no date**, which is
  deliberately distinct from "verified just now": dropping the mark would have
  undone a security decision the user made, and inventing a date would have lied
  about it.
- Sending to an unverified recipient prints a non-blocking warning naming the
  fingerprint and the command to fix it (`warn_if_unverified`). It does not block:
  a warning that stops the send is a warning that gets switched off.
- `arvolo contacts trust` — which lets the daemon auto-download — refuses an
  unverified contact outright unless `--force`.

### 3. Key-change detection / after-the-fact anti-MITM (shipped)
- The contact book stores the key it verified. If a contact's key **changes**,
  `arvolo contacts add` warns prominently and **clears both the verified and the
  trusted marks** ([`contact_add`](../cli/src/book/contacts.rs)), so auto-download
  stops until the new key is confirmed.
- That demotion is also what surfaces the change at send time: an identity whose
  key changed is no longer verified, so §2's warning fires on the next send. There
  is deliberately no *separate* key-change record — the mark is the state.
- The demotion propagates to your other devices as a CRDT tombstone
  ([`sync_bridge.rs`](../cli/src/book/sync_bridge.rs)), so re-verifying is a
  conscious act on each of them.
- Closes the attack where someone swaps the key of an already-known contact,
  **provided the swap arrives through `contacts add`**. A key learned only from an
  incoming transfer is never auto-rebound, so there is nothing to demote.

### 4. Verified exchange via in-person pairing (shipped)
- `arvolo contacts pair` — one side shows a short code, the other types it, and
  **both** end up with the other saved and marked verified. No fingerprint
  comparison, no pasting a 52-character id.
- The verification is real, not a courtesy: the SPAKE2 channel only forms between
  two parties that knew the same code, so a key arriving through it is
  authenticated *by* the code. Its strength is that of the channel you read the
  code over, plus the relay's rate limiting against guessing (§6.1/§6.4 of
  [`PROTOCOL.md`](PROTOCOL.md)) — the same terms `arvolo code` already relies on.
- This required one protocol addition. The rendezvous was one-directional (a
  sender fills a slot, a receiver empties it), which cannot express an *exchange*;
  a per-session `b.` key now carries a sealed reply back, under a key
  domain-separated from the payload's.
- It therefore **requires a relay with rendezvous v2**. The command checks before
  showing you a code, and refuses rather than completing half a trade: an exchange
  where only one side ends up holding the other's id leaves the two of you
  disagreeing about what just happened, which is worse than a clean failure.
- Do not confuse it with `arvolo device pair`, which shares your **secret**
  identity to make another machine you. This trades **public** ids between two
  different people.

### 5. QR verification (to do, optional — and largely superseded)
- Show the fingerprint / key as a **QR**; scanning it in person is equivalent to
  comparing the eight words but without typos. Useful for the future GUI/mobile.
- Mostly answered by §4: `arvolo contacts pair --qr` already renders the *pairing
  code* as a QR, and scanning that both exchanges and verifies. A fingerprint QR
  would only help where the two sides cannot reach a common relay, since pairing
  needs one and reading eight words aloud does not.

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

## State of the plan (open core)

§1–§4 are shipped: the fingerprint, `contacts verify` (with the date of the
check), key-change demotion, and the mutual `contacts pair`. What remains is §5
(a fingerprint QR), which pairing has largely answered.

Nothing yet *acts* on the verification date — there is no expiry, no "re-verify
after N months" nudge. That is deliberate for now: an expiry policy is a decision
about someone else's threat model, and the date has to exist before it can be
argued about.

Everything further is to be built driven by real usage, consistent with the
philosophy of [`ROADMAP-FUTURE.md`](ROADMAP-FUTURE.md).
