# Commercial / Enterprise split — ⚠️ TO RE-EXAMINE

> **Reminder (read this again before launch).** The commercial/Enterprise part of
> Arvolo must live in a **separate, private, protected git repository** (e.g.
> `arvolo-enterprise`) under the **commercial license** — never merged into this
> public AGPL-3.0 repo. We still need to **define the exact boundary** between the
> open core and the commercial part. This file is the placeholder for that decision.

## Why a separate private repo

- Keeps dual-licensing clean: the public repo is 100% AGPL; the commercial code
  never touches it, so there's no licensing ambiguity.
- Protects the IP that funds the project (the paid features).
- Lets the open core stay fully auditable (good for a security product) while the
  business layer stays closed.

## Tentative split (TO CONFIRM)

**Open core — this repo (AGPL-3.0, public):**
- Client (`arvolo`): P2P send/recv, offline send/recv, identity.
- Single relay (`arvolo-relay`): zero-knowledge mailbox, TTL, burn-after-read.
- Core crypto/transport (HPKE, SPAKE2, iroh).

**Commercial / Enterprise — separate private repo (commercial license):**
- **Federation** across multiple relays (allow-list, home-relay routing, quotas).
- **SSO / SAML**, team management, RBAC.
- **Audit logging** and compliance exports.
- **Admin console** / dashboard.
- **Managed hosting** control plane and billing.
- Advanced policies at scale (org-wide retention, multi-recipient, etc.).

## Open questions to settle before launch

- [ ] Exact feature-by-feature boundary (what is "core" vs "Enterprise").
- [ ] Mechanism: separate binary? plugin/feature-flag? separate service that talks
      to the open relay over an API? (Prefer a clean API boundary so the open core
      never imports closed code.)
- [ ] CLA in place before accepting external contributions (needed to dual-license).
- [ ] Trademark registration for "Arvolo".
- [ ] Commercial license text (EULA) drafted.

See also: [`../LICENSING.md`](../LICENSING.md) and [`ROADMAP-FUTURE.md`](ROADMAP-FUTURE.md).
