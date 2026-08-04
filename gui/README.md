# Arvolo — Desktop GUI (Tauri 2)

A cross-platform desktop app for Arvolo that runs **alongside** the CLI. It does
**not** contain a second transfer engine: it is a thin client of the same
background **daemon** the CLI drives, over the local IPC socket. Closing the
window leaves transfers running; the CLI and GUI share one engine and identity.

Implements the **board `3a`** design from `../Arvolo.dc.html`: two columns
(Inviati / Ricevuti) with sections, live search, pause-all + clear-finished, a
5-tab send panel (Persone / ID·QR / Codice / Link / Ticket), a paste-anything
**Ricevi** modal (`arvolo recv`), a full **Rubrica** (add/rename/remove,
verify-with-fingerprint, trust, block, advertised-name approval, own display
name), a **Storico** panel (`arvolo history`) and an incoming-offer modal with
fingerprint verification and block-sender.

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
  * `store.ts` — a single Zustand `TransferStore`, seeded from a daemon snapshot
    (`list`/`list_pending`/`list_contacts`) then mutated **only** by pushed
    events. No polling.
  * `components/` — `Board` (+ `Column`, `TransferRow`, `RowMenu`), `SendSheet`,
    `IncomingModal`; `App.tsx` is the shell (header, drop zone, control row).

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
* **Windows** — deferred: needs a named-pipe transport behind the same protocol
  in `arvolo-ipc` (the app opens but shows "Disconnesso" until then).

## Extra behaviours

* **Tray**: closing the window hides it to the system tray ("Mostra Arvolo" /
  "Esci" in the tray menu); the daemon keeps running either way.
* **Campanella arrivi** in the header: red badge = offers awaiting a decision;
  click opens the oldest one.
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

## Known gaps vs. the CLI (deferred)

* **`arvolo contacts pair`** (SPAKE2 trade of public ids) and **`arvolo device
  pair/join/sync`** (multi-device identity) — interactive rendezvous flows, a
  separate epic.
* **`arvolo contacts export/import`** — file-based book backup stays CLI-only.
* **Resume of an interrupted send** by session id / re-supplied `arvc…` ticket —
  the board resumes *paused* transfers; disk-session recovery stays CLI-only.
* **Scansione QR** con fotocamera — the app *shows* QR codes (own code, pairing
  code, ticket); scanning is deferred.
* Chip **metodo** (P2P vs Cloud) is best-effort: `TransferDto` carries no method
  flag, so it is inferred.
* **Riprova** on a failed transfer and **Forza ripresa** on a stalled one are not
  offered: the engine has no retry for terminal failures (stalled sends already
  auto-retry), so the menu only shows actions that actually work.
* **Link** invio: supports a single file (a folder is archived); multiple selected
  files fall back to the first — use Ticket/Persone to send a group. `--password`
  is not offered because the CLI doesn't support it on links either (the browser
  page can't unwrap it); password-protected **deposits** are received fine (the
  Ricevi modal asks when the ticket carries one).
