# Arvolo swarm — design spec

> **Status:** phases 1–4 implemented and phase 5 partially, on top of 0.8.5.
> Scope: **swarm applies only to shared `arvc…` tickets** (reusable content
> capabilities, magnet-like). `--to <contact>` sends stay strictly 1-to-1 (sealed
> per recipient) and are out of scope — they keep the sender+relay resilience
> already shipped in 0.8.5.
>
> **Implementation status** (see §11 for the phase plan):
> - ✅ **Phase 1** — supervised control channel (reconnects), live provider choice.
> - ✅ **Phase 2** — persist + resume downloads across a receiver restart.
> - ✅ **Phase 3** — relay tracker (`/v1/swarm/*`), receiver-as-seeder (`Reseal`),
>   peers as providers.
> - ✅ **Phase 4** — rarest-first piece selection.
> - ◻︎ **Phase 5** — done: relay-only privacy toggle (`ARVOLO_SWARM=off`).
>   Deferred: provider health/banning, swarm metrics in `arvolo transfers`,
>   endgame mode (needs per-attempt stage files), seed-after-complete.

This document specifies turning a reusable-ticket transfer into a **BitTorrent-style
swarm**: multiple concurrent sources (original sender, relay, and other receivers
that already hold pieces), **rarest-first** piece selection, **fill mode** (use all
sources at once), churn tolerance, and **resume across a receiver restart**.

---

## 1. Goals / non-goals

**Goals**
- One `arvc…` ticket → many receivers download the same content, sharing pieces.
- A receiver pulls each piece from whatever source has it: original sender (P2P),
  relay (seeded pieces), or **other receivers** (peers) that already hold it.
- **Rarest-first**: fetch the scarcest pieces first, so scarce sources (a sender
  about to drop) are drained before abundant ones (already on the relay/peers).
- **Fill mode**: saturate download capacity across all sources concurrently.
- Survive repeated source churn: sender/peers coming and going, any number of times.
- **Resume across a receiver daemon/machine restart** with no re-accept.
- Preserve arvolo's **zero-knowledge** property: relay/tracker never see plaintext
  or the content key.

**Non-goals (for this spec)**
- Swarming `--to` sends (per-recipient sealing → non-shareable pieces).
- Incentive/anti-leech schemes (tit-for-tat, super-seeding) — small trusted swarms
  don't need them; noted as a future option (§12).
- Changing the crypto of a chunk (sealing stays as-is).
- **Backward compatibility.** We are pre-1.0 and control every node: all clients and
  the relay run a version that speaks the swarm protocol. We change the ticket and
  wire formats freely — no old-client fallbacks, no version negotiation, no graceful
  degradation. (See §10.)

---

## 2. Why arvolo is already well-suited

The hard parts of a swarm are mostly already present:

- **Content-addressed pieces.** A chunk is fetched by its BLAKE3 hash:
  `fetch_chunk_wire(endpoint, addr, hash)` pulls a chunk *from any address* and
  verifies `BLAKE3(ciphertext) == hash`. The fetch path already takes a **list of
  providers** and tries them in order — adding peers is just a longer list.
- **Deterministic sealing.** `seal_chunk(key, idx, total_chunks, plaintext)`
  produces a fixed ciphertext → fixed hash. So a receiver that has the **key** (from
  the ticket) and the **plaintext** of a piece can *reproduce the exact ciphertext*
  for that piece → it can serve it to others. **Receivers can seed** with the same
  `ChunkServer` machinery the sender uses.
- **Piece list = content identity.** The `arvc…` ticket already carries
  `chunks: Vec<Hash>`, `total_size`, and `key`. The ordered hash list is the
  content fingerprint (like a torrent infohash).
- **Relay is already a per-hash chunk store** (`/v1/seed`, `/v1/release/{token}/{hash}`,
  `ChunkBackend::Files`) and a **presence service** (`/v1/presence/{slot}`). It can
  double as the **tracker**.
- **Control channel** already carries `Have(idx)` acks and `RelayHas(indices)` — the
  seed of a bitfield/availability exchange.

What's missing: peer **discovery** (a tracker of who-has-what), a **bitfield**
exchange, the **receiver-as-seeder** role, a **rarest-first scheduler**, a **dynamic**
provider set (reconnect/re-resolve), and **on-disk persistence**.

---

## 3. Terminology

