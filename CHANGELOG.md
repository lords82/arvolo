# Changelog

## v0.14.1 — nothing asks twice, and nothing waits for ever

Four things the app knew and did not say. A file already arriving was put in
front of you as a question; a refusal travelled as an English sentence another
program had to read; a send with no relay in reach waited for one silently, for
ever; and the id you hand to somebody was missing the half that says where you
listen.

### Fixed

- **A file already arriving was offered to you as a question, and the answer
  failed.** A trusted contact's file — or one from another of your own machines
  — is downloaded the moment it arrives, with nothing to decide. The daemon says
  so on the arrival it sends the window, precisely so a front-end can announce it
  instead of asking; the window read every other field and not that one. So the
  same file appeared twice, once downloading and once waiting for permission, and
  pressing Accept reached for an offer the daemon had already consumed: "no such
  pending offer", next to a transfer that was going perfectly well.

  The flag was also computed from trust alone while the decision behind it
  covered trust *and* our own devices, which is why two of your own machines
  disagreed the most reliably. Both now read one predicate, so the window and the
  daemon cannot answer the same question differently.

- **Offers the daemon disposed of by itself left a row behind.** Taking one
  because the sender is trusted, dropping one because the sender is blocked, and
  replacing one because the sender re-posted are all removals no window can see —
  and the daemon announced none of them. It announced only what a front-end had
  itself asked for. Each is now announced, so a row (and its button) cannot
  outlive the offer it stands for, rather than depending on the window guessing
  from the sender and the filename.

- **Accepting an offer that was no longer waiting reported a map lookup.** It now
  says what happened: the offer was already taken, or the sender replaced it with
  a newer one.

- **An automatic download that could not start took the file with it.** A trusted
  sender's deposit sealed with a password, or a direct offer arriving with P2P
  switched off, is *refused* by the engine rather than failed: nothing is taken,
  and the offer stays exactly where it was so it can be accepted again. The daemon
  threw it away anyway, printing a line to a terminal nobody had open. It now asks
  instead — the offer becomes an ordinary question, with the password field where
  the password goes. When the attempt really did consume the offer, it says so in
  a notification rather than in silence, since a file arriving automatically is
  deliberately not drawn as a row.

- **A send on a network with no reachable relay never left "preparing".** Before
  a ticket can name where to find the sender, the endpoint has to know how it can
  be reached, and asking iroh that means `Endpoint::online()` — which waits for a
  home relay and has no timeout. On a network where none answers (captive Wi-Fi,
  a firewall that passes one port) the wait simply did not end: the file was read
  and encrypted in milliseconds and then the row sat on a word that describes
  local work, for as long as anyone cared to watch. The wait is now bounded, and
  what it falls back to is this network's own addresses — which is the whole
  answer for a recipient in the same building. A send left with no way out at all
  says so: **"no relay reachable — local network only"**, rather than blaming the
  relay for being busy.

- **The app handed out an id with no relay in it.** Settings copies "your public
  id", and the id alone says who you are and not where you listen — so a person
  given one reaches you if their client happens to default to your relay, and
  silently not otherwise. `arvolo me` has always printed the pair (`<id>@<relay>`)
  and `contacts add` reads it back; the window now shows the same string, assembled
  by the daemon rather than by pairing two fields and a rule about dropping
  `https://`. Same for the id on the Devices screen.

### Changed

- **A refused accept now carries a reason, not a sentence.** `needsPassword`,
  `p2pOff` and `noLongerWaiting` travel as codes (`Response::Refused`, asked for
  per request so an older client keeps the plain error it can read), the way
  waiting reasons already did. The desktop app decided whether to show its
  password field by testing the engine's English for the word "password": that
  made rewording — or translating — a sentence into a behaviour change, with the
  only place a password can be typed as the thing at stake. It now reads the code,
  and falls back to the old test only against a daemon too old to send one.

## v0.14.0 — the relay stops being something only the sender can see

A transfer that leans on the relay used to depend on the sender being there
to say so. Now both ends can ask it directly, which turns the relay's copy
from a hint the sender passes on into a source the recipient can find
alone — and makes parking a file there as good a promise as depositing it.

### Fixed

- **A download could sit stalled while the relay held the whole file.** Which
  pieces the relay was holding travelled one way only: from the sender, over
  the control channel. So it was unknowable in exactly the situation the
  relay's copy exists for — with the sender gone there is nobody to say it,
  and a receiver resuming alone found no source for any piece while the relay
  had everything it needed. Both ends can now ask (`POST /v1/have`). The
  question discloses nothing new: the chunk protocol already answers it for
  anyone holding the hashes, and those are hashes of ciphertext under a
  per-transfer random key, so having the same file does not produce them.

