//! Best-effort desktop notifications for the daemon.
//!
//! When a file offer arrives that isn't auto-downloaded, the daemon has no
//! attached front-end to prompt — so it raises a native desktop notification
//! (D-Bus on Linux, Notification Center on macOS, toast on Windows) via
//! `notify-rust`. On a headless host (no session bus / no display) `show()`
//! simply errors and we drop it: the daemon already logged the offer, so the
//! notification is pure convenience, never the system of record.
//!
//! **None of this reaches the screen on macOS 26.** `notify-rust` goes through
//! `mac-notification-sys`, which posts via `NSUserNotificationCenter` — the API
//! Apple deprecated in 10.14 and that no longer delivers on 26. Verified on
//! 26.5.1: `set_application` succeeds, `show()` reports no error, and nothing is
//! delivered or even registered in `com.apple.ncprefs`. The GUI is in the same
//! boat, since Tauri's notification plugin sits on the same crate. Getting these
//! back means `UNUserNotificationCenter`, which needs a signed bundle asking for
//! authorization — something a CLI daemon cannot do at all, and the GUI can only
//! do from an installed .app, never under `tauri dev`. Linux and Windows are
//! unaffected.
//!
//! The strings here are Italian while the rest of the CLI prints English, and
//! that is deliberate. Everything else this binary writes goes to a terminal
//! someone opened on purpose; these land in the notification centre of a desktop,
//! next to the GUI's own — which are Italian — and often for someone who never
//! opened a terminal at all. Same surface, same language.

/// Notify the user that `who` is offering `name` (`size` already humanized) and
/// it's waiting for approval. Runs the (blocking) platform call off the async
/// runtime; failures are ignored.
pub fn offer_awaiting(name: &str, who: &str, size: &str) {
    show(
        "Arvolo — file in arrivo",
        // No "run `arvolo accept`": this fires only when no front-end is attached,
        // but the user may still be about to open the app rather than a terminal.
        format!("{who} vuole inviarti “{name}” ({size}). È in attesa di una risposta."),
    );
}

/// Notify the user that a file from a *trusted* `who` is being auto-downloaded —
/// no approval needed, but still surfaced so they know it's happening.
pub fn auto_downloading(name: &str, who: &str, size: &str) {
    show(
        "Arvolo — sto scaricando",
        format!("“{name}” ({size}) da {who}, che è un contatto fidato."),
    );
}

/// The same, for a file sent by another device of the user's own identity. A
/// separate line rather than a `who` passed into the one above: "da <id>, che è un
/// contatto fidato" is not what happened, and the id would mean nothing to read.
pub fn auto_downloading_own_device(name: &str, size: &str) {
    show(
        "Arvolo — sto scaricando",
        format!("“{name}” ({size}) da un altro dei tuoi dispositivi."),
    );
}

/// The installed app bundle. macOS attributes a notification to a bundle, not to
/// a process, and a bare CLI binary has none — so without this the daemon's
/// notifications are posted by whatever `mac-notification-sys` falls back to,
/// which is `com.apple.Finder`. They then arrive as Finder, or not at all if the
/// user has Finder's notifications turned off, which is why they looked missing.
#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "it.termox.arvolo";

/// Claim the bundle once per process. Fails when the app is not installed — a
/// daemon running from a plain `cargo build` on a machine without Arvolo.app —
/// and then we are no worse off than before.
#[cfg(target_os = "macos")]
fn claim_bundle() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Err(e) = notify_rust::set_application(BUNDLE_ID) {
            tracing::debug!("notifications will not be attributed to Arvolo: {e}");
        }
    });
}

/// Best-effort desktop notification, off the async runtime; failures ignored.
fn show(summary: &str, body: String) {
    let summary = summary.to_string();
    tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        claim_bundle();
        // `appname` is a no-op on macOS — there the application is whatever
        // `set_application` claimed — but it is what names the sender on Linux.
        let _ = notify_rust::Notification::new()
            .summary(&summary)
            .body(&body)
            .appname("arvolo")
            .show();
    });
}