- **Swarm** — the set of nodes exchanging pieces of one content.
- **`swarm_id`** — `BLAKE3(chunk_hash[0] ‖ chunk_hash[1] ‖ … ‖ total_size)`,
  32 bytes. Derived purely from the ticket's piece list, so every ticket holder
  computes the same id without contacting anyone. Used as the tracker key. It does
  **not** reveal plaintext (hashes are of ciphertext).
- **Piece / chunk** — a 16 MiB unit (last one shorter), sealed, addressed by hash.
- **Bitfield** — `ceil(n_chunks/8)` bytes, bit `i` = "I have verified piece `i`".
- **Provider** — anything that can serve a piece: the original sender, the relay,
  or a peer (another receiver). Each provider advertises a bitfield.
- **Seeder** — a node with the complete bitfield. **Leecher** — an incomplete node
  that also serves what it has.
- **Tracker** — the relay endpoint that lets peers announce and discover each other.

---

## 4. Architecture overview

```
                         ┌────────────────────────┐
                         │        RELAY           │
                         │  • tracker (announce/   │
                         │    peers, per swarm_id) │
                         │  • chunk store (Files,   │
                         │    seeded pieces)        │
                         │  • presence             │
                         └───────┬─────────┬───────┘
             announce/peers      │         │  fetch seeded pieces
             (bitfields)         │         │  (CHUNK_ALPN via relay addr)
          ┌──────────────────────┘         └───────────────────────┐
          │                                                          │
   ┌──────┴───────┐   direct P2P pieces (CHUNK_ALPN)    ┌────────────┴─────┐
   │  RECEIVER A  │◀──────────────────────────────────▶│   ORIGINAL       │
   │ (leecher +   │                                     │   SENDER (seed)  │
   │  seeder)     │◀───── peer pieces (CHUNK_ALPN) ────▶└──────────────────┘
   └──────┬───────┘         ▲
          │                 │
          │   peer pieces   │
          ▼                 │
   ┌──────────────┐         │
   │  RECEIVER B  │─────────┘
   │ (leecher +   │
   │  seeder)     │
   └──────────────┘
```

Every node runs both roles for a swarm it's in:
- **Leecher**: schedules and fetches missing pieces from the best available providers.
- **Seeder**: serves pieces it has (via `ChunkServer` + a reseal backend) to peers.

The relay is a special provider that is always reachable and stores whatever the
sender/peers seed to it — it's the durable backstop and the meeting point.

---

## 5. Component specs

### 5.1 The swarm ticket

The `arvc…` ticket already carries everything the swarm needs: `chunks`,
`total_size`, `key`, the origin node addr, and the relay URL. Reusable (`arvc…`)
tickets simply **become swarm tickets** — no marker or flag needed, since back-compat
is a non-goal and the relay/all clients speak the swarm protocol. `swarm_id` is
**derived** from the piece list (§3), never stored, so it can't disagree with it.

(We are free to reshape `ChunkTicket` if convenient — e.g. drop now-unused fields —
without worrying about older readers.)

### 5.2 Tracker (relay endpoints)

New relay routes, keyed by `swarm_id` (hex). All zero-knowledge: the relay sees node
addresses and bitfields, never the key or plaintext.