- **A send whose recipient dropped kept serving an empty room for ever.** It
  now ends once everything they still lack is on the relay — and says so,
  which is what stops the same file going up a second time as a sealed
  mailbox blob. That second copy would have spent the relay twice and thrown
  away whatever the recipient had already pulled, because a mailbox fetch
  cannot resume. It also unstuck two things that could not run while the send
  was pinned: the wake-up when the recipient finally accepts, and the
  seven-day give-up.

- **Repeated drops could exhaust a transfer's relay budget without the relay
  storing a byte.** Every drop re-offered the whole undelivered tail; the
  relay skipped files it already had, but the meter was charged before it got
  that far. Seeding now asks first and sends only what is missing.

- **The relay's two stores were metered separately over one disk** — blobs in
  bytes, seeded chunks in rows, where fifty thousand rows stood for some
  800 GB. What arrived first was the filesystem running out. One budget now
  covers both. **If you run a relay, set `ARVOLO_MAX_TOTAL_BLOB_BYTES`**: it
  is the knob that now governs the whole disk.

- **A relay with no room said nothing.** Its refusal was dropped on the floor:
  the offload silently did not happen and the send went back to waiting with
  nothing on screen to distinguish that from a relay that was fine.

### Changed

- **"Park it on the relay now" parks the pieces instead of uploading a second
  copy** when the recipient is already holding a ticket for them — keeping
  whatever they had downloaded rather than starting them from zero. It makes
  the same three promises the mailbox does, which is new: the pieces last as
  long as the offer naming them (rather than a fixed day), a cancel really
  withdraws them, and the pickup is watched. Where a parked send cannot tell
  a pickup from an expiry — a recipient who swarms never releases the pieces
  — it says it does not know rather than reporting that nobody came.

- **A waiting send says why in the reader's language** for three more
  reasons: parked, the relay declining to hold more of this transfer, and a
  park that failed.

## v0.13.1 — the Mac and Windows apps, which v0.13.0 forgot

### Fixed

- **v0.13.0 shipped with no `.dmg` and no `.msi`.** The GUI binary had been
  renamed to `Arvolo`, and the CLI travels inside the bundle as a sidecar named
  `arvolo`, installed in the same directory. macOS and Windows compare filenames
  without case, so the two files are one name there and landed on top of each
  other: notarization refused the app ("the signature of the binary is invalid"),
  and WiX refused to link an installer holding the same file twice. The Linux
  AppImage, built on a case-sensitive filesystem, came out fine and hid it.

  Worse than the missing downloads: `latest.json` is assembled from the
  signatures that exist, so it went out naming `linux-x86_64` alone — every
  installed Mac and Windows copy was told there was nothing newer. **If you are
  on macOS or Windows, this is the release to install by hand**; the in-app
  updater will take it from here.

- **A relay with no room left now says so in words.** It answers `507` when it is
  holding as many bytes as its operator allows, and the client used to pass that
  through as "relay returned 507 Insufficient Storage" — the machine's condition,
  not yours. It now says that the relay is busy, that it passes, and that a relay
  of your own has no queue. Nothing about the retrying changed: deposits expire,
  the room comes back, and a direct transfer still happens the moment the
  recipient appears.

## v0.13.0 — a contact brings their own relay, and the app updates itself

Two features that ought to have shipped years ago, a fix for something that
should never have shipped at all, and a handful of smaller things that stop
asking you to think about them.

### Fixed

- **Every address-book snapshot an identity ever published went out under the
  same key and nonce.** The key was derived from the identity secret alone, so it
  was constant for the identity and identical on every device, and the nonce comes
  from the chunk index — which for a single-chunk snapshot is always zero. Two
  snapshots under one (key, nonce) pair is the failure AES-GCM does not survive:
  XOR the ciphertexts and the plaintexts XOR too, so anyone holding two of them off
  the relay learns where they differ. That is the address book. Sealing now mixes a
  random per-message salt into the key (`seal_under` in `core/src/sync.rs`), and
  reading still accepts the old shape so notes already in flight open.

  **Update your devices together.** A device on an older build cannot open the new
  form, and on failing it *deletes* the note — so a mixed-version pair does not
  sync until both sides are current. The desktop app now says so on the update
  banner rather than leaving it to these notes.

