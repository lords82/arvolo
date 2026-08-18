# Arvolo on phones — design

> Status: **design decided** (2026-08-17), implementation not started. The
> four calls at the bottom were made by the project owner; the rest of this
> document is the reasoning they rest on.

## The one constraint that shapes everything

On a phone the app cannot stay listening. iOS suspends it seconds after it
leaves the screen; Android is gentler but headed the same way. So the desktop
model — a daemon holding a live inbox subscription — does not port. What does
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
- **v1 ships the essential path only**: the vendor gateway, APNs + FCM. Two
  no-vendor roads stay on the map for later versions, deliberately not in v1:
  UnifiedPush (self-hosted distributor instead of FCM) and an Android
  **foreground service** ("always listening", Syncthing-style, opt-in).
- **iOS fallback** when push is declined: periodic `BGAppRefreshTask` inbox
  polls, and a full check on every app open. Slower, never broken.

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

- **Navigation**: the rail becomes a bottom **tab bar** (Transfers, People,
  Send as the center action, History, Settings); "Links and deposits" and
  "Your devices" move under Settings — six rail places don't fit five tabs.
- **The two verbs get primary**: Send is the tab bar's center button; Receive
  (paste a code / scan a QR) lives one tap away on the Transfers screen.
  Drag-and-drop disappears; the platform **share sheet** and file/photo
  pickers replace it.
- **Sheets go full-screen** and gain swipe-to-dismiss; `⌘K` and every
  keyboard shortcut disappear (a search field stays); hover states become
  press states; row actions move into swipe actions and long-press menus.
- **Touch and platform**: 44 pt minimum targets, safe-area insets (notch,
  home indicator), on-screen keyboard pushing layouts, pull-to-refresh
  instead of a refresh button.
- **What carries over untouched**: the store, the IPC command surface, i18n
  (all four languages), the design tokens/theme, and the component library —
  restyled where touch demands it. The claude.ai/design project holding the
  26 synced components is where the phone variants get designed first.

## Onboarding

The multi-device design (SPAKE2 pairing, one shared identity, synced address
book) is the phone's front door: **scan a QR on your desktop Arvolo and the
phone becomes your identity's device** — contacts, blocklist and all. A
phone-only user mints a fresh identity and points at a relay the same way the
desktop app does. Identity key lands in Keychain / Android Keystore-backed
storage.

## v1 scope (decided)

In: join-identity onboarding (+ fresh identity), receive with push + wake
fetch + notification, browse/save/share received files, send via share sheet
or in-app to a contact (live if both foreground, else deposit), **hosting
codes from the phone while the app is in the foreground** (backgrounding
pauses, reopening resumes), **deposits and links management** (create, list,
revoke), scan/show pairing and transfer QR codes, open `arvolo send --link` URLs
in-app.

Out, deliberately: swarm seeding from the phone, UnifiedPush, the Android
always-listening foreground service (all later versions),
tablets-as-first-class.

## The decisions

Made 2026-08-17 by the project owner:

1. **Stack: Tauri 2 mobile** — core in-process behind the GUI's command
   surface, React frontend with a phone layout, native edges as contained
   targets.
2. **Push: vendor gateway only in v1** (APNs + FCM through `arvolo-push`,
   content-free wakes). UnifiedPush later.
3. **Scope: the wider v1** — codes hosted from the phone and deposits/links
   management included.
4. **Android foreground service: not in v1.**

## Build order

1. **Engine as a library** (pure Rust, no mobile yet): give `arvolo-core` the
   in-process face the mobile shell needs — the same command surface
   `bridge.rs` speaks over the socket, callable without a daemon. Desktop
   keeps the daemon; both frontends share one seam. Testable entirely on
   desktop.
2. **Walking skeleton on Android** (fastest iteration loop): Tauri shell +
   embedded engine + the React frontend in a phone layout; identity pairing
   and foreground receive/send against a real relay.
3. **iOS shell** + Keychain identity + the same flows.
4. **Push**: `arvolo-push` gateway, relay wake forwarding, FCM + APNs, the
   iOS Notification Service Extension, background fetch fallback.
5. **Share sheets** both platforms; codes-from-phone; deposits/links views.
6. **Store passage**: signing, review, TestFlight / Play internal track.
