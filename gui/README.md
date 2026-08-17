# Arvolo — Desktop GUI (Tauri 2)

A cross-platform desktop app for Arvolo that runs **alongside** the CLI. It does
**not** contain a second transfer engine: it is a thin client of the same
background **daemon** the CLI drives, over the local IPC socket. Closing the
window leaves transfers running; the CLI and GUI share one engine and identity.

The app is **six places and two verbs**: a rail carrying Invia / Ricevi above
Trasferimenti, Persone, Link e depositi, Cronologia, I tuoi dispositivi and
Impostazioni. `⌘K` reaches all of them, plus every contact. It covers the CLI's
surface — see *Parity* below — and does so with a real light and dark theme.

Two ideas run through the design and are worth knowing before reading the CSS:

* **Direction is a colour.** Outgoing is amber, incoming is blue, on every
  progress bar, row stripe and section header. A glance tells you which way the
  bytes are going before you read a word. Semantic colours (green/red/amber) are
  reserved for *outcomes*, so the two vocabularies never collide.
* **Trust is typography.** Fingerprints, public ids and pairing codes are set in
  mono, spaced, selectable, and never truncated where a decision depends on
  them. A pairing code gets the largest type in the app, because somebody is
  about to read it out loud.

## Architecture

```
React UI (Zustand store)  ──invoke()──▶  #[tauri::command] bridge ──▶ DaemonClient ─┐
        ▲                                                                            │  unix socket
        └──listen("engine://event")◀── event pump ◀──subscribe()──── arvolo daemon ◀┘  (one engine)
                                                                          ▲
                                                              CLI also ───┘
```

* **`src-tauri/`** — Rust backend.
  * `bridge.rs` — one `#[tauri::command]` per UI action, forwarding to the daemon
    via `arvolo_ipc::client::DaemonClient` (a fresh short-lived RPC dial per call).
  * `daemon.rs` — `ensure_running()`: connects if a daemon is up, else spawns
    `arvolo daemon` **detached** (its own process group) so it outlives the GUI.
  * `main.rs` — the **event pump**: holds one `subscribe()` stream and re-emits
    every engine event as `engine://event`, plus an `engine://connected`
    heartbeat and a native notification on each incoming offer.
* **`src/`** — React + TypeScript frontend.
  * `store.ts` — a single Zustand store, seeded from a daemon snapshot
    (`list`/`list_pending`/`list_contacts`) then mutated **only** by pushed
    events. No polling. The screens with no push event behind them — deposits,
    history, settings, devices — refetch on arrival instead, because a stale
    panel must never be what greets the user.
  * `theme.css` — the design system: tokens, base, components. Both themes are
    defined in full; `data-theme` on `<html>` overrides the system preference in
    either direction.
  * `ui/` — primitives (`Button`, `Field`, `Switch`, `Segmented`, `Sheet`,
    `Menu`, `Toasts`) and the app's own pieces (`Avatar`, `CodeHero`,
    `Fingerprint`, `Progress`) in `Bits.tsx`.
  * `views/` — one file per place, plus `Rail`.
  * `overlays/` — `SendSheet`, `ReceiveSheet`, `IncomingDialog`, `PairSheet`,
    `CommandPalette`.
  * `App.tsx` — the frame: banners, header, the current view, and the three
    things that belong to the *window* rather than any screen (drag-and-drop,
    the keyboard map, the daemon-health banners).

The wire contract + client live in the shared **`arvolo-ipc`** crate (also used by
the CLI), so both frontends speak the same protocol.

## Prerequisites

* Rust (workspace toolchain) and the platform WebView:
  * **macOS** — WebKit (system).
  * **Linux** — `webkit2gtk-4.1`, `libayatana-appindicator3`, `librsvg2` dev pkgs.
* Node ≥ 18 and npm.

## Run (development)

```sh
cd gui
npm install
# ARVOLO_BIN lets the GUI find the daemon binary during dev:
ARVOLO_BIN="$(cargo metadata --format-version 1 -q | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')/debug/arvolo" \
  npm run tauri dev
```

Build the CLI first (`cargo build -p arvolo-cli`) so `arvolo daemon` exists. If a
daemon is already running (e.g. you started `arvolo daemon` yourself, or via
systemd/launchd), the GUI just attaches to it and `ARVOLO_BIN` is unnecessary.

`ARVOLO_CONFIG_DIR` / `ARVOLO_RELAY` are honored exactly as by the CLI — point
them at a test config/relay to sandbox a dev instance.