- **A device name typed into the list was thrown away by the two gestures people
  make after typing one.** The field committed on Enter and closed on blur, so
  pressing the Save button beside it blurred the field and ended the edit, and
  clicking anywhere else did the same — in both cases the name was gone before
  anything read it, with nothing on screen to say so and nothing ever reaching the
  daemon. The edit now commits in one place, reached from Enter and from Save
  alike, and the field stays open until one of the two buttons beside it is
  pressed.

### New

- **A contact can carry their own relay.** There is no federation between relays,
  so an offer has to land in an inbox the other person actually reads. An id can
  now name where that is — `<id>@her.relay` — and `send --to`, the online marker
  in `contacts list` and the mailbox notice all follow it. A mailbox deposit still
  uploads to *yours*, because the `arvm…` ticket names the relay it went to and
  you should not be spending someone else's storage. Mostly you never type it:
  pairing trades relays, and the first file someone sends you carries theirs. It
  is routing, not identity — getting it wrong costs a delivery that goes nowhere,
  never a file read by the wrong person.

- **The desktop app finds its own new version and installs it.** Once a week it
  asks whether there is a newer release, and if there is, one button downloads it,
  verifies it against a signing key compiled into the app, and restarts. Signed
  and notarized on macOS as before; the AppImage replaces itself; the `.msi` runs
  its own installer.

  Three things it will not do. It never downloads or installs without being asked
  — in a file-transfer tool an unrequested restart is destructive by definition.
  It says nothing while a transfer is running, and comes back when the board is
  quiet. And it can be turned off entirely, because that weekly request to
  github.com is the only third party Arvolo talks to; with it off, the button in
  Settings still works.

  The daemon is stopped before the installer runs and comes back with the app,
  which is not a detail: it runs from the files being replaced, and on Windows it
  holds them.

- **The relay says what version it is**, at `GET /v1/features`. The desktop app
  reads it before offering an upgrade, so it cannot talk somebody into the one
  order `docs/DEPLOY.md` asks them to avoid — clients ahead of the relay they
  depend on. Nothing today needs a newer relay and `compat.json` says so; the
  check is there for the release that does.

### Changed

- **The "clean up orphans" button is gone from the address book, and so is the
  chore.** It dropped advertised-name rows for identities no contact points at any
  more; its command-line twin went away in v0.12.0, where pruning became something
  that happens after a sync merge. Nothing produces those rows now — removing a
  contact clears its own row — so what was left was books written by older builds,
  which no merge would ever visit on a machine that does not sync. The daemon
  sweeps once at start instead, and nobody is asked to think about it.

- **Rename and forget moved into the ⋯ menu on each device row**, as everywhere
  else in the app, and the one that cannot apply to a row is shown disabled with
  its reason rather than left out. A device row belongs to the device that writes
  it, so renaming happens on the machine you are at; a machine that forgot itself
  would announce itself again on the next round, which is why the daemon refuses
  that one. An absent menu item cannot say either.

## v0.13.0-rc1 — fewer words, and they stay in the box

The words in the CLI and the GUI explained too much. Almost every string said
the true and useful thing in its first three words and then explained itself,
and the explanation is read once with interest and a hundred times after by
somebody who already knows it and is in a hurry. This prerelease is that
rewritten — everywhere, in all four languages — plus a layout bug the same
audit turned up.

Nothing is deleted, it moves: what happened and what you can do about it stay
in front of you, the reason why goes to the hint line in the GUI or to `-v` in
the CLI. `docs/COPY.md` is the rule and `gui/src/test/copy.test.ts` is what
fails when it drifts.

### Fixed

- **A relay could strand files that nothing would ever free.** Every cleanup path
  — the TTL reaper, burn-after-read, revoke, the seed-release the receiver calls
  per chunk — deletes a file by naming it from its metadata row. That can only
  free a file the database still knows about, and there are two ways a file loses
  that: a seed request that fails part-way leaves behind the chunks it had already
  pulled, and a crash between a write and its insert leaves a blob no row ever
  named. Neither is visible to anything that walks rows, so those bytes were held
  for good — on the public relay, one such chunk had been sitting there for
  fifteen days. A failed seed now removes what it wrote, and the reaper runs an
  hourly sweep that starts from the directory rather than the table, deleting any
  file with no row behind it that has been untouched for six hours.
