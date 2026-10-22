# Arvolo on phones — design

> Status: **design decided** (2026-08-17), **scope widened to desktop parity**
> (2026-08-27), implementation not started. The calls at the bottom were made
> by the project owner; the rest of this document is the reasoning they rest
> on.

## The one constraint that shapes everything

On a phone the app cannot stay listening. iOS suspends it seconds after it
leaves the screen; Android is gentler but headed the same way. So the desktop
model — a daemon holding a live inbox subscription — does not port on its own
(Android can buy it back with a foreground service the user turns on; iOS
cannot, at any price). What does
port, remarkably well, is the **mailbox**: Arvolo's offline path *is* the
architecture Signal-class apps use on mobile. A send to a phone is a sealed
deposit; the phone is told to wake up; it fetches over HTTPS, decrypts
locally, and shows a notification. The phone is a **mailbox-first client**:
store-and-forward is the norm, live P2P the foreground exception.

Concretely:

- **Receive** = push wake → fetch the sealed offer/deposit from the relay
  (plain HTTPS — background-friendly on both platforms, and on iOS a
  background `URLSession` can carry a large download) → decrypt on device →
  local notification with sender and filename. The push itself carries
  **nothing**: no name, no size, no sender — a content-free "check your
  inbox". A sender's watchdog already falls back to the mailbox when the
  recipient is not live; from the sender's side a phone just looks like a
  contact who is often offline. Tombstones (*pending / arrived / taken*)
  already tell them what happened.
- **Live P2P** (codes, big local transfers, LAN) runs **only in the
  foreground**: iroh ships first-party iOS/Android support, but a QUIC
  endpoint is a battery cost and iOS kills it in background anyway. Open app →
  scan or type a code → direct transfer with progress. Leave the app mid-code
  and the transfer pauses and resumes — the resume machinery exists.

## The push problem, honestly

Apple allows instant pushes **only** through APNs, and APNs credentials are
tied to the app's publisher. A self-hosted relay cannot reach an iPhone by
itself — every self-hosted E2E product hits this wall (Matrix runs Sygnal,
ntfy relays through ntfy.sh). The workable shape, and the one that keeps the
zero-knowledge promise intact:

- The relay gains one optional duty: on a deposit for a registered device,
  forward a **content-free wake** `{push_token}` to a push gateway.
- **`arvolo-push`**, a small vendor-run gateway (CryCatch), holds the APNs and
  FCM credentials and forwards wakes. It learns *a token woke at time T* —
  never who sent, what, to whom, or from which inbox slot. Source stays open;
  an enterprise shipping its own app build under its own bundle id can run its
  own gateway with its own keys.
- **v1 ships the store path**: the vendor gateway, APNs + FCM. **UnifiedPush**
  (a self-hosted distributor instead of FCM) belongs with the no-store
  distribution road and waits for it — see *Distribution* below.
- **iOS fallback** when push is declined: periodic `BGAppRefreshTask` inbox
  polls, and a full check on every app open. Slower, never broken.
- **Android does not have to settle for wakes.** An opt-in **foreground
  service** (Syncthing-style, off until the user turns it on) holds a live
  inbox subscription with the screen off — that is the desktop daemon, on a
  phone. It costs a permanent notification and a Play Store justification, and
  it buys real parity on the one platform that allows it. iOS cannot have it;
  there, mailbox-first is not a preference but the floor.

## Stack

**Tauri 2 mobile** (recommended; final call pending):

- `arvolo-core` is embedded **in-process** — on mobile there is no daemon to
  spawn. The engine gets a library face: the same command surface
  `gui/src-tauri/src/bridge.rs` speaks over the socket today, backed directly
  by core. That refactor is the heart of the port, it is pure Rust, and the
  desktop app later benefits from the same seam.
- The React frontend ports with a phone layout: the views, store, i18n and
  design system carry over. Same codebase, responsive — but see below: this
  is an adaptation designed on purpose, not a window squeezed until it fits.
- Native edges stay native, and are contained: an iOS **Notification Service
  Extension** (fetch + decrypt inside the ~30 s push budget, or at least name
  the sender from the sealed offer) and **share extensions** on both platforms
  (send from the gallery / Files / any app) are separate Xcode/Gradle targets
  next to the Tauri shell — the community has mapped this road
  (`tauri-plugin-mobile-push`, `tauri-plugin-mobile-sharetarget`, xcodegen
  overrides).

Considered and set aside: **native twice** (SwiftUI + Compose over
UniFFI-wrapped core — best platform feel, double UI forever; wrong trade for a
one-maintainer project), **Flutter / React Native** (new stack, design system
rebuilt, still needs the same native edges).

## The phone GUI is a redesign, not a resize

