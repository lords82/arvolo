# arvolo — Open-core future roadmap (post-MVP)

> Ideas that are designed but **deliberately out of the MVP**, parked here for later versions.
> The MVP is: *P2P-first + ONE self-hosted relay with an expiring mailbox + browser link + desktop UI* (CLI-first).
> We build from here driven by **real customers/data**, not up front. Every entry keeps its rationale.
>
> Scope: **open-core only** (public AGPL-3.0 repo). Commercial features — relay
> federation, governance/console, SSO/audit, managed hosting — belong to the
> commercial edition (separate private repo); see
> [`COMMERCIAL.md`](COMMERCIAL.md) and [`../LICENSING.md`](../LICENSING.md).

---

## 1. Mobile (iOS / Android)
- Build via **UniFFI** or **flutter_rust_bridge** on top of the Rust core.
- **Share sheet** ("Share with arvolo" from Photos/Files).
- **Background receive / push** (APNs/FCM) while the app is closed.
- ⚠️ Sharp edge: iOS constraints on background work and long transfers → validate early with a dedicated spike.

## 2. Multi-recipient
- **Body dedup** via HPKE **KEM+DEM**: the body is encrypted once under a content-key; only the **encapsulation** is per-recipient → the relay keeps a single copy of the body chunks for N recipients, and the sender backfills them once.
- **Reference-counted GC**: for each chunk the relay keeps the **set of pending recipients** and frees it only when the set empties (refcount → 0) → in multi-recipient it never drops a chunk a slow recipient still needs. **Per-recipient TTL** on the hold (a no-show expires); only the recipient's **signed ack** frees space.
- **Slow recipient** (e.g. A completes, B is halfway): B's chunks **stay on the relay** because refcount>0 → B downloads them from the relay (or from the sender if online).

## 3. Multi-source swarming (torrent-style)
- Being content-addressed, the receiver downloads **BLAKE3 ranges of iroh-blobs in parallel from multiple providers** (sender + relay + other relays).
- **Seeding among co-recipients**: a recipient who has completed acts as an extra source for the others (covers the "sender crashed abruptly" case). **Opt-in** (it retains the ciphertext chunks) and **bound by privacy policy** (it reveals the co-recipients → default off if they must not know each other).
- ⚠️ Rarely triggers (most sends are 1 recipient) → build **only if usage data** shows large sends to many.

### 3.1 Large-file distribution strategy to N recipients
> "Seed each chunk once + swarm + durable relay seed" → sender upload ≈ **1×** instead of N×.
- *All online + connectable + privacy OK* → pure swarm with disjoint seeding, relay at zero storage.
- *Someone offline / hostile NAT / strict privacy* → the same chunks flow through the relay hub.

## 4. Transfer protocol — advanced source selection
- **Asymmetric failure handling**: direct path broken (both online) → iroh relay in live forwarding, zero storage, retries hole punching; receiver down → sender→relay backfill; sender down (abruptly) → the receiver waits for its return.
- **Rarest-first**: on resume, first the chunks present only on the sender (least redundant) to secure them against a second crash.
- **Anti double-send**: during direct delivery the sender pauses the backfill of chunks it is already pushing directly.

## 5. Recipient identity verification (open-core)
> **TOFU + manual verification** model, Signal safety-number style: no central
> trust anchor, trust is established by the user once, out-of-band.
> Full detail in [`IDENTITY-VERIFICATION.md`](IDENTITY-VERIFICATION.md).
- `arvolo contact verify` + **verified** state in the contact book.
- **Key-change detection** (after-the-fact anti-MITM): warn if a known contact's key changes.
- In-person pairing that **pins long-term identities** (a contact verified "for free").
- Verification via **QR** (once GUI/mobile land).
- Automatic key↔person binding (directory, SSO, key transparency) is **out of the open core**: commercial edition (see [`COMMERCIAL.md`](COMMERCIAL.md)).

## 6. Hardening & self-host (open-core)
- Link policies: **download-once, revoke, password, max downloads**.
- **Self-host packaging** (Docker/compose) + self-hosted iroh relay for 100% control/privacy.
- **Operational-grade self-hosting** (not just `docker run`): **Helm chart / Terraform**, relay in **HA**, backup/restore, Prometheus metrics + healthcheck, documented upgrades.
- **Security audit** of the protocol/crypto (HPKE auth mode, BLAKE3-signed manifest, claim token) + **public security whitepaper**. Without an independent audit, "zero-knowledge" is a claim, not a fact.

## 7. Experience & adoption (open-core)

> Product factors that widen the addressable market. The *sales strategy* and the
> paid features are in the commercial edition (see [`COMMERCIAL.md`](COMMERCIAL.md)).

### 7.1 No-install experience (adoption blocker #1)
Today it is CLI-only; installer for Linux/macOS only. Nobody adopts a tool where
the *recipient* has to install something or use a terminal.
- **Browser link-mode** (successor to Firefox Send): the recipient opens a link,
  downloads and decrypts in the browser, zero install. On its own it widens the addressable market.
- **Desktop GUI + Windows build** (today even the Windows binary is missing).
- **Recipient notification** (email "you have a file") + **proof-of-delivery /
  signed delivery receipt**.
  → **if you pick one thing, it's browser link-mode**: it unlocks the adoption funnel.

### 7.2 Technical wedge: API + CI/CD (where we beat GUI-first competitors)
Secure **machine-to-machine transfer in pipelines** (artifacts between environments,
encrypted backups to a partner, automated B2B exchange). CLI + self-hosted relay
+ E2E is exactly what's needed.
- **Stable API/SDK, webhooks, clean exit codes** → a product for DevOps/platform
  teams that no WeTransfer covers.

### 7.3 EU sovereignty / data residency (structural advantage)
- **Data residency / EU sovereignty** vs WeTransfer/Dropbox/US-SaaS: "your data
  never leaves your infrastructure / the EU" (GDPR, no CLOUD Act). Strong in the EU
  market and the **Italian public sector**. ✅ *Already led with on the README front page.*
