# Terms of Use

**Version 1.0 — 24 August 2026.** This file's git history is its changelog.

## What these terms cover

Two different things travel under the name Arvolo, and only one of them can have
terms attached to it.

**The software** is free software under AGPL-3.0 — see
[`LICENSE`](LICENSE) and [`LICENSING.md`](LICENSING.md). That license is the
entire agreement about the code: you may run it, study it, modify it, and host it
yourself, for any purpose. Nothing in this document narrows that. A free software
license cannot carry a restriction on what you use it for, and this is not an
attempt to add one. What you do with the software is between you and the law that
applies to you.

**The public relay** at `arvolo.duckdns.org` is a service — a small machine the
project runs and pays for. A service may set conditions of access, and these are
them. They apply to anyone who uses that relay: as a sender, as a recipient, or by
opening a download link it serves. If you run your own relay, none of this binds
you; you are the operator, and these questions become yours.

## The service, as it is

No account, no registration, no fee. Nothing to sign up for and nothing to cancel.

A mailbox deposit lives until it is claimed or until it expires, whichever comes
first. The sender chooses the lifetime, capped by the relay at **30 days**. Size
and rate limits apply and may change without notice. Expired deposits are deleted
by a background reaper and are not recoverable — no copy is kept anywhere.

There is no uptime guarantee. The relay may be slow, unreachable, restarted,
reconfigured, or shut down permanently, and a transfer in flight may be lost.
**Arvolo is transport, not storage.** Do not use it as a backup, and do not let it
hold the only copy of anything you care about.

## Acceptable use

You may not use the public relay to transfer, deposit, host, offer for download, or
invite anyone to download:

- **Works protected by copyright or neighbouring rights that you do not hold and
  do not have permission to share.** Sending a file you made, or one you are
  licensed or otherwise entitled to pass on, is fine. Using the relay to
  distribute films, music, books, software, games, or any other protected work to
  people who have no right to receive it is not, and neither is circumventing a
  technical protection measure in order to do it. The right to share a file is
  yours to establish before you send it, not the operator's to check afterwards.
- Material that sexually exploits or abuses minors. This is reported to the
  authorities, without exception and without a warning first.
- Non-consensual intimate images of a person.
- Content that is illegal where you are, where your recipient is, or under Italian
  and EU law, where the relay runs — including content inciting terrorism or
  violence, and material whose possession or distribution is itself an offence.
- Malware, ransomware payloads, phishing kits, stolen credentials, or data
  obtained through unauthorised access to someone else's systems.
- Anything sent to a person who has not asked for it and does not want it, as
  harassment, threat, or bulk unsolicited delivery.

You may also not attack the service itself: no circumventing the size, rate, or
lifetime limits, no automated bulk deposits, no using the relay as a content
delivery network or as backing storage for another product, and nothing intended
to degrade it for other people.

You are responsible for what you send, and equally for what you invite others to
download.

## What the operator can see, and why enforcement works the way it does

This is the part worth reading before you send anything, and before you report
anything.

Content is end-to-end encrypted. Keys never reach the relay. What the relay holds
is ciphertext, its size, the time it arrived, a random claim token, an IP address,
and a public key that anyone can generate for free, in unlimited quantity, with no
connection to a real identity. There is no phone number, no email address, no
account, no verified sender.

Two consequences follow, and they cut in both directions. The operator **cannot**
discover a breach of these rules by looking — no scanning is possible, and none is
performed. And the operator **cannot** tell you who sent something, because the
information does not exist.

Enforcement is therefore entirely reactive. Something is acted on when it is
reported, or when a lawful order arrives — never before.

## Reporting something

Write to **lords82@gmail.com**. A report needs to contain:

- **The exact download link, or the claim token / mailbox id.** Without it nothing
  can be located and nothing can be removed. A report that describes the content
  but not where it is cannot be acted on, however serious it is.
- Why it breaches these terms. For a copyright claim: which work, and whether you
  are the rightsholder or authorised to act for them.
- How to reach you.
- A statement that what you have said is accurate to the best of your knowledge.

You will get an acknowledgement within **72 hours** and, where the report is
actionable, removal follows promptly. Notices known to be false are ignored, and
persistent abuse of this channel is blocked like any other.

## What happens after a report

Depending on what is reported and what is found, some or all of:

- the deposit is deleted — irreversibly, with no copy retained;
- the public key or the IP address behind it is blocked;
- access to the relay is refused entirely, for repeated breaches;
- where the content is child sexual abuse material, or where there is a threat to
  someone's life or safety, the authorities are notified, together with the little
  the relay actually holds.

If something of yours was removed and you believe it should not have been, write
to the same address. You will be told the reason and answered by a person.

## Requests from authorities

Lawful orders from a competent authority are complied with. It is worth being
plain about what there is to comply with: no plaintext, no keys, no filenames, no
recipient lists — none of it exists on the relay. What can exist is the ciphertext
for as long as the deposit lives, a claim token, and connection metadata such as
timestamps and IP addresses, for as long as server logs are retained
(**`<LOG_RETENTION>`**).

Nothing is decrypted, because it cannot be, and nothing is invented to fill the
gap.

## Logs and data

Stored: the encrypted blob until it is claimed or expires; its claim token; server
logs containing timestamps and IP addresses, kept for **`<LOG_RETENTION>`** and
then discarded.

Not stored, because it never arrives: file contents, encryption keys, filenames,
your contacts, or any account.

## Status of the relay

In forwarding transfers and temporarily storing deposits at users' request, the
relay acts as an intermediary service within the meaning of Regulation (EU)
2022/2065 (the Digital Services Act) — a mere conduit for what it relays, a
hosting service for what it holds. It does not initiate transmissions, does not
select their recipients, does not modify their content, and is under no general
obligation to monitor it.

That is not a licence to ignore what is reported. It is the reason the reporting
channel above exists and is answered.

## No warranty, no liability

For the software, sections 15 and 16 of the AGPL apply.

For the service: it is provided **as is**, with no warranty of any kind. The
project is not liable for transfers that are lost, delayed, undelivered, or
expired, for the loss of data held on the relay, for what other users send, or for
what anyone does with what they receive. Nothing here excludes liability that
cannot lawfully be excluded — including for wilful misconduct or gross negligence,
and including mandatory consumer rights.

## Changes

These terms may change. The new version is published here with a new date, and the
git history of this file records exactly what moved. Continuing to use the public
relay after a change means accepting it.

## Law and contact

Italian law applies, and the courts of Italy have jurisdiction — without prejudice
to any mandatory protection available to you as a consumer in your country of
residence.

Contact: **lords82@gmail.com**. Security vulnerabilities go to the channels in
[`SECURITY.md`](SECURITY.md) instead. Licensing questions are in
[`LICENSING.md`](LICENSING.md).