- **A long id ran out of the box that was showing it.** An id is 52 base32
  characters and a ticket is several hundred, both with no space anywhere in
  them, and CSS has no break opportunity to work with in such a run: the box
  does not wrap it, it draws it out the side. Four places already defended
  themselves; everywhere else was one long value away from spilling. The worst
  of it was the sender's own id — an unknown sender has no fingerprint to show,
  so the raw id goes on screen through a component written for words with
  spaces — and the five cards that print a daemon error verbatim, where the text
  ran past the red border and was sheared off at the edge of the screen. Fixed
  on the containers rather than per screen, so the next long value is covered
  too.
- **Five messages named commands that do not exist.** `--deposit` had been
  `--mailbox` for long enough that a test was asserting on the old spelling;
  `arvolo code --foreground` had become `send --code --foreground`; `arvolo
  name` had become `arvolo me name`. A new test reads the source and checks
  every cited verb against the parser, so the next rename cannot leave a message
  behind.
- One error surfaced in Italian from an otherwise English CLI.
- **A download cap that was not a number was no cap at all.** "tre" parsed to
  NaN and fell through to the wire value for *no limit*, so a field the user
  believed said three published a link anyone could download for ever.

### Changed

- The shared-relay `docker-compose.yml` now sets `ARVOLO_MAX_TOTAL_BLOB_BYTES`
  (4 GiB). The per-blob cap it shipped with never bounded the disk — 20k entries
  of 0.5 GiB is not a budget — and the relay had been warning about exactly this
  at every startup.
- **A new application icon.** The mark is concept 3d, "Doppia V": two concentric
  V's, the small one dropping into the big one, with no box drawn around either.
  It replaces the wedge falling into an open box everywhere that one appeared —
  the macOS squircle and the full-bleed Windows/Linux master, the menu-bar
  template, the tray's attention frames, and the copy the relay's download and
  disabled pages inline by hand because they may not fetch anything. The boxed
  colour icons draw it at stroke 10.5 and the bare monochrome template at 12: a
  glyph sitting on its own coloured tile can carry the finer line, and the
  menu-bar mark — scaled down further, and tinted by the system — cannot. Below
  30px there is now a third cut, at stroke 12.5 with the glyph 16% larger, used
  for the 16px and 24px entries of the Windows `.ico` and the two 16pt layers of
  the macOS `.icns`. Two nested V's need more room than the single shape they
  replaced, and at 16px the regular cut lands on a one-pixel stroke that
  antialiasing all but erases.
- **The tray now breathes while something waits on you, instead of dropping three
  times.** There is no box left for anything to land in, so the small V drifts
  down toward the big one and back up, once every 3.6 seconds, for exactly as
  long as an offer or a name change is still unanswered. The old fall ran a fixed
  three times and then held a landed frame, which said "something arrived" but
  not "something is still waiting". Clearing the last request no longer cuts back
  to the resting frame either: the mark walks home the short way round, at most
  half a breath, so the return is never a jump.
- **The copy, throughout.** Buttons are verbs rather than sentences describing
  their own effect ("Trust", not "Mark as trusted: download automatically").
  The app no longer speaks in the first person about its own plumbing. Facts
  that were repeated across four or five screens are stated once, where the
  choice that depends on them is actually made — the relay's zero-knowledge
  property next to the relay field, the "whoever types this code becomes you"
  warning on the sheet where the code appears rather than three times across
  two screens. Nine near-variants of "compare the fingerprint out loud" are one.
- **Security warnings have a floor as well as a ceiling.** Six strings are the
  last thing read before something irreversible, and "Careful." would satisfy
  every length rule while warning nobody. Each carries a minimum and a note on
  what it must still contain.
- Length limits are written for English and scaled per language, so the German
  is not squeezed into English word lengths.

### New

- **The desktop app can send to your own devices.** `arvolo send --to me` has
  worked since v0.12.0-rc1, but the word `me` was resolved by the CLI before the
  daemon ever saw it, so the app — which talks to the daemon directly — had no
  way to name the destination: the recipient list only offered contacts. The
  daemon now resolves the reserved word itself, one rule for both, and "your
  devices" is the fifth entry on the send panel's switch — which answers "who is
  this for?", and your own machines are one of the answers: the one nobody has
  to accept, and the only one that is not a person. A transfer to yourself is
  labelled by name rather than by a raw id, and a contact who happens to be
  called "me" is addressed by their id so the reserved word cannot swallow them.