- `POST /v1/swarm/{swarm_id}/announce`
  Body: `{ node_addr, bitfield, n_chunks, event: "started"|"progress"|"completed"|"stopped", want: u16 }`
  - Registers/refreshes this peer with a TTL (reuse presence's ~30 s TTL + refresh).
  - Returns: `{ peers: [{ node_addr, bitfield }...], relay_bitfield }` — up to `want`
    other peers (random sample), plus which pieces the **relay itself** holds for
    this swarm (so the relay is treated as a provider uniformly).
  - `event:"stopped"` deregisters; `"completed"` marks a seeder.
- `GET /v1/swarm/{swarm_id}/peers?want=N` — poll for peers without re-announcing.

Tracker state is in-memory with TTL (like presence), plus the existing seeded-chunk
index for `relay_bitfield`. No new persistence on the relay.

**Abuse control**: rate-limit announces per IP; cap `bitfield` length to
`ceil(n_chunks/8)` and `n_chunks` to a sane max; optionally only track a `swarm_id`
once at least one valid seed for it exists (prevents using the relay as a generic
tracker for unknown content).

### 5.3 Bitfield & availability

- On join, a peer sends its bitfield in `announce` and receives peers' bitfields.
- As pieces complete, peers push incremental **`Have(idx)`** over a lightweight peer
  control stream (reuse `CtrlMsg`, add `Have` already exists; add `Bitfield(bytes)`
  for the initial dump). Periodic re-announce also refreshes the tracker's view.
- Each leecher maintains an **availability map**: `avail[piece] = count of providers
  (sender+relay+peers) currently advertising it`. Updated on bitfield/Have/peer
  join/leave.

### 5.4 Peer wire — the seeder role

Reuse `CHUNK_ALPN` and `ChunkServer`. Add a backend:

```
ChunkBackend::Reseal {
    path: PathBuf,                     // the (partial) output file
    key: [u8; CHUNK_KEY_LEN],
    total_chunks: u32,
    have: Arc<Mutex<Bitfield>>,        // which pieces we can serve
}
```

`produce(hash)`:
1. Map `hash → idx` via the ticket's `chunks` list (a peer knows the full hash list).
2. If `!have[idx]` → return `None` (respond `total_len = 0`, "not available here").
3. Read `path[idx*CHUNK_SIZE .. ]` (16 MiB or tail), `seal_chunk(key, idx,
   total_chunks, plain)` → ciphertext, return it. Deterministic → matches `hash`.

A leecher thus serves exactly the pieces it has verified, and grows its served set as
it downloads. Seeders (complete) serve everything.

`produce` stays on the blocking pool (it's already re-encrypting 16 MiB there).

Note: a peer reseals from **plaintext** it decrypted. That's fine — it holds the key
already (it's a legitimate ticket holder). It never exposes plaintext on the wire;
it serves ciphertext identical to the sender's.

### 5.5 Fetch scheduler — rarest-first + fill mode

This replaces the current ascending-order, per-chunk provider pick.

State per transfer:
- `need`: set of pieces not yet verified locally.
- `avail[piece]`: provider count (from §5.3).
- `providers[piece]`: the concrete list of addresses currently advertising it.
- `in_flight`: pieces currently being fetched (with which provider).

Loop (keeps `concurrency` fetches in flight — fill mode):
1. **Pick piece**: among `need \ in_flight` with `avail ≥ 1`, choose the one with the
   **lowest `avail`** (rarest). Tie-break **randomly** among the rarest set (so peers
   don't all grab the same piece). → This is why **rarest-first subsumes
   "sender-only first"**: a piece only the sender has (`avail = 1`) is rarer than a
   piece the sender *and* the relay have (`avail = 2`), so it's fetched first —
   automatically draining the scarce source before it can drop.
2. **Pick provider** for that piece: among providers advertising it, prefer by a
   simple policy — spread load (avoid hammering one), prefer peers/relay over the
   original sender when equal (offload the origin), avoid providers with recent
   failures. Pluggable; start simple (random among healthy providers, capped
   in-flight per provider).
3. Spawn a fetch → stage in `.arvpart.{i}` → BLAKE3-verify (already done by
   `fetch_from`) → on success mark `have[i]`, remove from `need`, broadcast `Have(i)`
   to peer control streams, and `announce(progress)` periodically.
4. On failure: keep the piece in `need`, decrement that provider's health, retry via
   the indefinite backoff already in place (§ shipped 0.8.5).
5. **Endgame**: when `need.len() ≤ ENDGAME_THRESHOLD`, request each remaining piece
   from **multiple** providers concurrently and cancel the losers, so one slow
   provider can't stall the finish.

Commit order can stay ascending (assemble the output file by seeking to
`i*CHUNK_SIZE`), independent of fetch order — pieces are staged per-index and
committed when contiguous, exactly as today. Out-of-order fetching is already how the
parallel window works; only the **selection** changes from "next ascending" to
"rarest".

### 5.6 Dynamic provider set & connection management

- The provider set is **re-resolved continuously**, not captured once:
  - periodic tracker re-announce (refresh TTL + fresh peer list);
  - peer join/leave via control streams;
  - the original sender's `RelayHas` updates (relay-side availability).
- **Control-channel reconnection**: today the control channel opens once and never
  recovers (see `flow.rs::recv_chunked`). The swarm scheduler owns a small
  "connection manager" that keeps a control stream to each active provider, reopening
  on drop with backoff, so `avail`/bitfields stay live across churn.
- `sender_offline` stops being a static boolean — a provider is "up" iff it currently
  has a live control/data connection; scheduling reacts to that.

### 5.7 Persistence & resume across restart

Per accepted swarm transfer, keep on-disk state under
`~/.config/arvolo/transfers/{id}/`:
- `meta.toml`: `{ ticket (or its essentials: chunks, total_size, key, relay, swarm_id),
  out_path, direction, peer, created_at }`.
- `bitfield.bin`: verified-pieces bitfield.
- The partial output file at `out_path` (+ any `.arvpart.{i}` for the in-flight
  piece).

On daemon startup:
1. Scan `transfers/`; for each incomplete one, **re-validate**: recompute BLAKE3 of
   each "have" piece from the output file (cheap-ish; or trust bitfield + spot-check,
   configurable) and rebuild `have`/`need`.
2. **Rejoin the swarm** (announce with the current bitfield) and resume the scheduler
   — **no re-accept prompt**; the transfer was already accepted, its acceptance is
   the on-disk record.
3. Emit a `Resumed` event so `arvolo transfers` shows it active again.

The **key is stored on disk** (in `meta.toml`, owner-only `0600`, same as the ticket
would be). This is required for resume and reseeding; call it out as a security
tradeoff (§8).

---

## 6. Wire protocol summary

**Relay HTTP (new):**
- `POST /v1/swarm/{swarm_id}/announce` → peers + relay_bitfield.
- `GET  /v1/swarm/{swarm_id}/peers` → peers.
(Existing `/v1/seed`, `/v1/release/{token}/{hash}` still used to push/free relay
pieces; existing `/v1/presence` unchanged.)

**Peer P2P (existing ALPNs, extended messages):**
- `CHUNK_ALPN`: unchanged request/response (`ChunkReq{hash, offset}` →
  `ChunkResp{total_len}` + bytes). Now also spoken *by receivers* (seeder role).
- `CTRL_ALPN` (`CtrlMsg`): add `Bitfield(Vec<u8>)` (initial dump) alongside existing
  `Have(idx)` / `RelayHas(indices)` / `Hello` / heartbeat. Used peer↔peer, not just
  receiver→sender.

All piece bytes remain ciphertext, BLAKE3-verified on receipt. No new plaintext ever
crosses the wire.

---

## 7. Scheduling details & tunables

- `ENDGAME_THRESHOLD` (e.g. 4 pieces): switch to multi-source redundant fetch.
- `MAX_PEERS` per swarm (e.g. 30) and `MAX_INFLIGHT_PER_PROVIDER` (e.g. 4).
- Rarest-first with random tie-break; optional **"first N pieces random"** warm-up so
  a brand-new leecher gets *something* to serve quickly (helps small swarms bootstrap).
- Provider health: exponential backoff per provider on failures; drop a provider that
  serves a piece failing BLAKE3 (poisoning defense) and don't re-pick it for a while.

---

## 8. Security & privacy

- **Zero-knowledge preserved.** Relay/tracker see: ciphertext pieces, node addresses,
  bitfields, `swarm_id`. They do **not** see the content key or plaintext. Same trust
  model as today's relay, plus peer addresses (see below).
- **Capability = the ticket.** Anyone with the `arvc…` ticket can decrypt — that's the
  existing semantics of a reusable ticket. The swarm doesn't broaden who can read;
  it only broadens where bytes come from.
- **Piece integrity / poisoning.** Every piece is BLAKE3-verified against the ticket's
  hash before use; a bad piece is discarded and the offending provider penalized/banned
  for the transfer. A malicious peer cannot inject wrong data.
- **Peer address exposure.** Swarm members learn each other's iroh node addresses (and
  thus approximate network location) — inherent to P2P swarms. Acceptable for
  shared-ticket content (explicitly opt-in via a swarm ticket). Offer a
  **"relay-only" privacy mode**: a peer can choose to fetch/seed only via the relay
  and never expose itself to other peers (no `announce` of its addr; still uses relay
  as store). Trades swarm efficiency for privacy.
- **At-rest key.** Resume requires the key on disk (`0600`). Document it; consider an
  optional passphrase-wrapped store later.
- **Tracker abuse.** Rate-limit; cap bitfield/n_chunks; optionally require a prior seed
  for a `swarm_id` before tracking it.

---

## 9. Failure handling / churn

- **Sender leaves**: its pieces are (partly) on the relay/peers; rarest-first already
  prioritized its unique pieces while it was up. Missing-everywhere pieces wait
  (indefinite retry) until the sender or a seeder returns.
- **Peer leaves**: `avail` drops; scheduler re-picks from remaining providers; tracker
  TTL evicts it.
- **All sources gone**: transfer stays `active`, retrying with 5-min-capped backoff,
  resuming when anyone returns (shipped behavior).
- **Relay restart**: seeded pieces persist in its `/data` volume; presence/tracker
  state is in-memory and repopulates as peers re-announce.
- **Receiver restart**: §5.7 resume.

---

## 10. Backward compatibility — explicitly out of scope

We are pre-1.0 and operate a closed set of nodes (our clients + our relay), so
**backward compatibility is a non-goal**. Consequences we deliberately accept:

- The **ticket format** may change shape freely (rename/drop/add fields, bump the
  encoding). No "old clients ignore unknown fields" contortions.
- The **wire protocol** (relay routes, `CtrlMsg`, chunk framing) may change; we do
  **not** add version negotiation or `/v1/features`-gated fallbacks for the swarm.
- Rollout is a **coordinated upgrade**: deploy the relay first, then clients. A brief
  window where mismatched versions can't talk to each other is acceptable (the
  version gate already refuses a mismatched CLI↔daemon, and transfers resume after
  everyone's upgraded).
- No "reusable ticket also works with an old client" guarantee — a `arvc…` ticket is
  a swarm ticket, full stop.

If we ever ship publicly to third parties on their own release cadence, revisit this.

---

## 11. Phased implementation plan

Each phase is independently shippable and testable.

- **Phase 1 — dynamic multi-source (sender + relay), fill + reconnect.**
  Re-resolve providers continuously; reconnect the control channel; fetch sender and
  relay concurrently (fill). No peers yet. Lands most of the single-sender resilience
  win. *(Builds directly on the 0.8.5 retry.)*
- **Phase 2 — persistence & resume across receiver restart** (§5.7).
- **Phase 3 — relay-as-tracker + receiver-as-seeder** (§5.2, §5.4): peers discover
  each other and serve pieces. Availability from the tracker; still simple selection.
- **Phase 4 — rarest-first + endgame scheduler** (§5.5): the real swarm scheduling.
- **Phase 5 — polish**: privacy "relay-only" mode, warm-up randomization, provider
  health/banning, tunables, metrics in `arvolo transfers`.

Suggested first build target after this spec is approved: **Phase 1**.

---

## 12. Decisions & remaining questions

**Decided (approved during review):**
- **Peer address privacy** → provide an opt-in **"relay-only" mode**: a peer can
  fetch/seed only via the relay and never announce its address to other peers
  (§8). Default is full swarm; relay-only is the privacy escape hatch.
- **Key at rest** → store the content key `0600` in the per-transfer `meta.toml` to
  enable resume/reseed. An optional passphrase-wrapped store is a **later** add-on,
  not blocking.
- **Origin offload / super-seeding** → **deferred** (Phase 5+). The origin keeps
  serving normally for now; revisit throttling once ≥K seeders exist.
- **Resume validation** → **default: full BLAKE3 re-verify** of on-disk "have" pieces
  at startup (correctness first — a truncated/corrupt file must not be trusted), with
  a config override to **trust-bitfield + spot-check** for speed on large files.
- **Incentives** → none: naive "serve everyone" for the expected small, trusted
  swarms. Revisit only if abuse shows up.

**Still open:**
1. **Max content size / piece count** the tracker accepts (bitfield caps) — pick
   concrete limits during Phase 3.
2. **Discovery beyond the relay**: relay-tracker only for now; local-network (mDNS) /
   DHT is out of scope — flag if ever needed.
3. **`--to` swarm** (deferred): would need a shared key among recipients — revisit
   only if a real need appears.

---

## 13. Testing strategy

- **Unit**: `swarm_id` derivation; bitfield ops; rarest-first selection (given
  synthetic availability maps); `Reseal` backend reproduces the sender's ciphertext
  byte-for-byte (property test: `produce(hash_i) == sender_ciphertext_i`).
- **Integration (local harness)**: 1 relay + N daemons on distinct `HOME`s + a local
  swarm ticket; assert every leecher completes with an intact file while sources are
  killed/restarted mid-transfer. Fault injection is easier here than the sender/relay
  race we hit on loopback, because we can hold specific peers offline.
- **Churn/resume**: kill a receiver mid-download, restart its daemon, assert it
  rejoins and finishes from its bitfield with no re-accept.
- **Poisoning**: a peer serving corrupted bytes is detected (BLAKE3) and avoided.
