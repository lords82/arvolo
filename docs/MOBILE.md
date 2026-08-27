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

- **Navigation**: the rail becomes a bottom **tab bar** — *Casella*, *People*,
  **Send** as the center action, *Codes*, *You*. Six rail places don't fit
  five tabs, so they fold rather than drop: Transfers and History merge into
  **Casella**, one row per correspondent showing the latest exchange and
  opening a thread with everything before it; "Links and deposits" and "Your
  devices" move under *You*. Nothing is lost — see *Parity with the desktop*
  below.
- **The two verbs get primary**: Send is the tab bar's center button; Receive
  (paste a code / scan a QR) is the *Codes* tab, one tap from anywhere.
  Drag-and-drop disappears; the platform **share sheet** and file/photo
  pickers replace it.
- **Sheets go full-screen** and gain swipe-to-dismiss; `⌘K` and every
  keyboard shortcut disappear (a search field stays); hover states become
  press states; row actions move into swipe actions and long-press menus.
- **Touch and platform**: 44 pt minimum targets, safe-area insets (notch,
  home indicator), on-screen keyboard pushing layouts, pull-to-refresh
  instead of a refresh button.
- **Tablets are the same app, wider**: past a breakpoint the tab bar goes back
  to being a rail and the sheets become two columns. Same features, one
  codebase, no separate tablet design.
- **What carries over untouched**: the store, the IPC command surface, i18n
  (all four languages), the design tokens/theme, and the component library —
  restyled where touch demands it. The claude.ai/design project holding the
  26 synced components is where the phone variants get designed first.

**The design of record** is `gui/.design-sync/mobile/casella.dc.html` — a copy
of the "Arvolo Mobile 1c — Casella" canvas in the *Proposte app mobile Arvolo*
project on claude.ai/design. It shows the five tabs, the Casella list, the
thread, the send sheet, the incoming sheet, deposits and links, search, and the
pairing onboarding, all built from the real design system bundle. It is a
**starting point, not a specification**: where parity and the mockup disagree,
parity wins and the screen gets redrawn. The earlier three-layout exploration
(A/B/C) is superseded and was deleted.

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
| Transfers — active, queued, stalled, pause/resume, cancel, retry | Casella rows and the thread sheet; pause/resume and cancel as swipe actions |
| History — everything past, searchable | the same threads, scrolled back; full-text search from the Casella app bar |
| People — add, pair, verify fingerprint, trust, block, rename, forget, per-contact relay | People tab, verbs under the row menu; fingerprint compared out loud as on the desktop |
| Your devices — pair, rename, forget, send to a device | under *You*, same actions; a paired phone is a send target like any other |
| Links and deposits — create, list, extend, revoke | under *You*, full sheet; creating a link is a send option |
| Settings — name, fingerprint, public id, theme, language, relay, download folder, seeding, identity | under *You*, plus the phone-only switches (push, auto-save, Wi-Fi only, foreground service). The download folder is the one row that is **read-only** — see *Where received files land* |
| Send — files, folders, several recipients, a code, a public link | the Send tab and the platform share sheet, which replaces drag-and-drop |
| Receive — paste a code, scan a QR, open an `arvolo send --link` URL | the Codes tab (show/scan) and URL handling in-app |
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

   What is left is the layout: the window is still drawn for a 980×670 pointer —
   a rail down the left, a drop zone asking to have files dragged onto it — and
   the design of record is a phone.
3. **iOS shell** + Keychain identity + the same flows.
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