- **And it can be sent to *one* of them.** Until now "send to my devices" was a
  race: your machines share an identity, so they share an inbox, and whichever
  read the slot first took the file — sometimes more than one at a time, since
  they all unseal the same content key and co-swarm the download. There is now a
  register of the machines, and a send can name one.

  The register is a seventh entry in the CRDT that already syncs the address
  book, so it rides inside the same encrypted snapshot and the relay learns
  nothing from it. It has one rule the address book does not: **a live row is
  only accepted from the machine it describes**. Without that it would be an
  ordinary last-writer-wins map, and "send it to the MacBook" would have come to
  mean "send it to whichever machine last called itself the MacBook".

  Three things the screen and the docs say rather than hide. The list is
  self-reported: a machine that has never synced is missing, one that stopped
  syncing stays until you forget it, and "seen" is that machine's own clock, so
  it is shown as elapsed time and never as an online light. Aiming is honoured by
  your devices, not enforced by the relay — a client too old to know the field
  takes the offer as it always did. And `device forget` is bookkeeping, not
  revocation: the machine keeps the identity secret, which nothing can take back;
  what makes it stick is that the forgotten machine sees it was retired and stops
  announcing itself, because it is your machine running your software.

## v0.12.0 — no silent stalls

Four prereleases, all driven by dogfooding one stubborn 10.7 GiB `send --to`.
The theme, in the end, was silence: a transfer that hangs without an error, a
sender that reports a downloader doing nothing, an accept that lands in the
middle of a five-minute backoff and wakes nobody. Everything below is about
noticing — and acting — instead.

### Fixed

- **A peer that went quiet parked a task on both ends of a chunk transfer — for
  good.** QUIC keep-alives run every second, so a connection with nothing moving
  on it is never "idle" by the transport's reckoning: it stays alive as long as
  both processes do, and "connected" says nothing about anything happening. Both
  sides had an unbounded await on the other saying something — the receiver's
  chunk-body read, the sender's wait for a request on an opened connection — and
  each turned a silent peer into a task parked for ever: a 10.7 GiB download sat
  at 97.4% for hours, no error and no progress, while the sender counted a
  connected downloader the whole time. Every wait is now bounded — 30s of silence
  for a first request or mid-body, 90s to reach a provider at all (connecting
  across the internet is a slower, different thing than a stream going quiet),
  five minutes of quiet for a connection that has already served — and each bound
  turns silence into an ordinary error, which is the one form the scheduler's
  existing recovery (cool the provider down, re-queue the piece, reassign) knows
  what to do with.
- **"1 downloading" could mean a peer that had done nothing for minutes.** The
  count behind the CLI's "N downloading" and the GUI's "N people downloading it"
  counted open chunk connections — but a healthy receiver opens a fresh
  connection per 16 MiB piece, so the only connection that stays open for long is
  precisely the hung one: the metric was at its most stable when it was least
  true. A peer now counts only while it shows signs of life — a request arriving,
  or bytes still leaving for it mid-piece — and drops out of the count ten
  seconds after the last one. The connection bookkeeping underneath is unchanged;
  what changed is who gets called a downloader.
- **A trickle could hold a chunk for ever.** The silence bound restarts on every
  byte, so one byte every 29 seconds passed it indefinitely — and the scheduler
  never reassigns a piece that still has a "working" source. The body read now
  also enforces a floor on the average rate (1 KiB/s, judged only after 30
  seconds of grace): a trickle is not silent, and not alive either. Deliberately
  a floor and not a deadline — a deadline is the thing that turns a slow-but-real
  link into a broken one, and any link that can ever finish a 16 MiB chunk clears
  this floor by an order of magnitude.
- **Fallback providers were tried one at a time.** A chunk whose first source was
  dead paid that source's full 90-second open bound before the second was even
  dialled, so the worst case grew linearly with the number of sources — all of it
  spent knowingly waiting on timeouts. Spare providers now join the race after a
  few seconds' stagger each, and the first verified chunk wins; a healthy first
  provider still answers long before the second is dialled, so the common case
  costs exactly what it did.
- **Every 16 MiB chunk paid a fresh QUIC handshake.** The receiver opened a new
  connection per chunk — ~680 of them for a 10.7 GiB file, each a round trip of
  pure ceremony that high-latency links felt keenly. Connections now go back
  into a per-provider pool on success and the next chunk rides one that is
  already warm; a pooled connection the server has since hung up on is quietly
  discarded and redialled, never mistaken for the provider being down. The
  relay's backfill node pools the same way — it pulls hundreds of chunks from
  one sender. Parallel fetches still ride parallel connections, so the sender's
  one-request-at-a-time serve loop never becomes a shared bottleneck.
