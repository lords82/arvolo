// Hide the extra console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Arvolo desktop GUI backend.
//!
//! The GUI never runs the transfer engine itself: it drives the same background
//! **daemon** the CLI uses, over the local IPC socket. This process is a thin
//! bridge —
//! * `bridge` exposes `#[tauri::command]`s that forward UI actions to the daemon,
//! * the **event pump** (below) keeps one subscription open and re-emits every
//!   engine event to the webview as `engine://event`, plus a `engine://connected`
//!   heartbeat and a native notification for incoming offers.

mod attention;
mod bridge;
mod daemon;
#[cfg(target_os = "macos")]
mod notify_mac;

use arvolo_ipc::client::DaemonClient;
use arvolo_ipc::protocol::EventDto;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;

/// Frontend event channels.
const EV_ENGINE: &str = "engine://event";
const EV_CONNECTED: &str = "engine://connected";
/// Why the daemon would not start, when it would not start.
const EV_DAEMON_ERROR: &str = "engine://daemon-error";
/// Files just dropped on the window, already registered: `[PickedItemDto]`.
const EV_FILES_PICKED: &str = "files://picked";

/// Whether the system tray icon actually got created. Hiding the window on close
/// is only safe if there is a tray to get back from: on Linux desktops without a
/// StatusNotifier host (GNOME without extensions) there is none, and closing has
/// to quit instead of leaving an unreachable process behind.
struct HasTray(bool);

fn main() {
    // `--autostart` is what the login entry passes (see the autostart plugin
    // registration below): the app was started by the OS, not by a person, so
    // it should come up hidden in the tray rather than with a window in the
    // face of someone who just signed in.
    let autostarted = std::env::args().any(|a| a == "--autostart");

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            bridge::status,
            bridge::list_transfers,
            bridge::list_pending,
            bridge::list_contacts,
            bridge::send_to,
            bridge::serve_ticket,
            bridge::serve_code,
            bridge::create_link,
            bridge::recv,
            bridge::accept_offer,
            bridge::reject_offer,
            bridge::pause,
            bridge::resume,
            bridge::cancel,
            bridge::remove,
            bridge::clear_finished,
            bridge::mark_verified,
            bridge::mark_unverified,
            bridge::mark_trusted,
            bridge::mark_untrusted,
            bridge::block_contact,
            bridge::unblock_contact,
            bridge::accept_name,
            bridge::add_contact,
            bridge::remove_contact,
            bridge::rename_contact,
            bridge::list_history,
            bridge::clear_history,
            bridge::set_my_name,
            bridge::restart_daemon,
            bridge::list_deposits,
            bridge::revoke_deposit,
            bridge::deposit_to,
            bridge::get_config,
            bridge::set_config,
            bridge::prune_names,
            bridge::presence,
            bridge::sync_status,
            bridge::sync_now,
            bridge::start_pairing,
            bridge::cancel_pairing,
            bridge::import_contacts,
            bridge::export_contacts,
            bridge::pick_files,
            bridge::gui_version,
        ])
        .setup(move |app| {
            let has_tray = match setup_tray(app.handle()) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("arvolo: icona di stato non disponibile ({e})");
                    false
                }
            };
            app.manage(HasTray(has_tray));
            // The window is declared hidden (`visible: false` in the config)
            // and shown from here, so there is nothing to hide and no flash: a
            // login launch with a tray to live in stays out of sight — the same
            // resting state as a window closed to the tray — and every other
            // launch (including a login launch on a desktop with no tray, which
            // would otherwise be unreachable) shows the window at once.
            if autostarted && has_tray {
                #[cfg(target_os = "macos")]
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            } else {
                show_main_window(app.handle());
            }
            app.manage(attention::State::default());
            app.manage(bridge::PickedFiles::new());
            // Ask now, not at the first arrival: a permission prompt that appears
            // the moment somebody sends you a file is in the way of the thing you
            // wanted to see. No-op when there is no bundle to ask for.
            #[cfg(target_os = "macos")]
            notify_mac::request_authorization();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(event_pump(handle));
            Ok(())
        })
        // Closing the window hides it away instead of quitting: transfers keep
        // running in the daemon and arrivals keep notifying, with Arvolo left in
        // the tray only — off the Dock on macOS, off the taskbar elsewhere.
        // "Esci" in the tray menu quits.
        .on_window_event(|window, event| {
            // A drop is handled HERE, on the window, and only the registered ids
            // reach the frontend. The webview still receives its own drag-drop
            // event with the raw paths — that cannot be turned off without losing
            // the hover states — but those strings are inert: no command accepts a
            // path any more, so knowing one buys nothing.
            if let WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                let app = window.app_handle();
                let items = bridge::register_paths(&app.state::<bridge::PickedFiles>(), paths);
                if !items.is_empty() {
                    let _ = app.emit(EV_FILES_PICKED, items);
                }
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                // No tray to restore from: let the close through, which quits.
                if !app.try_state::<HasTray>().is_some_and(|t| t.0) {
                    return;
                }
                api.prevent_close();
                let _ = window.hide();
                // A hidden window drops off the taskbar on Windows and Linux by
                // itself; the macOS Dock icon belongs to the process, so it has
                // to be taken down explicitly.
                #[cfg(target_os = "macos")]
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
        })
        .build(tauri::generate_context!())
        .expect("error while running the Arvolo GUI");

    app.run(|_app, _event| {
        // Clicking the Dock icon (or re-launching the bundle) while Arvolo is
        // already running asks for the window back.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = _event {
            show_main_window(_app);
        }
    });
}

