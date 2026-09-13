//! Arvolo GUI backend.
//!
//! On a desktop the GUI never runs the transfer engine itself: it drives the same
//! background **daemon** the CLI uses, over the local IPC socket. This process is
//! a thin bridge —
//! * `bridge` exposes `#[tauri::command]`s that forward UI actions to the daemon,
//! * the **event pump** (below) keeps one subscription open and re-emits every
//!   engine event to the webview as `engine://event`, plus a `engine://connected`
//!   heartbeat and a native notification for incoming offers.
//!
//! A library, and not only a binary, because a phone loads it as one: there is no
//! process to start there, only a `.so` the platform's activity opens. `run` is
//! the same entry point on both — the desktop's `main` calls it, and the mobile
//! attribute below makes it what the activity finds.

mod attention;
mod bridge;
// Both are about a daemon in another process: whether its version matches ours,
// whether it is up, how to bring it up. A phone has neither the process nor the
// question.
#[cfg(desktop)]
mod compat;
#[cfg(desktop)]
mod daemon;
#[cfg(target_os = "ios")]
mod ios_background;
#[cfg(target_os = "ios")]
mod ios_bounce;
#[cfg(target_os = "ios")]
mod ios_open;
#[cfg(target_os = "macos")]
mod notify_mac;
// Where a finished download goes to live, on the one platform that has an
// opinion about it. Declared everywhere so its one table — what type a file is,
// which decides whether tapping it opens a viewer or a chooser — is compiled and
// tested on the machine this is developed on; the platform half inside is gated.
mod publish_android;
// The same seam on the other phone. Also declared everywhere: the naming rule
// inside it — what a second file of the same name is called — is a decision, and
// decisions are tested on a laptop.
mod publish_ios;
mod update;

use arvolo_ipc::protocol::EventDto;
#[cfg(desktop)]
use tauri::menu::{MenuBuilder, MenuItemBuilder};
#[cfg(desktop)]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;

/// Frontend event channels.
const EV_ENGINE: &str = "engine://event";
const EV_CONNECTED: &str = "engine://connected";
/// Why the daemon would not start, when it would not start.
#[cfg(desktop)]
const EV_DAEMON_ERROR: &str = "engine://daemon-error";
/// Files just dropped on the window, already registered: `[PickedItemDto]`.
const EV_FILES_PICKED: &str = "files://picked";
/// The ticket read out of a `.arvolo` file handed to the app — dropped on the
/// window, double-clicked (macOS `Opened`), or passed on argv. The webview gets
/// the ticket string, never the path.
const EV_ARVOLO_TICKET: &str = "files://arvolo";

/// The UI language, as the webview resolved it ("en"/"it"/"fr"/"de"). The six
/// native strings (tray menu, arrival notifications) read it; the webview sets
/// it on boot and on every change. Until it does, the OS locale decides — an
/// English desktop must not get an Italian tray while the app loads.
struct UiLang(std::sync::Mutex<String>);

fn os_lang() -> String {
    let raw = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default()
        .to_lowercase();
    for l in ["it", "fr", "de"] {
        if raw.starts_with(l) {
            return l.to_string();
        }
    }
    "en".to_string()
}

/// The native-side dictionary: six strings, four languages, no framework.
fn tr(lang: &str, key: &str) -> &'static str {
    match (lang, key) {
        ("it", "show") => "Mostra Arvolo",
        ("it", "quit") => "Esci",
        ("it", "someone") => "Qualcuno",
        ("it", "arrival_title") => "Arvolo — file in arrivo",
        ("it", "auto_title") => "Arvolo — sto scaricando",
        ("fr", "show") => "Afficher Arvolo",
        ("fr", "quit") => "Quitter",
        ("fr", "someone") => "Quelqu'un",
        ("fr", "arrival_title") => "Arvolo — fichier entrant",
        ("fr", "auto_title") => "Arvolo — téléchargement en cours",
        ("de", "show") => "Arvolo anzeigen",
        ("de", "quit") => "Beenden",
        ("de", "someone") => "Jemand",
        ("de", "arrival_title") => "Arvolo — eingehende Datei",
        ("de", "auto_title") => "Arvolo — lade herunter",
        (_, "show") => "Show Arvolo",
        (_, "quit") => "Quit",
        (_, "someone") => "Somebody",
        (_, "arrival_title") => "Arvolo — incoming file",
        (_, "auto_title") => "Arvolo — downloading",
        _ => "",
    }
}