- **Every client asked the relay for its inbox every two seconds, forever.** The
  long-poll ended the moment the slot was non-empty — and the contact-sync cell
  sits in that slot permanently, so for anyone with sync on it ended immediately,
  every round, and a 25-second hold became 30 requests a minute (held down only by
  a floor on the client side, added after this pegged a relay at >700 req/s). A
  poll now says which rows it already holds and the relay waits for one it does
  not: about 2.4 requests a minute instead. The relay has to be updated first —
  an older one ignores the header and answers exactly as it did before.
- **A held poll cost the relay a database scan twice a second.** Its steady-state
  work grew with the number of *connected* clients rather than with the number of
  deposits: ten thousand idle inboxes meant twenty thousand queries a second, all
  of them answering "still nothing". A deposit now wakes exactly the slot it
  landed in, which also takes the delivery latency of an offer arriving mid-hold
  from up to half a second down to none.
- **Accepting a big file left it sitting at 0 B for minutes.** A file the relay
  refuses as too large can only go peer-to-peer, so the sender keeps trying while
  the recipient is away — on a backoff that grows to five minutes. Nothing told it
  the recipient had said yes, so an accept landed in the middle of that sleep and
  both ends showed a transfer that was doing nothing. The sender now spends the
  wait held open on its offer's status, and the relay answers the moment the
  recipient touches it: accepting starts the transfer in a round trip. Both kinds
  of recipient reach it — the daemon, which acks on accept, and a bare
  `arvolo recv`, which only lists its inbox before dialling.
- **An offer for a file already downloading asked for approval again.** A held send
  re-offers itself — after a pause, after a restart, after any attempt that found
  nobody — and every one of those offers used to park as a fresh decision to make:
  one inbox was holding three of them for a 10.7 GiB file while that very file was
  downloading at 10.4 GiB. An offer whose ticket serves what a download already has
  in flight (or paused) now goes to that download instead of to the user, sweeping
  the copies already parked for it, and is acked so the sender wakes and serves
  rather than waiting on a row nobody will answer. It is a strong match, not a
  guess about filenames: the digests are of ciphertext under a random per-transfer
  key, so producing them means being that send. Anything else — another file, or
  the same content from a different sender — still asks.
- **Every delivery attempt left another copy of the same offer.** Each attempt
  posted a fresh offer and withdrew it on the way out, so a recipient who was away
  for a while collected a row per attempt, all but one of them already dead — and
  accepting any of the dead ones named an offer the sender had stopped listening
  for. A held send now keeps one offer standing for its whole life and withdraws
  it once, when the send really ends.
- **Pausing a send and resuming it re-encrypted the whole file.** The preparation —
  the chunk digests, and the content key and node id they belong to — lived in the
  delivery task, and a pause is what ends that task: resuming paid the pass again
  (half a minute for 10 GB) and, worse, minted a fresh key and node id. The
  recipient's transfer was then pointing at a node nobody was serving, and the only
  way back was another offer for them to approve by hand. The preparation now
  belongs to the held send, which survives the pause, so resuming carries on with
  the same content under the same node id. It is also written down beside the
  send's own record, so a daemon restart resumes the same send too — 22 KB of
  digests for a 10 GB payload, guarded by what the file looked like when they were
  taken (length, mtime, and the inode where there is one). A payload that moved on
  in the meantime is simply prepared again, as it always was.
- **A file from another of your own devices asked for approval.** Two halves of
  `--to me` shipped in rc1 were written but never wired up. An offer that opens
  and authenticates as your own identity can only have been sealed by something
  holding your identity secret — your other device — so it now downloads without
  asking, in all three places that decide (the daemon, `recv` attached to it, and
  standalone `recv`), rather than asking your permission to receive what you just
  sent yourself.
- **`send --to me` offered to pair devices that were already paired.** The marker
  a completed `device pair`/`device join` is supposed to leave was never written,
  so until the first sync round landed a snapshot from the other side, a
  freshly-paired identity still looked unpaired.

### New

- **The name a peer introduces themselves with is readable before approving
  them** (GUI): it is on screen exactly when you decide, not after.

## v0.12.0-rc4

Fourth prerelease. All of it is about a held `send --to` — a file too large for the
mailbox, which can only move while both ends are awake — and all of it came out of
dogfooding a 10.7 GiB transfer that kept stalling.

