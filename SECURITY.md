# Security

## Reporting a vulnerability

Report it privately, not as a public issue: **info@termox.it**, or through
GitHub's [private vulnerability
reporting](https://github.com/lords82/arvolo/security/advisories/new) on this
repository.

Please include what you did, what happened, and what you expected — a proof of
concept if you have one, but a clear description is worth more than a working
exploit. You will get a first reply within **72 hours**. If a fix is needed, we
will agree a disclosure date with you rather than sitting on it.

Arvolo is maintained by one person. That is not an excuse for a slow answer, but
it is the reason there is no security team address to escalate to.

## What is supported

The **latest release**. Fixes go into the next release rather than into patches
for older ones, and the desktop app updates itself once a fix is out.

## What has and has not been reviewed

**Arvolo has not had an external security audit.** The design is documented in
detail — [`docs/PROTOCOL.md`](docs/PROTOCOL.md) is the wire protocol, every flow
and every format; [`docs/TECHNICAL-OVERVIEW.md`](docs/TECHNICAL-OVERVIEW.md) is
the reasoning behind it — precisely so that it can be reviewed. Review is
welcome, and so is the finding that something in there is wrong.

Two limits worth knowing before you rely on it:

- **The Windows installer is not code-signed** (an EV certificate costs a few
  hundred euro a year). SmartScreen warns about it. Every release carries a
  `SHA256SUMS` file if you want to verify what you downloaded. The macOS `.dmg`
  is signed and notarized.
- **A browser download link trusts the relay that serves it.** The decryption
  happens in the browser and the key stays in the URL fragment, which browsers
  never send to a server — but the relay serves the JavaScript doing the
  decrypting, so it could serve different JavaScript. For a link that matters,
  use a relay you run. Nothing else in Arvolo has this property: P2P transfers
  and sealed mailbox deposits are decrypted by Arvolo itself.

## Reporting abuse

The public default relay (`arvolo.duckdns.org`) is a small machine run by the
project. If someone is using it to distribute something they should not, write to
**info@termox.it** and it will be removed.
