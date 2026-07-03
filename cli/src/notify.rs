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
        format!("{who} wants to send you {name} ({size}).\nApprove with `arvolo accept`."),
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

/// Best-effort desktop notification, off the async runtime; failures ignored.
fn show(summary: &str, body: String) {
    let summary = summary.to_string();
    tokio::task::spawn_blocking(move || {
        let _ = notify_rust::Notification::new()
            .summary(&summary)
            .body(&body)
            .appname("arvolo")
            .show();
    });
}