- **Accepting starts the transfer in a round trip**, instead of somewhere inside a
  backoff that grows to five minutes. The sender now waits held open on its offer's
  status, and the relay answers the moment the recipient touches it.
- **One offer per send, not one per attempt.** A recipient who was away used to
  collect a row per delivery attempt, all but one of them already dead.
- **Pausing no longer throws away the preparation.** It used to live in the delivery
  task, which is exactly what a pause ends: resuming re-read and re-encrypted the
  whole payload *and* minted a fresh key and node id, so the recipient's transfer was
  left pointing at a node nobody was serving. It now belongs to the held send, and is
  written down beside it (22 KB of digests for 10 GB) so a daemon restart resumes the
  same send too. Measured on a 3 GiB payload: 60 seconds of preparation before,
  nothing after — and the recipient's download simply carried on, across both a pause
  and a restart of the sender.
- **An offer for a file already downloading no longer asks for approval again.** It
  goes to the download it belongs to, sweeping the copies already parked for it.
  Anything else — another file, or the same content from a different sender — still
  asks.

Both ends want updating: the wake-up needs the relay, and not asking twice needs the
recipient.

## v0.12.0-rc3

Same code as rc2, plus the fix that lets a prerelease ship Linux packages at all:
the .deb files were being built and then discarded when the .rpm in the same step
rejected the hyphen in the version. Both now get `0.12.0~rc3`, which is how both
ecosystems spell "sorts before the final release".

## v0.12.0-rc2

Second prerelease, on top of rc1.

- **Preparing a send now uses several cores.** Reading stays sequential — that is
  what a disk wants — while sealing and hashing the chunks runs on four threads.
  `seal_chunk` is a pure function of `(key, index, bytes)`, so the digests are
  identical to the single-threaded ones; only the wall clock changes. Measured
  132 → 411 MiB/s, which takes a 10.7 GiB file from 83 seconds to 27 before the
  recipient is offered anything.
- **`--to me` is wired up end to end**: the two halves of sending between your own
  devices had been written but never connected.
- **The relay's inbox long-poll holds properly**, and a deposit wakes it.
- A `cargo fmt` sweep: the check had been failing for a while.

## v0.12.0-rc1 — cancelling, and saying what is happening

A prerelease off the work branch, for dogfooding.

### Fixed

- **A live send could not be cancelled.** Marking a send `Active` went through the
  setter that drops the transfer's cancel token, so from the moment it started
  serving nothing could stop it: `cancel` found no token, did nothing, and said
  nothing. The window sat on "cancelling…" until it was restarted.
- **Nothing bounded a request to the daemon.** No timeout anywhere in the IPC
  client, so a request the daemon never answered hung its caller for good.
- **Cancel is now total**: every live transfer either reaches a terminal state or
  has a task that will announce one, and the daemon says so instead of answering
  `Ok` for an id it has never heard of.
- **A cancel during a mailbox upload** was only noticed once the upload finished.

### New

- **"Preparing…"** — a send spends its first minutes reading and encrypting the
  payload, with nothing on the wire and the recipient not yet told anything. It
  said "active, 0 B", which is what a stuck transfer looks like.
- **That preparation is no longer repeated per attempt.** A delivery loop retrying
  against a recipient who has not connected yet reused to redo the whole
  read-and-encrypt pass every time — about a minute and a half per 10 GB — and
  minted a new ticket each round, so an offer already in the recipient's inbox
  went stale. It is computed once and carried.
- **Save the sender while their file is arriving**, in the CLI and the GUI: their
  id is on screen exactly once, and that is also the only moment you know who
  they are.
- **`daemon status` names the process**: pid and binary, in the CLI and in the
  GUI's settings, above the button that restarts it.
- **Direct sends between your own devices** (`--to me`), with a device id in the
  beacon.

## v0.11.0 — the CLI, simplified

One verb sends, one verb receives. The surface drops from 18 top-level commands
to 14, output becomes pipeable, and every id a person types is the same 8-hex
handle. **This is a breaking release: the old verbs are gone, with no aliases.**

### Command mapping