fn arrival_body(lang: &str, auto: bool, name: &str, who: &str) -> String {
    match (lang, auto) {
        ("it", true) => format!("“{name}” da {who}, che è un contatto fidato"),
        ("it", false) => format!("{who} vuole inviarti “{name}”"),
        ("fr", true) => format!("« {name} » de {who}, un contact de confiance"),
        ("fr", false) => format!("{who} veut vous envoyer « {name} »"),
        ("de", true) => format!("„{name}“ von {who}, einem vertrauten Kontakt"),
        ("de", false) => format!("{who} möchte dir „{name}“ senden"),
        (_, true) => format!("“{name}” from {who}, a trusted contact"),
        (_, false) => format!("{who} wants to send you “{name}”"),
    }
}

/// The webview's language just resolved (boot, or the user switched): keep the
/// native strings in step — the tray menu is rebuilt in place.
#[tauri::command]
fn set_ui_language(app: AppHandle, state: tauri::State<'_, UiLang>, lang: String) {
    *state.0.lock().unwrap() = lang.clone();
    #[cfg(desktop)]
    rebuild_tray_menu(&app, &lang);
    // A phone has no menu bar to keep in step, and the webview is the only thing
    // on screen that speaks a language at all.
    #[cfg(mobile)]
    let _ = (app, lang);
}

/// The window has gone away, or come back.
///
/// Only iOS does anything with this, and only because only iOS freezes an app
/// for going to the background — see `ios_background` for what the time is for
/// and why it ends in a pause rather than a freeze. Android keeps the process
/// running and needs nothing; a desktop window that is merely covered is not
/// going anywhere at all.
#[tauri::command]
fn window_hidden(app: AppHandle, hidden: bool) {
    #[cfg(target_os = "ios")]
    {
        if hidden {
            ios_background::entered_background(&app);
        } else {
            ios_background::left_background(&app);
        }
    }
    #[cfg(not(target_os = "ios"))]
    let _ = (app, hidden);
}

/// Rewrite the tray menu in the language the webview just settled on.
#[cfg(desktop)]
fn rebuild_tray_menu(app: &AppHandle, lang: &str) {
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    let rebuilt = (|| -> tauri::Result<_> {
        let show = MenuItemBuilder::with_id("show", tr(lang, "show")).build(app)?;
        let quit = MenuItemBuilder::with_id("quit", tr(lang, "quit")).build(app)?;
        MenuBuilder::new(app)
            .item(&show)
            .separator()
            .item(&quit)
            .build()
    })();
    if let Ok(menu) = rebuilt {
        let _ = tray.set_menu(Some(menu));
    }
}

fn ui_lang(app: &AppHandle) -> String {
    app.try_state::<UiLang>()
        .map(|s| s.0.lock().unwrap().clone())
        .unwrap_or_else(os_lang)
}

/// Whether the system tray icon actually got created. Hiding the window on close
/// is only safe if there is a tray to get back from: on Linux desktops without a
/// StatusNotifier host (GNOME without extensions) there is none, and closing has
/// to quit instead of leaving an unreachable process behind.
struct HasTray(bool);

