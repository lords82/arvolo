# Arvolo on phones — design

> Status: **design under discussion** (2026-08-17). Nothing here is built.

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
- **Android keeps a no-vendor path**: UnifiedPush (self-hosted distributor)
  as a first-class alternative to FCM — and optionally a persistent
  **foreground service** ("always listening", Syncthing-style, off by
  default) for true live receive with no push at all.
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
  design system carry over; the rail becomes a tab bar, sheets go full-screen.
  Same codebase, responsive — the claude.ai/design component set already
  covers the pieces.
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

## Onboarding

The multi-device design (SPAKE2 pairing, one shared identity, synced address
book) is the phone's front door: **scan a QR on your desktop Arvolo and the
phone becomes your identity's device** — contacts, blocklist and all. A
phone-only user mints a fresh identity and points at a relay the same way the
desktop app does. Identity key lands in Keychain / Android Keystore-backed
storage.

## v1 scope (proposed)

In: join-identity onboarding (+ fresh identity), receive with push + wake
fetch + notification, browse/save/share received files, send via share sheet
to a contact (live if both foreground, else deposit), scan/show pairing and
transfer QR codes, open `arvolo link` URLs in-app.

Out, deliberately: hosting long-lived codes/shares from the phone, swarm
seeding, deposits/links management (v1 points to the desktop for those),
tablets-as-first-class.

## Open questions

Held in the PR discussion: final stack sign-off; the vendor-gateway trade-off;
v1 scope cut; whether Android ships the "always listening" foreground-service
option at v1.