/// System-tray icon with a minimal menu (Mostra / Esci) — the way back to a
/// window closed to the tray, and the only one once the Dock/taskbar entry is
/// gone.
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItemBuilder::with_id("show", "Mostra Arvolo").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Esci").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show)
        .separator()
        .item(&quit)
        .build()?;

    let mut tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        // macOS menu bar items open their menu on a plain click; on Windows a
        // left click restores the app and only the right click opens the menu.
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .tooltip("Arvolo")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if !cfg!(target_os = "macos") {
                    show_main_window(tray.app_handle());
                }
            }
        });

    // The menu bar wants the monochrome template icon, which macOS tints for the
    // light or dark bar; elsewhere the tray shows the regular app icon. Either way
    // this is the resting state; `attention::apply` drops the wedge into the box
    // once the pump knows something is waiting. `icons/attention/tray-0.png` is this
    // very file, so switching between them never resizes the mark.
    #[cfg(target_os = "macos")]
    {
        let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
        tray = tray.icon(icon).icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }

    tray.build(app)?;
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    // Put the Dock icon back first: while Arvolo is a background-only process
    // macOS won't bring its windows to the front.
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        restore_dock_icon(app);
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Hand AppKit Arvolo's icon again. Leaving `Accessory` builds the Dock tile from
/// scratch, and it is only the *bundle* that carries an icon: the bare binary
/// `tauri dev` runs has none and lands on the generic "exec" square. Setting it
/// ourselves makes the two look the same.
#[cfg(target_os = "macos")]
fn restore_dock_icon(app: &AppHandle) {
    /// The bundle icon, baked in — the same file `tauri.conf.json` ships.
    const ICON: &[u8] = include_bytes!("../icons/icon.icns");

    let _ = app.run_on_main_thread(|| {
        use objc2::{AllocAnyThread, MainThreadMarker};
        use objc2_app_kit::{NSApplication, NSImage};
        use objc2_foundation::NSData;

        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let data = NSData::with_bytes(ICON);
        if let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) {
            unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&icon)) };
        }
    });
}

