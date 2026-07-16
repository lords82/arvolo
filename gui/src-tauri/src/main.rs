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

mod bridge;
mod daemon;

use arvolo_ipc::client::DaemonClient;
use arvolo_ipc::protocol::EventDto;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;

/// Frontend event channels.
const EV_ENGINE: &str = "engine://event";
const EV_CONNECTED: &str = "engine://connected";

fn main() {
    tauri::Builder::default()
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
            bridge::create_link,
            bridge::accept_offer,
            bridge::reject_offer,
            bridge::pause,
            bridge::resume,
            bridge::cancel,
            bridge::remove,
            bridge::mark_verified,
            bridge::list_deposits,
            bridge::revoke_deposit,
            bridge::gui_version,
        ])
        .setup(|app| {
            setup_tray(app.handle())?;
            let handle = app.handle().clone();
            // The whole engine bridge is unix-only until the Windows named-pipe
            // phase; on Windows the app still opens (showing "disconnesso").
            #[cfg(unix)]
            tauri::async_runtime::spawn(event_pump(handle));
            #[cfg(not(unix))]
            let _ = handle;
            Ok(())
        })
        // Closing the window hides it to the tray: transfers keep running in the
        // daemon and arrivals keep notifying; "Esci" in the tray menu quits.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running the Arvolo GUI");
}

/// System-tray icon with a minimal menu (Mostra / Esci). Left-clicking the icon
/// re-opens the window hidden by the close button.
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
        .show_menu_on_left_click(true)
        .tooltip("Arvolo")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Keep a live subscription to the daemon and forward every event to the webview.
/// Reconnects (spawning the daemon if needed) whenever the stream drops, so the
/// UI recovers on its own after a daemon restart.
#[cfg(unix)]
async fn event_pump(app: AppHandle) {
    loop {
        // Make sure a daemon exists; if we can't bring one up, report disconnected
        // and retry shortly.
        if daemon::ensure_running().await.is_err() {
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

        // Loops while events arrive; exits on a closed (`Ok(None)`) or errored
        // stream, which drops us to the outer reconnect loop.
        while let Ok(Some(ev)) = stream.next().await {
            if let EventDto::OfferReceived {
                name, sender_name, ..
            } = &ev
            {
                notify_offer(&app, name, sender_name);
            }
            let _ = app.emit(EV_ENGINE, &ev);
        }

        let _ = app.emit(EV_CONNECTED, false);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// Fire a native notification for an incoming offer — so arrivals surface even
/// when the window is in the background or closed to the tray.
#[cfg(unix)]
fn notify_offer(app: &AppHandle, name: &str, sender_name: &str) {
    let who = if sender_name.trim().is_empty() {
        "Qualcuno".to_string()
    } else {
        sender_name.to_string()
    };
    let _ = app
        .notification()
        .builder()
        .title("Arvolo — file in arrivo")
        .body(format!("{who} vuole inviarti “{name}”"))
        .show();
}
