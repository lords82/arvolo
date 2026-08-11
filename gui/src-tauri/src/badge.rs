//! The count badge on the tray icon.
//!
//! The number is a circle knocked *out* of the mark with the count sitting in the
//! hole. Because the hole is transparent the same file reads on a light and a dark
//! bar, so one asset per count serves every platform and nothing here is
//! macOS-only: every platform goes through [`TrayIcon::set_icon`].
//!
//! The variants are pre-rendered — `icons/source/gen-badge.py` builds them, with
//! the digits baked in as outlines — because drawing text at runtime would mean a
//! font rasteriser in the GUI for ten glyphs that never change.

use arvolo_ipc::client::DaemonClient;
use tauri::image::Image;
use tauri::AppHandle;
// Only the Windows taskbar overlay needs this trait, for `get_webview_window`.
// `default_window_icon` and `tray_by_id` are inherent on `AppHandle`, so anywhere
// else this import is dead — and the CI builds Linux with `-D warnings`.
#[cfg(target_os = "windows")]
use tauri::Manager;

/// The unbadged menu-bar glyph: black on alpha, flagged as a template image so
/// macOS tints it to match the bar (white on dark, black on light) like every
/// other status item.
#[cfg(target_os = "macos")]
pub const PLAIN_TEMPLATE: &[u8] = include_bytes!("../icons/tray.png");

/// Counts 1..=9 then `9+`. On macOS these are template images and the badge bites
/// into the mark; elsewhere they are the full-colour icon, where the badge sits in
/// the corner and only removes orange.
#[cfg(target_os = "macos")]
const BADGES: [&[u8]; 10] = [
    include_bytes!("../icons/badge/tray-1.png"),
    include_bytes!("../icons/badge/tray-2.png"),
    include_bytes!("../icons/badge/tray-3.png"),
    include_bytes!("../icons/badge/tray-4.png"),
    include_bytes!("../icons/badge/tray-5.png"),
    include_bytes!("../icons/badge/tray-6.png"),
    include_bytes!("../icons/badge/tray-7.png"),
    include_bytes!("../icons/badge/tray-8.png"),
    include_bytes!("../icons/badge/tray-9.png"),
    include_bytes!("../icons/badge/tray-9plus.png"),
];
#[cfg(not(target_os = "macos"))]
const BADGES: [&[u8]; 10] = [
    include_bytes!("../icons/badge/app-1.png"),
    include_bytes!("../icons/badge/app-2.png"),
    include_bytes!("../icons/badge/app-3.png"),
    include_bytes!("../icons/badge/app-4.png"),
    include_bytes!("../icons/badge/app-5.png"),
    include_bytes!("../icons/badge/app-6.png"),
    include_bytes!("../icons/badge/app-7.png"),
    include_bytes!("../icons/badge/app-8.png"),
    include_bytes!("../icons/badge/app-9.png"),
    include_bytes!("../icons/badge/app-9plus.png"),
];

/// The Windows taskbar overlay slot is 16pt square — far too small for a digit to
/// survive, so it carries a plain disc that only says "there is something".
#[cfg(target_os = "windows")]
const OVERLAY: &[u8] = include_bytes!("../icons/badge/overlay.png");

/// How many things are waiting for the user to decide.
///
/// Only decisions count. A finished download already fired its notification, and
/// folding those in would turn the badge into something to clear rather than
/// something to read — it would never sit at zero long enough to mean anything.
async fn pending_count() -> Option<usize> {
    let mut c = DaemonClient::connect().await.ok()?;
    let offers = c.list_pending().await.ok()?.len();
    // A contact advertising a new display name blocks on the user the same way an
    // offer does: nothing moves until it is accepted or ignored.
    let names = c
        .list_contacts()
        .await
        .ok()?
        .iter()
        .filter(|k| !k.pending_name.is_empty())
        .count();
    Some(offers + names)
}

/// Ask the daemon what is pending and repaint the tray to match. Quiet on
/// failure: a badge that cannot be counted is not worth an error in the UI, and
/// the next event re-runs this anyway.
pub async fn refresh(app: &AppHandle) {
    if let Some(n) = pending_count().await {
        apply(app, n);
    }
}

/// Repaint the tray icon (and, on Windows, the taskbar overlay) for `count`.
pub fn apply(app: &AppHandle, count: usize) {
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };

    let icon = if count == 0 {
        plain(app)
    } else {
        // 10 variants cover 1..=9 and then everything above, which is `9+`.
        Image::from_bytes(BADGES[count.min(BADGES.len()) - 1]).ok()
    };
    if let Some(icon) = icon {
        // Atomic on macOS: setting the icon and the template flag separately makes
        // the menu bar render it twice and flicker.
        let _ = tray.set_icon_with_as_template(Some(icon), cfg!(target_os = "macos"));
    }

    #[cfg(target_os = "windows")]
    if let Some(w) = app.get_webview_window("main") {
        let overlay = (count > 0)
            .then(|| Image::from_bytes(OVERLAY).ok())
            .flatten();
        let _ = w.set_overlay_icon(overlay);
    }
}

/// The tray icon with no badge on it.
fn plain(app: &AppHandle) -> Option<Image<'static>> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        Image::from_bytes(PLAIN_TEMPLATE).ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        // `default_window_icon` hands back an `Image` borrowing the app, so it has
        // to be lifted off that borrow — `Image` holds a `Cow`, and `to_owned`
        // turns the borrowed half into an owned one.
        app.default_window_icon().cloned().map(Image::to_owned)
    }
}