## Build (release bundle)

```sh
cd gui
npm install
npm run tauri build   # -> src-tauri/target/release/bundle/
```

Bundle the `arvolo` binary as a Tauri sidecar (or install it on `PATH`) so the
app can spawn the daemon on a clean machine.

## Platform status

* **macOS / Linux** — supported (the daemon IPC uses a unix socket).
* **Windows** — supported: `arvolo-ipc` speaks the same protocol over a named
  pipe, and the release ships an `.msi` (unsigned for now, so SmartScreen asks
  for a "Run anyway").

## Extra behaviours

* **Tray**: closing the window hides it to the system tray ("Mostra Arvolo" /
  "Esci" in the tray menu); the daemon keeps running either way. With the window
  closed Arvolo leaves the Dock (macOS) and the taskbar (Windows, Linux) and
  lives in the status area only — clicking the tray icon brings it back, as does
  the Dock icon or a re-launch on macOS. Where no tray can be created (a Linux
  desktop without a StatusNotifier host) closing quits instead, so the app never
  disappears with no way back.
* **Arrivi in attesa** are announced by a banner on every screen but the board,
  and by a count on the rail; a native notification fires even when the window
  is closed to the tray.
* **Scorciatoie**: `⌘K` command palette, `⌘N` invia, `⌘V` ricevi, `⌘,`
  impostazioni, `/` cerca. All of them stand down while focus is in a text
  field, so `⌘N` mid-note does not throw the note away.
* **Tema**: chiaro, scuro o di sistema (Impostazioni → Aspetto, or the palette).
  The choice is stored locally and applied before the first paint.
* **Verifica identità** (Rubrica and the incoming modal) marks a *saved contact*
  verified **with the fingerprint on screen** — the click confirms an
  out-of-band comparison, it never marks blind (`MarkVerified` IPC).
* Per-row **Elimina** removes a finished transfer from the daemon's list
  (`Remove` IPC); **Pulisci** sweeps every finished row (`ClearFinished`);
  **Sposta su/giù** is local ordering only.
* A live **pairing code** shows as a copyable chip on its board row
  (`code_ready` / `code_closed` events).
* A **version banner** appears when the running daemon's version differs from the
  GUI's, with a one-click **Riavvia il daemon** (SIGTERM via pid file; the event
  pump respawns it).
* No right-click context menu; in release builds the reload/devtools shortcuts
  are swallowed too.

## Parity with the CLI

Every verb has a home:

| CLI | Where |
|---|---|
| `send --to` / `--deposit --ttl --max --password` | Invia → *A un contatto*, with *Lascia in casella* |
| `code` (incl. multi-recipient) | Invia → *Codice* |
| `link --ttl --max` | Invia → *Link* |
| `ticket` | Invia → *Ticket* |
| `recv` (code / `arvc…` / `arvm…` + password) | Ricevi |
| `status`, `pause`, `resume`, `cancel`, `status clear` | Trasferimenti |
| `contacts` add/list/rename/remove/verify/trust/block/prune | Persone |
| `contacts pair` | Persone → *Scambia contatti* |
| `contacts export` / `import` | Persone → *Esporta* / *Importa* |
| `contacts list` presence probe | Persone → *Chi c'è* |
| `history`, `history clear` | Cronologia |
| `device pair` / `join` / `sync` / `status` | I tuoi dispositivi |
| `me`, `me name`, relay + download dir config | Impostazioni |

Pairing runs as a **session**, not a request/reply: the daemon answers with a
handle and the code and outcome arrive as `pairing_*` events, because the
exchange waits on a human at another machine. Closing the sheet cancels it —
an unattended `device pair` would otherwise keep offering this device's
identity secret for its whole window.

### Still deferred

* **Scansione QR** con fotocamera — the app *shows* QR codes (pairing codes,
  tickets, links); scanning one is deferred.
* **Resume of an interrupted send** by session id or re-supplied `arvc…` ticket.
  The board resumes *paused* transfers; disk-session recovery stays CLI-only.
* **Riprova** on a failed transfer and **Forza ripresa** on a stalled one are not
  offered: the engine has no retry for terminal failures (stalled sends already
  auto-retry), so the menu only shows actions that actually work.
* **Link** publishes a single item — pick a folder to send a group. No password:
  the CLI refuses it there too, because the browser page cannot unwrap one.
  Password-protected *deposits* are both created and received fine.
* `completions` has no GUI equivalent, and does not want one.