/// Keep the app's log to one readable file. Same bound and same one-generation
/// policy the daemon's log uses. Desktop only: a phone has no log file to bound
/// — its diagnostics go to the system log, which the system rotates.
#[cfg(desktop)]
const GUI_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Send this process's diagnostics somewhere a person can read them.
///
/// A bundled app writes to no terminal: without this, every `tracing::error!` and
/// every `log::error!` from Tauri itself went nowhere at all. That is not a
/// missing nicety — it is why "the update installed and the app never came back"
/// could only be investigated by reasoning about the source, with the one line
/// that would have said which step failed being written to a subscriber that did
/// not exist.
///
/// `fmt::init` also installs the `log` bridge, which is the half that matters
/// here: the updater's own failure paths inside Tauri use `log`, not `tracing`.
fn init_logging() {
    // A phone has no log file to write to — not yet, anyway: this runs before
    // anything knows where the app's own storage is (the engine settles that a
    // moment later), so the open below fails and every line the app ever logs
    // goes nowhere. It went unnoticed because nothing looked for them until
    // something went wrong on the device. Stdout is the answer there: on
    // Android it *is* the log, and `adb logcat` reads it.
    #[cfg(mobile)]
    {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("ARVOLO_LOG")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .with_ansi(false)
            .try_init();
        return;
    }
    #[cfg(desktop)]
    {
    arvolo_ipc::rotate_log_at(&arvolo_ipc::gui_log_path(), GUI_LOG_MAX_BYTES);
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(arvolo_ipc::gui_log_path())
    else {
        // Nowhere to write is not a reason to refuse to start; it only puts us
        // back where we were before this existed.
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("ARVOLO_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init();
    }
}

/// Build the app and run it. The whole of the backend's life.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    // `--autostart` is what the login entry passes (see the autostart plugin
    // registration below): the app was started by the OS, not by a person, so
    // it should come up hidden in the tray rather than with a window in the
    // face of someone who just signed in.
    let autostarted = std::env::args().any(|a| a == "--autostart");
    // A `.arvolo` passed on argv: the file-association launch on Windows/Linux
    // (macOS uses the Opened event instead). Read now, handed to the webview
    // when it asks (`take_pending_ticket`).
    let pending_ticket = std::env::args()
        .skip(1)
        .find_map(|a| read_arvolo_ticket(std::path::Path::new(&a)));

    let builder = tauri::Builder::default();
    // Starting at login, and replacing yourself with a newer build, are things a
    // desktop does. A phone is started by a person and updated by a store.
    // Reading a code off another screen is something only a device with a camera
    // in your hand does.
    #[cfg(mobile)]
    let builder = builder
        .plugin(tauri_plugin_barcode_scanner::init())
        .plugin(tauri_plugin_haptics::init());
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build());
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        // Registered for its Rust side alone — `Fs::open`, the only thing that
        // turns a picked `content://` into an open file (see
        // `bridge::open_content_uri`). The webview is granted none of its
        // commands: nothing in the capabilities names them, and a filesystem
        // the webview could call directly would undo the whole point of
        // `PickedFiles`. It has to be registered even so, because asking Tauri
        // for a plugin's state that was never managed is a *panic* — which is
        // exactly how this arrived, as a picked file that vanished.
        .plugin(tauri_plugin_fs::init())
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
            bridge::suspend_transfers,
            bridge::resume_suspended_transfers,
            bridge::deposit_now,
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
            bridge::rebind_contact,
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
            bridge::presence,
            bridge::sync_status,
            bridge::sync_now,
            bridge::list_devices,
            bridge::rename_this_device,
            bridge::forget_device,
            bridge::start_pairing,
            bridge::cancel_pairing,
            bridge::import_contacts,
            bridge::export_contacts,
            bridge::save_ticket,
            bridge::take_pending_ticket,
            set_ui_language,
            window_hidden,
            bridge::pick_files,
            bridge::gui_version,
            bridge::open_path,
            bridge::note,
            update::check_update,
            update::install_update,
        ])
        .setup(move |app| {
            #[cfg(desktop)]
            let has_tray = match setup_tray(app.handle()) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("arvolo: icona di stato non disponibile ({e})");
                    false
                }
            };
            // Nowhere to put an icon, and nothing to get a window back from: on a
            // phone the app is either in front of you or in the task switcher.
            #[cfg(mobile)]
            let has_tray = false;
            app.manage(HasTray(has_tray));
            // The window is declared hidden (`visible: false` in the config)
            // and shown from here, so there is nothing to hide and no flash: a
            // login launch with a tray to live in stays out of sight — the same
            // resting state as a window closed to the tray — and every other
            // launch (including a login launch on a desktop with no tray, which
            // would otherwise be unreachable) shows the window at once.
            if autostarted && has_tray {
                #[cfg(target_os = "macos")]
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            } else {
                show_main_window(app.handle());
            }
            app.manage(UiLang(std::sync::Mutex::new(os_lang())));
            app.manage(attention::State::default());
            app.manage(bridge::PickedFiles::new());
            app.manage(bridge::PendingArvolo(std::sync::Mutex::new(
                pending_ticket.clone(),
            )));
            // Ask now, not at the first arrival: a permission prompt that appears
            // the moment somebody sends you a file is in the way of the thing you
            // wanted to see. No-op when there is no bundle to ask for.
            #[cfg(target_os = "macos")]
            notify_mac::request_authorization();
            // The same, for the platforms that also gate it — and where nobody
            // was asking at all. A phone that is not asked simply drops every
            // notification the app posts (Android reports the app's importance
            // as NONE and counts them enqueued-but-never-posted), which is the
            // whole of "a file arrived while I wasn't looking and nothing told
            // me". Asked once, at launch, away from the arrival it is for.
            #[cfg(mobile)]
            {
                use tauri_plugin_notification::NotificationExt;
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    match handle.notification().permission_state() {
                        Ok(tauri_plugin_notification::PermissionState::Prompt)
                        | Ok(tauri_plugin_notification::PermissionState::PromptWithRationale) => {
                            if let Err(e) = handle.notification().request_permission() {
                                tracing::warn!("could not ask about notifications: {e}");
                            }
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("could not read notification permission: {e}"),
                    }
                });
            }
            // No daemon to dial here, so there is one to start: the engine runs
            // inside this process and the bridge asks it directly. Before the
            // event pump, which subscribes to whatever is answering.
            #[cfg(mobile)]
            match start_embedded_engine(app.handle()) {
                // stdout, not the log file: on Android that is logcat, and the
                // log file's own home is not known until the line above has run.
                Ok(dir) => println!("arvolo: engine up, keeping its files in {dir}"),
                Err(e) => eprintln!("arvolo: the engine did not start: {e:#}"),
            }
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
                // One `.arvolo` on its own is something to RECEIVE, not to send:
                // read the ticket here (the path stays Rust-side) and open the
                // receive flow with it. Mixed drops keep the send semantics.
                if let [only] = paths.as_slice() {
                    if let Some(ticket) = read_arvolo_ticket(only) {
                        let _ = app.emit(EV_ARVOLO_TICKET, ticket);
                        return;
                    }
                }
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
        // A `.arvolo` opened from the Finder (double click / "Open with") while
        // the app runs: macOS hands it over as an Opened event. The ticket goes
        // to the webview; the path never does.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = _event {
            for url in urls {
                if let Ok(path) = url.to_file_path() {
                    if let Some(ticket) = read_arvolo_ticket(&path) {
                        show_main_window(_app);
                        let _ = _app.emit(EV_ARVOLO_TICKET, ticket);
                    }
                }
            }
        }
    });
}