Today's interface assumes a 980×670 window, a pointer and a keyboard. None of
those exist on a phone, so the adaptation is designed deliberately:

- **Navigation**: the rail becomes a bottom **tab bar** — *Attività*, *Persone*,
  **Scambia** in the middle, *Invia*, *Impostazioni*. Six rail places don't fit
  five tabs, so they fold rather than drop: Transfers and History are the two
  halves of **Attività**; "Link e depositi" is the tray glyph on that screen and
  a row in Impostazioni; "I tuoi dispositivi" is in Impostazioni and, for
  everyday use, already inside Scambia. Nothing is lost — see *Parity with the
  desktop* below.
- **The middle of the bar is a camera, not a sheet**: *Scambia* opens the
  viewfinder already looking, with your machines, your own code and a typed
  code in a sheet under it. *Invia* is the one entry in the bar that does
  something rather than going somewhere. Drag-and-drop disappears; the platform
  **share sheet** and file/photo pickers replace it.
- **Sheets are cards** (the screen behind steps back, a pull puts them away)
  and the camera's sheet has two detents; `⌘K` and every keyboard shortcut
  disappear (search is a dock at the bottom); hover states become press
  states; row actions move into swipe actions and a held row's context menu.
- **Touch and platform**: 44 pt minimum targets, safe-area insets (all four
  sides), the keyboard measured and kept clear of, pull-to-refresh on every
  root screen, Dynamic Type, haptics on the system's own occasions.
- **Tablets are the same app, wider**: past a breakpoint the tab bar goes back
  to being a rail and the sheets become two columns. Same features, one
  codebase, no separate tablet design.
- **What carries over untouched**: the store, the IPC command surface, i18n
  (all four languages), the design tokens/theme, and the component library —
  restyled where touch demands it. The claude.ai/design project holding the
  26 synced components is where the phone variants get designed first.

**The design of record** is `gui/.design-sync/mobile/scambia.dc.html` — a copy
of the "Arvolo Mobile 2 — Scambia" canvas on claude.ai/design, built from the
real design system bundle. It supersedes `casella.dc.html` beside it, which is
kept for the reasoning it carries. Where parity and the canvas disagree, parity
still wins and the screen gets redrawn.

What that canvas settles, and what the app is built to (2026-10-02):

- **Five places: Attività, Persone, _Scambia_, Invia, Impostazioni** — and the
  disc in the middle is a *place*, not a sheet. It opens the camera already
  looking, because the two things a phone is picked up for are showing a square
  and reading one.
- **Scambia is the camera with a sheet over it**: your own machines (a tap sends
  to one — no door to open, no fingerprint to compare), your own code with how
  much is already waiting to be given, a cold row for anything one of your
  machines has left you, the people you met recently, and last the keyboard.
- **Attività is one list in time order**, not two directions: on a phone a
  direction is two tinted words at the start of a line. Waiting decisions first,
  then what is in flight, then what finished today; *Cronologia* is the other
  half of the same segmented control rather than a place of its own.
- **Link e depositi** is reached from the tray glyph on Attività (and from
  Impostazioni); **search** is the magnifier beside it, which is the only search
  a phone has — there is no field in the bar to put one in.

Three things in the canvas the platform or the engine does not support yet, and
they are the open ends rather than oversights:

- **No torch button.** `tauri-plugin-barcode-scanner` exposes no torch.
- **A square read in Scambia is always tried as a transfer.** A contact-swap
  code and a send code are the same shape on the wire, so nothing local can tell
  them apart; swapping contacts stays a button in *Persone* (7k's "inquadrare da
  Scambia fa lo stesso" needs the daemon to try both).
- **A device card cannot say "same network" or "off, via relay"** (5a), and the
  cold row cannot name *which* of your machines left you something. Presence is
  a relay beacon per **identity**, and all your machines share one; `last_seen`
  is a sync round, explicitly not a presence light. The cards say when a machine
  was last seen, in words. Per-device presence needs a device id in the beacon —
  the same thing device-targeted sending already wanted.
- **Section 6 (photos and videos into the Gallery) is not built.** It needs a
  publisher that can write to MediaStore / Photos and a preference the engine
  honours; nothing in the GUI can stand in for it.

### Sections 11 and 12: one road, three places (2026-10-04)

The canvas grew two sections and the owner asked for the app to match them —
**and, asked explicitly, to match the canvas's own controls too**, which undid
the one-day iOS 26 "Liquid Glass" experiment of 2026-10-03 (flat bar, bordered
buttons and full-screen sheets are back; what stayed is the behaviour the
canvas does not draw: pull-to-refresh on root screens, Dynamic Type through
`--dt`, the keyboard measured into `--kb`, dragging the bottom sheets, haptics
on the system's occasions).

- **The bar is three places** — Attività, **Scambia** in the middle (disc 60,
  overhanging 22), Persone. *Invia* is gone: it and Scambia asked "a chi"
  twice. **Impostazioni is behind the gear** every root screen carries at the
  top right (`.root-head` / `.act-tool`), as a pushed screen with the title on
  the page (12b) and no bar under it.
- **Scambia is the only place a send or a receive starts from** (11a):
  *Ce l'ho qui davanti* (a black card; the camera is behind it, `CameraView`,
  route `camera`), *I tuoi dispositivi* (three cards), *Persone* (three faces
  and *Rubrica*), *Chiunque abbia il link*. Two questions in a fixed order —
  with whom, then what — each asked once: from Condividi the files are already
  in hand (11e, `composeFiles`), from a person's row the recipient is
  (`startCompose`).
- **Cosa is one screen** (`ComposeView`, route `compose`, 11b–11d): the
  recipient block, a 3×2 grid of the last pictures (`photos_recent`, PhotoKit
  on iOS — Android still answers "no library", so the grid is simply absent
  there), *File* and *Appunti* (`clipboard_peek`/`clipboard_pick`: the
  clipboard as a `.txt` or `.png`), the line that says what will happen, and
  one button that says verb, quantity and recipient. The transport is the app's
  choice among the sealed ones; the link is public and therefore its own
  answer with an ink button and a "Link pronto" sheet. *Altri modi* (codice,
  ticket, casella) opens the full sheet, only for a person.
- **Link e depositi live in Attività** as the middle half, *Lasciati* (12a):
  public links and what waits in mailboxes, a tap on a row for the link, the
  revocation or the details (the 7c action sheet). Impostazioni holds only
  settings: who you are, I tuoi dispositivi, Ricevuti, Aspetto e lingua, Rete,
  Avanzate (12b).
- **The phone's palette is iOS's** (canvas tweak `data-palette="ios"`, taken
  2026-10-04): grouped background `#f2f2f7` with white surfaces, black ink,
  the system blue and orange for the two directions, the system font; dark is
  pure black with `#1c1c1e` surfaces. All of it under `.app-phone` in
  `theme.css` — the window keeps its warm paper. It replaced the canvas's
  earlier "dark v2" phone palette.
- **Share from another app** (11g's third road, 2026-10-04): Arvolo is in the
  system share sheet on both platforms. Android: an `ACTION_SEND` /
  `ACTION_SEND_MULTIPLE` intent filter and `Shared.kt`, which copies the
  `content://` streams (and shared text, as a `.txt`) into the app's storage
  while the grant is live and calls `Java_it_termox_arvolo_Shared_onShared`.
  iOS: a share-extension target (`ShareExtension`, Swift) that copies the
  attachments into the App Group `group.it.termox.arvolo` and opens
  `arvolo://shared`. Both land in `src-tauri/src/share_inbox.rs`, which queues
  the paths and nudges the page (`shared://arrived`); the page *pulls* with
  `take_pending_shared` at launch, on coming to the front and on the nudge —
  a share can land before any page exists to listen (Android's cold start, or
  an extension that could not open the app). The files are adopted into the
  picked-copies folder and registered like a pick; Scambia opens with them in
  hand (11e). **The native halves live under `src-tauri/mobile/` and are put
  into the gitignored `gen/` by `npm run mobile:patch`** — run it after every
  `tauri android|ios init` (it also carries the iOS script-phase path fix in
  `project.yml` and regenerates the Xcode project).
- **A picked file outlives a restart** (2026-10-04): on a phone every pick,
  descriptor or not, is copied into the picked-copies folder before the send
  is queued, so a send still waiting for its recipient when the app is killed
  resumes from a path that is still there. The fd hand-off stays the desktop's.
- **A mailbox pickup cannot be lost to a crash** (2026-10-04): the client
  fetches with `?hold=1` and acks (`POST /v1/fetch/{claim}/ack`) only once the
  file is written, and the inbox offer is acked at the same moment rather than
  on accept. An app killed mid-fetch comes back to the offer still in its inbox
  and the deposit still on the relay (relay ≥ the build that advertises
  `"hold":true` in `/v1/features`; an older relay burns on read as before).
- **Failures say why in the reader's language** (2026-10-04): `FailReason` in
  core carries a stable code beside the sentence (`sourceMissing`,
  `recipientCancelled`, `expiredOnRelay`, `depositGone`, `noSpace`,
  `network`); the row, the event and the history record carry it, the GUI
  translates it (`failed.*`, guarded by `failed.test.ts`), and anything without
  a code still shows the engine's own words.
- **Left out on purpose, to be asked of the canvas**: *Accetta
  automaticamente*, the typed code («Hai un codice scritto?») and «Prendi
  quello che un tuo dispositivo ti ha lasciato» — the owner chose not to place
  them until the design says where. And a square read in the camera is still
  always a *receive*: «Inquadra e partono» (11e) needs the engine to tell a
  contact code from a send code.

### Sections 13–16: Scambia remade, with the TermoX identity (2026-10-04, evening)

The canvas moved on four times in one afternoon and the last word wins. The
chain, for anyone reading the frames: 13 ("La schermata di invio, rifatta")
put people and devices in one grid and gave the search field the job of
reading a received code; 14 ("Invio, più carattere") brought the TermoX
identity — night blue `#090735`, water blue `#3C78BF`, Plus Jakarta for titles
and numbers, Inter for text, a cold grey ground — and made the bar a floating
island; 15 ("Il blu come in 14b, e l'invio non blocca") kept the night blue to
the camera card and the send sheet and moved the transfer out of the way into a
card over the bar; 16 ("Più trasferimenti insieme") made that card one card for
any number, and let one send go to several people. 14a/14c/14d/14e are
superseded by 15; 13a by 15a; 13b by 14b; 13c ("Altri modi": the spoken code
and the ticket) has no entry point left in 15a and is **not built**.

What the phone does now, screen by screen:

- **Scambia (15a).** A navy camera card ("Ce l'ho davanti / Inquadra il suo
  quadrato") opens the windowed scanner. Under it a field that filters the two
  grids by name and, when what is typed has the shape of a code (`dddd-word-word`,
  `arvc…`, `arvm…` — `shapeOf` in `ReceiveSheet.tsx`), turns into the 13e box:
  what it recognised, a password field for a mailbox ticket, and *Ricevi*. Then
  the Prendi row when one of your own machines left something the daemon did
  not take by itself (parked offers whose sender is your own id); then *I tuoi
  dispositivi* as 64px squares (green dot when seen in the last eleven minutes,
  a blue count when something of theirs is waiting, a dashed *Link* square last)
  and *Persone* as 64px discs (blue ring and dot when the relay says online,
  "in linea" / when you last exchanged / nothing under the name). Eight faces,
  then *Rubrica*.
- **More than one person (16a).** Holding a face turns the faces into boxes and
  shows every contact; the navy bar over the island counts them and says how
  many are online; *Avanti* goes to *Cosa* with `{kind: "group"}` — one send per
  name, same message, same options.
- **Cosa (14b).** Back, the capsule with who (ring on when online, options
  behind the sliders), the three chips — Foto (the library, sixty tiles,
  numbered in the order they are tapped), File (what is in hand plus "Scegli un
  file…"), Appunti — and the navy sheet at the foot: two tilted thumbnails, the
  count, the "how it arrives" line, the message field (people only) and the
  round send button. After the send you are back on Scambia; the card says the
  rest.
- **Opzioni (13d).** TTL (1h/1d/7d/30d), pickups (a stepper), password, and
  apart from them *Aspetta che sia collegata*. The mailbox rules make the send a
  `deposit` (the live path cannot carry them); the switch makes it a
  `send … live_only`, a new flag through IPC → `TransferManager::send_to` that
  keeps the delivery loop off both relay routes — no parking, no mailbox — and
  reports `WaitingReason::ForThem` while the recipient is away. The flag is
  persisted with the held send (`SendToRecord.live_only`; the previous record
  shape still decodes).
- **The card over the bar (15b–15d, 16b–16d).** `TransferDock` in
  `views/TransferCard.tsx`, mounted by `App.tsx` on every root. It shows what
  is moving (`active | preparing | offered | stalled | paused`) and, for four
  seconds, what just stopped moving — green ring and tick, "Arrivato a Sara",
  *Fatto* — then leaves; the row stays in Attività. Several rows pile their
  rings (light blue arriving, blue leaving, green arrived) and the text says who
  is in the mailbox. Tapping opens the sheet: one transfer gets the big
  percentage, its file row, Pausa/Annulla; several get the total, *In uscita* /
  *In arrivo* lanes, and a row tap opens the row's own actions. The page keeps
  `--dock` of room under it while the card is up.
- **The island (15a).** `.tabbar` is a 64px white capsule 22px in from the
  sides, Scambia a navy pill that always carries its name, a blue dot over
  Attività while something is moving or waiting.

Palette and type live under `.app-phone` in `theme.css`. The canvas declares its
own palette as `--x-*` variables in the helmet, in two sets, and both are
mirrored there. The light one is the TermoX identity: night blue `#090735`,
water blue `#3C78BF`, cold grey ground `#f7f8fa`.

**The dark one is neutral, not navy**: ground `#0c0c0e`, surfaces `#17171a` and
`#222226`, ink `#f2f2f4` — and the filled card (`--x-navy`, the camera card, the
send sheet, the transfer card) becomes `#1f1f24`. Night blue is a colour of the
light: on a black screen a blue card is the only coloured thing there is, so the
canvas turns it off. What stays lit is only what lives *on* the card and is the
same in both: `#3C78BF`, the scan line `#5a93d2`, the green `#3fbf8a`, the light
blue of what is arriving `#9cc4ef`, the red `#ff9a94`, and the disc inside a ring
(`#1a1850`), which the canvas keeps literal in both. Frame 14e ("Scambia, scuro",
a night-blue ground) is superseded: 15 replaces it and 16 says the dark "will
follow the palette of the other screens".

Inter and Plus Jakarta are bundled from `@fontsource-variable`. "in linea" rather
than the canvas's "collegata/collegato": the app does not know anybody's gender,
and the neutral word is the one deviation.

## Onboarding

The multi-device design (SPAKE2 pairing, one shared identity, synced address
book) is the phone's front door: **scan a QR on your desktop Arvolo and the
phone becomes your identity's device** — contacts, blocklist and all. A
phone-only user mints a fresh identity and points at a relay the same way the
desktop app does. Identity key lands in Keychain / Android Keystore-backed
storage.

## Parity with the desktop (decided 2026-08-27)

The phone is not a reduced Arvolo. **Everything the desktop app can do, the
phone can do**; only the way you reach it changes, and only where the platform
leaves no choice does behaviour change with it. Read the list below as the
acceptance criteria for v1, one line per desktop place:

| Desktop | Phone |
| --- | --- |
| Transfers — active, queued, stalled, pause/resume, cancel, retry | Attività, one list in time order; pause/resume and cancel as swipe actions, "Pausa a tutto" on the heading |
| History — everything past, searchable | *Cronologia*, the other half of Attività; search from the magnifier beside the title |
| People — add, pair, verify fingerprint, trust, block, rename, forget, per-contact relay | People tab, verbs under the row menu; fingerprint compared out loud as on the desktop |
| Your devices — pair, rename, forget, send to a device | under *You*, same actions; a paired phone is a send target like any other |
| Links and deposits — create, list, extend, revoke | under *You*, full sheet; creating a link is a send option |
| Settings — name, fingerprint, public id, theme, language, relay, download folder, seeding, identity | under *You*, plus the phone-only switches (push, auto-save, Wi-Fi only, foreground service). The download folder is the one row that is **read-only** — see *Where received files land* |
| Send — files, folders, several recipients, a code, a public link | the Send tab and the platform share sheet, which replaces drag-and-drop |
| Receive — paste a code, scan a QR, open an `arvolo send --link` URL | *Scambia*: the camera is the screen, the typed code is in the sheet under it, and URL handling is in-app |
| Incoming offers — accept, decline, choose where to save | the incoming sheet, reached from the notification or the Casella banner |
| Swarm seeding | **on, gated**: Wi-Fi and charging by default, and on iOS only while the app is in the foreground |

What the platform, not the design, takes away:

- **iOS cannot listen in the background.** No push, no wake; the fallbacks are
  `BGAppRefreshTask` and opening the app. Android buys the difference back with
  the opt-in foreground service.
- **Live P2P and hosted codes need the app in the foreground** on iOS, and on
  Android whenever the foreground service is off.
- **Keyboard shortcuts and `⌘K` have no analogue.** The command palette
  becomes the search field.
- **Auto-update is the store's job** in v1, not the in-app minisign updater.
- **Nothing reveals a file in Finder.** `~/.arvolo` is not browsable on a
  phone: the identity lives in the Keychain / Android Keystore, and the config
  is edited in the app or nowhere.

## Where received files land (decided 2026-08-27)

A file you cannot open with another app has not really arrived. On the desktop
that is free — the download is written into a folder every program can read. On a
phone it has to be designed, and what forces the design is our own code, not the
platforms.

A receive here **writes in place**: the output file is pre-allocated with
`set_len`, every verified piece is written at its own offset, a `.arvticket`
sidecar sits beside the partial so an interrupted download can resume, and the
`name (1).ext` dance needs to list the directory. None of that is possible inside
Android's MediaStore or a SAF tree — there is no path, no neighbours to list, and
no file at all until the handle closes.

So the phone **downloads into the app's own storage and publishes when the file
is whole**. That is not a workaround so much as a better promise: nothing appears
in a person's Downloads until it is complete and verified.

- **Android**: published into the public `Download/Arvolo/` through MediaStore.
  Visible in Files, indexed, and it survives uninstalling the app. No storage
  permission is needed on API 29+, and neither `WRITE_EXTERNAL_STORAGE` nor
  `MANAGE_EXTERNAL_STORAGE` — the latter would be a fight with Play review. It
  costs one copy at the end, so accepting checks for **twice** the payload size
  through the same `available_space` the disk-full pause already uses.
- **iOS**: `Documents/` is the home, exposed in the Files app as "On My iPhone ›
  Arvolo" by two `Info.plist` keys — `UIFileSharingEnabled` and
  `LSSupportsOpeningDocumentsInPlace`. The second is the one that matters: other
  apps open and edit the file **where it is** instead of taking a copy.
  Publishing is a same-volume rename, so it is instant and costs no extra space.
- **The destination is fixed on both**, and the download-folder setting is a
  read-only row with an "open the folder" action. There is no per-transfer "save
  where?" before accepting, because iOS cannot offer one and two behaviours are
  worse than one honest one. What replaces it is per-file: **Open**, **Share**,
  **Save to…** (SAF on Android, the document picker on iOS), **Save to Photos**
  (on request only — Arvolo never puts things in the camera roll by itself, and
  the permission is asked the first time you use it) and **Show in folder**.

The engine's only part in this is one seam, `manager::SavePublisher`
([core/src/manager/publish.rs](../core/src/manager/publish.rs)): the embedder is
handed the finished file and answers with where it now lives — a path, or an
opaque `content://` URI for the platforms that have no path to give. A manager
without a publisher is every desktop, unchanged byte for byte. Three consequences
worth writing down:

- A failed publish **does not fail the transfer**. The file stays in staging and
  the row says so; a download that took an hour is not thrown away because a
  volume was full.
- **A row with a URI has no path**, so a client opens `saved_uri` instead. Seeding
  such a file back to the swarm means reading it through a descriptor the platform
  opens — which the engine already supports as
  [`SendSource::Handle`](../core/src/source.rs), built for macOS's file consent
  and reusable here unchanged. Until a mobile shell wires it up, a published-by-URI
  row does not seed.
- **Forgetting a row does not delete the file.** Once published it belongs to the
  person, not to Arvolo.

## Distribution

**v1 ships through the App Store and Play**, which settles push as APNs + FCM.
The no-vendor road — a direct APK and F-Droid, UnifiedPush instead of FCM, and
the minisign in-app updater the desktop already has — is a later version, kept
open by not building anything that assumes the stores.

## The decisions

Made 2026-08-17 by the project owner:

1. **Stack: Tauri 2 mobile** — core in-process behind the GUI's command
   surface, React frontend with a phone layout, native edges as contained
   targets.
2. **Push: vendor gateway only in v1** (APNs + FCM through `arvolo-push`,
   content-free wakes). UnifiedPush later.
3. **Scope: the wider v1** — codes hosted from the phone and deposits/links
   management included.
4. ~~**Android foreground service: not in v1.**~~ Superseded by 5.

Made 2026-08-27, widening the scope to full parity:

5. **Android foreground service: in v1, opt-in** — off until the user turns it
   on, and then the desktop daemon's behaviour on Android.
6. **Swarm seeding from the phone: in v1, gated** — Wi-Fi and charging by
   default; foreground-only on iOS.
7. **Distribution: stores first.** APK / F-Droid, UnifiedPush and the in-app
   updater come in a later version, and nothing in v1 may make them harder.
8. **Tablets: responsive, not redesigned** — the rail comes back past a
   breakpoint; same features, same codebase.
9. **The design is a starting point.** "Arvolo Mobile 1c — Casella" sets the
   direction; parity decides the details.
10. **Received files: staged, then published** to `Download/Arvolo/` on Android
    and to the Files-visible `Documents/` on iOS; fixed destination, per-file
    Open / Share / Save to… / Save to Photos. See *Where received files land*.

## Build order

1. **Engine as a library** (pure Rust, no mobile yet): the same command surface
   `bridge.rs` speaks over the socket, callable without a daemon. Desktop keeps
   the daemon; both frontends share one seam. Testable entirely on desktop.

   This is the crate **`arvolo-engine`**, and it is where the line falls: what a
   request *means* is `engine::daemon::dispatch`, and how it arrived is the
   CLI's socket. With it went the address book, the config, deposits, sessions,
   handles, history, pairing, the sync round, the identity helpers — everything
   the daemon does on somebody's behalf. What stayed in `arvolo-cli` is argument
   parsing, progress bars, prompts, and the accept loop.

   Starting it is `engine::start::start(Options)`: identity, transfer manager,
   inbox subscription, the trust policy on an incoming offer, history, the
   address-book watcher — and back comes a `Daemon` ready to answer. The CLI
   wraps it in a socket; a phone holds it. What the embedder still owns is how
   it is reached, when it stops, and where the engine's announcements go
   (`announce::Announce` — the terminal prints and raises a desktop
   notification, a phone will ask the system to).

   Listening to it is `Daemon::subscribe()`: one stream of both event sources,
   each already stamped with what the address book knows, and holding it is what
   "a front-end is attached" means. The CLI writes those to a socket in four
   lines; a phone renders them.

   Also done here: the `SavePublisher` hook and the `saved_uri` that rides
   beside a row's path.

   **Phase 1 is done.** What a shell has to bring is a `Announce`, optionally a
   `SavePublisher`, and a way to be reached.
2. **Walking skeleton on Android** (fastest iteration loop): Tauri shell +
   embedded engine + the React frontend in a phone layout; identity pairing
   and foreground receive/send against a real relay.

   The half of this that is not Android is done: `DaemonClient` now carries
   either a socket or an engine in this process, `bridge.rs` asks it either way,
   and `bridge::use_embedded` is where a shell hands one over. Everything the
   window can ask a daemon it can ask an engine, and the tests prove it without
   a socket in sight.

   **`arvolo-core` and `arvolo-engine` compile for `aarch64-linux-android`** —
   iroh, quinn, rustls, hpke, argon2, spake2 and all. That was the open question
   under this whole plan and the answer is yes. The recipe, with the SDK where
   Android Studio puts it:

   ```sh
   rustup target add aarch64-linux-android x86_64-linux-android
   export ANDROID_HOME="$HOME/Library/Android/sdk"
   export NDK_HOME="$ANDROID_HOME/ndk/27.1.12297006"
   TC="$NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin"
   export PATH="$TC:$PATH"
   export CC_aarch64_linux_android="$TC/aarch64-linux-android24-clang"
   export AR_aarch64_linux_android="$TC/llvm-ar"
   export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TC/aarch64-linux-android24-clang"
   cargo check -p arvolo-engine --target aarch64-linux-android
   ```

   The first pass takes about 40 minutes; after that it is cached. `npm run
   tauri android init` inside `gui/` generates `src-tauri/gen/android`, which is
   gitignored and regenerated per clone.

   `gui/src-tauri` is now a library as well as a binary (`arvolo_gui_lib`,
   `cdylib`) because a phone has no `main` to call — the platform's activity
   loads the `.so` and enters at `run`, which is the same function the desktop's
   four-line `main` calls.

   **The backend compiles for Android**, and starts the engine inside itself:
   tray, autostart and the updater are behind `cfg(desktop)`, and
   `start_embedded_engine` points the config dir at the app's private storage,
   brings the engine up there and hands it to `use_embedded`. Downloads land in
   a staging folder inside that storage, which is where they have to land — see
   *Where received files land*; publishing them out of it is the next thing that
   file needs.

   **It runs.** On a Pixel 8 emulator (API 33, arm64) the app starts, the engine
   comes up inside it — `arvolo: engine up` in logcat — and it writes what a
   fresh installation writes: an identity, an address book, a device register,
   an `incoming/` folder. The window says connected, because it is: the bridge
   and the event pump both went through the engine and never looked for a
   socket.

   Three things had to be gated that only a running app could have found: the
   updater plugin (asking a Tauri state that was never managed is a *panic*, not
   an error), the event pump's daemon supervision, and `restart_daemon`.

   One packaging note. A debug APK is **583 MB** — the unstripped debug info of
   the whole transfer stack — and an emulator's data partition is smaller than
   that. Neither cargo nor AGP strips it here, so:

   ```sh
   llvm-strip --strip-debug target/aarch64-linux-android/debug/libarvolo_gui_lib.so
   # then, from a dir holding lib/arm64-v8a/libarvolo_gui_lib.so:
   zip -d app.apk "META-INF/*" && zip -0 -X app.apk lib/arm64-v8a/libarvolo_gui_lib.so
   zipalign -p -f 4 app.apk aligned.apk   # -0 and the alignment both matter:
   apksigner sign --ks ~/.android/debug.keystore …   # a compressed .so is refused
   ```

   67 MB, installs, runs. A release build would not need any of this.

   **And it receives.** A file sent from the Mac's CLI to the phone's public id
   with `--mailbox` went the whole way: sealed and deposited on the public
   relay, the phone's in-app engine long-polling its inbox, the offer decrypted
   and parked, the window drawing it as a question with the sender's fingerprint
   and "not in the address book", accept, fetch over HTTPS, decrypt on the
   phone, 47 bytes identical to what was sent, a history record written. The
   mailbox-first architecture this document proposed, running.

   Two things that test made plain. The accept dialog's "where to save it" reads
   `/data/user/0/it.termox.arvolo/incoming` — the staging folder, exactly as
   designed and exactly as useless to a person until something publishes out of
   it. And the whole screen is still the desktop's.

   **The frame is a phone's now.** Below 560px (`PHONE_QUERY` in `gui/src/phone.ts`,
   and the same number in `theme.css`) the rail is replaced by a bottom bar:
   Attività · Persone · **Scambia** · Invia · Impostazioni, with Scambia filled
   and in the middle where a thumb reaches. The places that did not fit — links
   and deposits, your devices — are a glyph on Attività and rows in
   Impostazioni, so a phone still reaches everything a desktop does. The header and the bar keep clear of the
   status bar and the gesture bar, and the first screen no longer asks anyone to
   drag a file onto a telephone.

   What is left of the layout is everything inside the frame: sheets that go
   full-screen and dismiss with a swipe, row actions that become swipes and
   long-presses, 44pt targets beyond the bar, pull-to-refresh. And then the
   larger move the design of record proposes — Transfers and History folding
   into one **Casella** of correspondents rather than two lists of files.
3. **iOS shell** + Keychain identity + the same flows.

   **The whole stack compiles for `aarch64-apple-ios` untouched** — core,
   engine, iroh, the crypto — and `tauri ios build --debug` produces a signed
   `.ipa` (25 MB) that installs on a device with the CryCatch team's Apple
   Development certificate. `start_embedded_engine` needed nothing
   iOS-specific: it is `cfg(mobile)` and resolves its home through
   `app_data_dir()`, which lands in `Library/Application Support/it.termox.arvolo`.

   Two things had to be added, and both live where a regeneration cannot eat
   them: the iOS Rust targets (`rustup target add aarch64-apple-ios
   aarch64-apple-ios-sim`), and `src-tauri/Info.ios.plist`, which is merged
   into the generated plist and carries `NSCameraUsageDescription` (the QR
   scanner) plus `UIFileSharingEnabled` and `LSSupportsOpeningDocumentsInPlace`
   — the two keys that will make the app a folder in Files once there is a
   publisher to move finished files into it.

   **The build recipe, because this checkout is on a path with a space in it**
   (`/Volumes/HD Esterno/…`, and Xcode itself is installed there too). Nothing
   in this toolchain quotes paths consistently, and the failures are three
   layers deep:

   ```sh
   APPLE_DEVELOPMENT_TEAM=D6FKBG7DZF npx tauri ios init   # generates gen/apple/
   # then, in gen/apple/arvolo-gui.xcodeproj/project.pbxproj, the "Build Rust
   # Code" script phase needs its sdk-root replaced by a space-free one:
   #   SDK_LINK="$HOME/.arvolo-ios-sdk"; ln -sfn "${SDKROOT:?}" "$SDK_LINK"
   #   … --sdk-root "$SDK_LINK" …
   npx tauri ios build --debug     # → gen/apple/build/arm64/Arvolo.ipa
   xcrun devicectl device install app --device "<name>" Arvolo.ipa
   ```

   Why a symlink rather than quoting. Quoting `${SDKROOT:?}` in the script
   phase is necessary but not sufficient: it only fixes the argument this
   project passes. `tauri ios xcode-script` then *builds its own flag string*
   — `-isysroot <sdk>` — and hands it to cargo through the environment, and
   cc-rs splits those strings on whitespace with no regard for quotes. So
   clang received `-isysroot /Volumes/HD` and a stray second argument.
   Shadowing the variable does not work either (the tool's value is added
   anyway, alongside). The only lever this side of the tool is the path it is
   given, and `$HOME` has no spaces in it. `cargo check --target
   aarch64-apple-ios` passing while the Xcode build failed is the tell: it is
   the environment, not the code.

   `gen/apple/` is gitignored, so **`tauri ios init` undoes the script-phase
   patch** and it has to be reapplied.
4. **Push**: `arvolo-push` gateway, relay wake forwarding, FCM + APNs, the
   iOS Notification Service Extension, background fetch fallback.
5. **Share sheets** both platforms; codes-from-phone; deposits/links views.
6. **The parity sweep**: People, Your devices, History and Settings in full —
   every verb the desktop rail offers, reachable on the phone. This is the
   phase that makes the table above true, and the one to measure against it.
7. **Android foreground service** (opt-in) and the **seeding gates** (Wi-Fi,
   charging, foreground-only on iOS).
8. **The wide layout**: the tab bar becomes a rail past the breakpoint.
9. **Store passage**: signing, review, TestFlight / Play internal track.
