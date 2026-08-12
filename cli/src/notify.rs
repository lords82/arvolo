//! Best-effort desktop notifications for the daemon.
//!
//! When a file offer arrives that isn't auto-downloaded, the daemon has no
//! attached front-end to prompt — so it raises a native desktop notification
//! (D-Bus on Linux, Notification Center on macOS, toast on Windows) via
//! `notify-rust`. On a headless host (no session bus / no display) `show()`
//! simply errors and we drop it: the daemon already logged the offer, so the
//! notification is pure convenience, never the system of record.

/// Notify the user that `who` is offering `name` (`size` already humanized) and
/// it's waiting for approval. Runs the (blocking) platform call off the async
/// runtime; failures are ignored.
pub fn offer_awaiting(name: &str, who: &str, size: &str) {
    show(
        "Arvolo — incoming file",
        // No "run `arvolo accept`": this fires only when no front-end is attached,
        // but the user may still be about to open the app rather than a terminal.
        format!("{who} wants to send you {name} ({size}). It is waiting for you."),
    );
}

/// Notify the user that a file from a *trusted* `who` is being auto-downloaded —
/// no approval needed, but still surfaced so they know it's happening.
pub fn auto_downloading(name: &str, who: &str, size: &str) {
    show(
        "Arvolo — downloading",
        format!("Auto-downloading {name} ({size}) from trusted {who}."),
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