/// The platform's say in where a finished download comes to rest.
///
/// Android has one and it is MediaStore; everywhere else the engine's own
/// behaviour is already right, and `None` leaves it untouched.
#[cfg(mobile)]
fn publisher(app: &AppHandle) -> Option<std::sync::Arc<dyn arvolo_core::manager::SavePublisher>> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        Some(std::sync::Arc::new(publish_android::MediaStorePublisher))
    }
    #[cfg(target_os = "ios")]
    {
        // `Documents`, which the bundle's plist keys turn into "Arvolo" under
        // *On My iPhone* in Files. Asked of the platform rather than assembled
        // from the data dir: the two are siblings today and that is Apple's
        // arrangement to change, not ours.
        let dir = app.path().document_dir().ok()?;
        Some(std::sync::Arc::new(publish_ios::DocumentsPublisher { dir }))
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        None
    }
}

/// Bring the engine up inside this app, and point the bridge at it.
///
/// This is what `daemon.rs` does for a desktop, in one process instead of two.
/// Two things differ and both are about where files live: the engine keeps its
/// records wherever the config dir is, which on a phone is the app's private
/// storage and nowhere a person will ever look; and downloads land in a staging
/// folder there rather than in anybody's Downloads, because a receive writes in
/// place and only a finished file can be published. See docs/MOBILE.md.
#[cfg(mobile)]
fn start_embedded_engine(app: &AppHandle) -> anyhow::Result<String> {
    use anyhow::Context as _;

    let dir = app
        .path()
        .app_data_dir()
        .context("this app has no data directory")?;
    std::fs::create_dir_all(&dir).context("create the app data directory")?;
    // Every store in the engine resolves through this; setting it is the whole
    // of teaching them where they are.
    std::env::set_var("ARVOLO_CONFIG_DIR", &dir);
    arvolo_engine::book::apply_config_to_env();

    let relay = arvolo_engine::book::default_relay_or_builtin()
        .context("no relay configured and no built-in one to fall back to")?;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let running = tauri::async_runtime::block_on(arvolo_engine::start::start(
        arvolo_engine::start::Options {
            download_dir: Some(dir.join("incoming")),
            relay,
            auto_sync: true,
            // Nothing to announce that the window is not already drawing: it is
            // attached for as long as the app is on screen, and it is the only
            // thing on screen. What speaks when it is *not* — a push waking the
            // app for a deposit — is a later phase.
            announce: std::sync::Arc::new(arvolo_engine::announce::Silent),
            shutdown,
            // Out of the app's storage and into the user's Downloads, once the
            // download is whole. Android only: iOS publishes by renaming into
            // the app's own `Documents`, which is a folder in Files, and that
            // needs the plist keys before it needs code.
            publisher: publisher(app),
            // The mailbox, rebuilt. This engine dies every time the app is put
            // away, so without it the phone's home screen says nothing has ever
            // happened — with the files sitting in Documents saying otherwise.
            // A few hundred rows is a mailbox; the whole log has its own screen.
            restore_board: 300,
            front_ends: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        },
    ))?;
    bridge::use_embedded(std::sync::Arc::new(running.daemon.clone()));
    bridge::use_manager(running.daemon.manager.clone());
    // Whatever the last run left in the picked-copies folder and no send still
    // claims. Here rather than on a timer: the copies are only made while the
    // app is in front, and this is the moment after the engine has read the
    // records that say which ones are spoken for.
    bridge::sweep_picked(app);
    // What to do with the last moment of borrowed time, when the system takes
    // it back. Synchronous on purpose: the expiry handler has to finish before
    // it returns the task, and `suspend_running` is a pause, not a wait.
    #[cfg(target_os = "ios")]
    {
        let mgr = running.daemon.manager.clone();
        ios_background::set_on_expiry(move || {
            let n = mgr.suspend_running().len();
            println!("arvolo: background time is up — stopped {n} transfer(s)");
        });
    }
    let where_it_lives = dir.display().to_string();
    // Held for the life of the app: dropping it would take the inbox with it.
    app.manage(running);
    Ok(where_it_lives)
}