| Before | Now |
|---|---|
| `arvolo send <who> <paths…>` | `arvolo send <paths…> --to <who>` |
| `arvolo send … --deposit` | `arvolo send … --to <who> --mailbox` |
| `arvolo link <paths…>` | `arvolo send <paths…> --link` |
| `arvolo code <paths…>` | `arvolo send <paths…> --code` |
| `arvolo ticket <paths…>` | `arvolo send <paths…>` (writes a `.arvolo` ticket file; `--ticket` prints the raw string) |
| `arvolo accept <26-char offer id>` | `arvolo recv <8-hex handle>` (unique prefix is enough) |
| `arvolo reject <26-char offer id>` | `arvolo decline <handle>` |
| `arvolo listen --yes` / `--auto-accept-*` | `arvolo listen --accept contacts\|verified\|all` |
| `arvolo daemon` | `arvolo daemon start` — plus `run` (foreground, for systemd/launchd), `stop`, `status` |
| `arvolo contacts pair [code]` | `arvolo contacts add <name> [code]` (no code: shows one and waits) |
| `arvolo contacts block` (no arg) | `arvolo contacts list --blocked` |
| `arvolo contacts accept-name <n>` | `arvolo contacts rename <n>` (no new name adopts the pending one) |
| `arvolo contacts unverify` / `untrust` | `arvolo contacts verify\|trust <n> --undo` |
| `arvolo contacts prune` | removed — pruning runs automatically after a sync merge |
| `arvolo contacts export` | removed — `arvolo contacts list --json` (import reads it as-is) |
| `arvolo history --all` | removed with the 20-row cap: `history` prints everything, pipe it |
| `arvolo cancel --token <t>` | removed — the token it asked for was never printed anywhere |
| `--use-http` (8 commands) | removed — write the scheme in the address: `--relay http://host:port` |
| `link --password` | removed — it was documented but always failed |
| config `debug` / `ARVOLO_DEBUG` | removed — use `-v`/`RUST_LOG` |

### New

- **`.arvolo` ticket files.** A bare `arvolo send file` writes `file.arvolo` —
  share it over any channel, like a .torrent; the other side runs
  `arvolo recv file.arvolo` (or double-clicks it: the desktop app registers the
  extension, opens on it, and can save one from a ticket result).
- **One handle shape.** Everything you type back is 8 hex chars with unique-prefix
  matching: transfers (restart-stable now — the old numeric ids were reassigned on
  every daemon restart), relay deposits, resumable sends, waiting offers. Tickets,
  codes, links and public ids stay what they are: capabilities.
- **Pipeable output.** stdout carries exactly the artefact — the URL, the code,
  the ticket, the saved file's path — and all narration goes to stderr.
  `arvolo send --link f | pbcopy` copies the URL and nothing else.
- **`--json`** on `status`, `history`, `device status`, `daemon status` (contacts
  list already had it). Handles included; secrets never.
- **Progress everywhere**: the mailbox download used to be silent until the end;
  the live send used to smear `\r` into logs. One renderer, bar on a terminal,
  milestone lines in a pipe.
- **Bare `--password` prompts on the TTY** instead of demanding the secret on
  argv (`--password=<pw>` stays, for scripts).
- Parse-time conflicts: the flag combinations that make no sense
  (`--link --code`, `--to --ticket`, `--keep` without `--code`, …) fail
  immediately, with the reason.
- Received files land in the configured download dir through **every** door, and
  derived names never overwrite — an existing `report.pdf` makes the new one
  `report (1).pdf`.

### Fixed

- Desktop notifications no longer fire twice while the app is open (a bitwise-NOT
  bug made the daemon notify unconditionally).
- `arvolo me name` now takes effect immediately when the daemon runs — it used
  to keep advertising the old name until a restart.
- `--ttl/--max/--password` are no longer silently dropped when a send is handed
  to the daemon and the recipient goes offline between the two presence probes.
- Removing a contact from the desktop app now clears its verified/trusted marks,
  exactly as the CLI does.
- Accepting or declining an offer from the CLI now removes its row from any open
  window (new `offer_gone` event).
- Resumable-send records are deleted on delivery and swept after 30 days — the
  list used to grow forever.
- The first-run wizard no longer claims "no relay set" while a built-in default
  exists; `arvolo status` names the built-in relay before first contacting it.
- Typing "Yes" at a y/n prompt no longer means no (one prompt had a
  case-sensitive accept list).
- **Test builds move data at full speed**: dependencies now compile at
  `opt-level 2` even in dev, ending the ~120 KB/s crawl that made every
  bulk-transfer test flaky on some machines.

### Upgrade notes

- **Re-run `arvolo completions <shell>`** — the completion script and the binary
  are versioned together, and the verbs changed.
- **systemd/launchd units**: the daemon verb is now `arvolo daemon run`
  (templates in `packaging/` are updated; edit any unit you deployed by hand).
- The desktop app must be updated together with the CLI it drives (same
  workspace version, as always).