/// Keep a live subscription to the daemon and forward every event to the webview.
/// Reconnects (spawning the daemon if needed) whenever the stream drops, so the
/// UI recovers on its own after a daemon restart.
async fn event_pump(app: AppHandle) {
    loop {
        // Make sure a daemon exists; if we can't bring one up, report disconnected
        // and retry shortly.
        if let Err(e) = daemon::ensure_running().await {
            // Say *why*, not just that. A daemon spawned from here has no terminal,
            // so its own explanation — an identity file it refuses to load, a relay
            // it cannot reach — would otherwise stay in a log file behind a banner
            // reading "disconnesso", which is the one thing the user can already
            // see. `ensure_running` quotes the tail of that log into this error.
            let _ = app.emit(EV_DAEMON_ERROR, format!("{e:#}"));
            let _ = app.emit(EV_CONNECTED, false);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        }

        let stream = match DaemonClient::connect().await {
            Ok(c) => c.subscribe().await,
            Err(e) => Err(e),
        };
        let mut stream = match stream {
            Ok(s) => s,
            Err(_) => {
                let _ = app.emit(EV_CONNECTED, false);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };

        let _ = app.emit(EV_CONNECTED, true);
        // A fresh connection says nothing about what was already pending — the
        // offers that piled up while the GUI was down are only in the daemon.
        attention::refresh(&app).await;

        // Loops while events arrive; exits on a closed (`Ok(None)`) or errored
        // stream, which drops us to the outer reconnect loop.
        while let Ok(Some(ev)) = stream.next().await {
            if let EventDto::OfferReceived {
                name,
                sender_name,
                auto,
                ..
            } = &ev
            {
                notify_arrival(&app, name, sender_name, *auto);
            }
            // The webview first: the tray state costs a round-trip to the daemon,
            // and the UI should not wait behind it.
            let _ = app.emit(EV_ENGINE, &ev);
            if moves_the_count(&ev) {
                attention::refresh(&app).await;
            }
        }

        let _ = app.emit(EV_CONNECTED, false);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// Whether an event can change how many decisions are waiting on the user.
///
/// `Progress` is the one that must stay out: it fires continuously for the whole
/// length of a transfer, and every match here costs a round-trip to the daemon.
/// `Completed` and `Cancelled` stay out too — a transfer ending decides nothing,
/// it only reports. `Started` is in because that is what an accepted offer turns
/// into; a *rejected* one emits nothing at all, so the reject command refreshes
/// the tray itself.
fn moves_the_count(ev: &EventDto) -> bool {
    matches!(
        ev,
        EventDto::OfferReceived { .. } | EventDto::ContactsChanged | EventDto::Started { .. }
    )
}

/// Fire a native notification for an incoming file — so arrivals surface even when
/// the window is in the background or closed to the tray.
///
/// While this front-end is attached the daemon stays quiet and leaves the telling
/// to us, so this has to cover both kinds of arrival. `auto` separates them: a
/// trusted sender's file is already downloading, and asking the user to approve it
/// would be offering a choice that has already been made. Only the other kind is a
/// question, and only that kind drops the wedge in the tray.
///
/// On macOS this goes through [`notify_mac`] rather than the plugin, whose backend
/// no longer delivers anything at all on current macOS; see that module. It needs a
/// built app — from `tauri dev` there is no bundle, so the call reports `false` and
/// we fall through to the plugin, which will also show nothing. A dev run being
/// silent on macOS is expected and not worth chasing.
fn notify_arrival(app: &AppHandle, name: &str, sender_name: &str, auto: bool) {
    let who = if sender_name.trim().is_empty() {
        "Qualcuno".to_string()
    } else {
        sender_name.to_string()
    };
    let (title, body) = if auto {
        (
            "Arvolo — sto scaricando",
            format!("“{name}” da {who}, che è un contatto fidato"),
        )
    } else {
        (
            "Arvolo — file in arrivo",
            format!("{who} vuole inviarti “{name}”"),
        )
    };
    #[cfg(target_os = "macos")]
    if notify_mac::available() {
        // One identifier per file name, so a re-offer of the same file replaces its
        // banner instead of stacking a second one.
        let id = format!("arvolo.arrival.{name}");
        let (title, body) = (title.to_string(), body);
        let _ = app.run_on_main_thread(move || {
            notify_mac::post(&id, &title, &body);
        });
        return;
    }
    let _ = app.notification().builder().title(title).body(body).show();
}