/// The ticket inside a `.arvolo` file, if `path` is one. The extension is what
/// makes a path eligible; the content is trimmed and must be non-empty.
fn read_arvolo_ticket(path: &std::path::Path) -> Option<String> {
    if path.extension().and_then(|e| e.to_str()) != Some("arvolo") {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// System-tray icon with a minimal menu (Mostra / Esci) — the way back to a
/// window closed to the tray, and the only one once the Dock/taskbar entry is
/// gone.
#[cfg(desktop)]
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let lang = os_lang();
    let show = MenuItemBuilder::with_id("show", tr(&lang, "show")).build(app)?;
    let quit = MenuItemBuilder::with_id("quit", tr(&lang, "quit")).build(app)?;
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
    // this is the resting state; `attention::apply` sets the mark breathing once
    // the pump knows something is waiting. `icons/attention/tray-0.png` is this very
    // file, so switching between them never resizes the mark.
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
        #[cfg(desktop)]
        let _ = w.unminimize();
        let _ = w.set_focus();
        // The ends of a scroll, which the layer under us turns off for everyone.
        #[cfg(target_os = "ios")]
        ios_bounce::restore(&w);
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
        // Everything down to the subscription is about a daemon in another
        // process: whether it is up, whether it was stopped on purpose, whether
        // to start one. On a phone the engine is in *this* process, there is
        // nothing to supervise, and asking would only find a socket that will
        // never exist.
        #[cfg(desktop)]
        {
            // An install is in flight and the daemon was stopped on purpose, so that
            // the installer can replace the binary it runs from. Say nothing and wait:
            // this is not a fault to report, and the app is on its way to restarting
            // into the new build anyway.
            if daemon::held_for_update() {
                let _ = app.emit(EV_CONNECTED, false);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
            // `arvolo daemon stop` leaves a marker: the user asked for it to be down,
            // and this loop must not win the argument by respawning it every two
            // seconds. Report disconnected and wait for the marker to go (a `daemon
            // start`/`run` removes it; so does the in-app restart below).
            if arvolo_ipc::stop_marker_path().exists() && !daemon::is_running().await {
                let _ = app.emit(
                EV_DAEMON_ERROR,
                "fermato con `arvolo daemon stop` — riavvialo da qui o con `arvolo daemon start`"
                    .to_string(),
            );
                let _ = app.emit(EV_CONNECTED, false);
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
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
        }

        let stream = match bridge::client().await {
            Ok(c) => c.subscribe().await,
            // A dial that failed, said in the same shape the subscribe would.
            Err(e) => Err(anyhow::anyhow!(e)),
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
/// question, and only that kind sets the tray breathing.
///
/// On macOS this goes through [`notify_mac`] rather than the plugin, whose backend
/// no longer delivers anything at all on current macOS; see that module. It needs a
/// built app — from `tauri dev` there is no bundle, so the call reports `false` and
/// we fall through to the plugin, which will also show nothing. A dev run being
/// silent on macOS is expected and not worth chasing.
fn notify_arrival(app: &AppHandle, name: &str, sender_name: &str, auto: bool) {
    let lang = ui_lang(app);
    let who = if sender_name.trim().is_empty() {
        tr(&lang, "someone").to_string()
    } else {
        sender_name.to_string()
    };
    let title = if auto {
        tr(&lang, "auto_title")
    } else {
        tr(&lang, "arrival_title")
    };
    let body = arrival_body(&lang, auto, name, &who);
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
