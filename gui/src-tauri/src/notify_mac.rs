//! Desktop notifications on macOS, posted through `UNUserNotificationCenter`.
//!
//! Tauri's notification plugin goes through `notify-rust` → `mac-notification-sys`
//! → `NSUserNotificationCenter`: the API Apple deprecated in 10.14 and stopped
//! delivering entirely. Nothing about that call fails — `show()` returns `Ok`, no
//! error is logged, and the notification simply never exists. That is why arrivals
//! looked silent on this platform while Linux and Windows were fine.
//!
//! `UNUserNotificationCenter` is the replacement, and it asks for two things the
//! old API did not:
//!
//! 1. **A bundle.** The center is keyed to the app bundle, and asking for it from a
//!    process that has no bundle identifier raises an Objective-C exception —
//!    which, from Rust, is an abort, not an error to handle. So every entry point
//!    here checks [`available`] first. That check is also why a `tauri dev` run
//!    still shows nothing: the binary Cargo built is not inside an `.app`, so there
//!    is no bundle to key on. Notifications need a built app (`tauri build`, or
//!    `tauri build --debug` for a quick one) — this is a property of the platform,
//!    not something the code can route around.
//! 2. **Authorization.** The first request pops the system prompt; the answer is
//!    remembered per bundle id, so it is asked once per install, not once per run.
//!    A refusal is not an error either: `add` still succeeds and the notification
//!    is silently dropped by the system, exactly as the user asked.
//!
//! Everything is dispatched to the main thread. UN itself is thread-safe, but the
//! authorization prompt is UI, and a background thread asking for it can land the
//! prompt behind the app.

use objc2::rc::Retained;
use objc2_foundation::{NSBundle, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNNotificationSound, UNUserNotificationCenter,
};

/// Whether this process is inside an app bundle, i.e. whether the notification
/// center can be asked for at all.
///
/// Checked before *every* call into UN, not cached behind a flag someone might
/// forget to consult: the cost is one `NSBundle` lookup and the alternative is an
/// abort.
pub fn available() -> bool {
    NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|id| !id.to_string().is_empty())
        .unwrap_or(false)
}

fn center() -> Option<Retained<UNUserNotificationCenter>> {
    available().then(UNUserNotificationCenter::currentNotificationCenter)
}

/// Ask for permission to notify, once. Safe to call when unavailable (it does
/// nothing), and safe to call more than once (macOS prompts only the first time
/// for a given bundle id).
///
/// Called at startup rather than at the first arrival: a permission prompt that
/// appears the moment someone sends you a file is a prompt in the way of the thing
/// you actually wanted to see.
pub fn request_authorization() {
    let Some(center) = center() else {
        tracing::debug!(
            "no app bundle: macOS notifications are unavailable (expected under tauri dev)"
        );
        return;
    };
    let options = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound;
    let handler = block2::RcBlock::new(
        |granted: objc2::runtime::Bool, _err: *mut objc2_foundation::NSError| {
            if !granted.as_bool() {
                tracing::debug!("macOS notifications not authorized — arrivals will be silent");
            }
        },
    );
    center.requestAuthorizationWithOptions_completionHandler(options, &handler);
}

/// Post a notification. Returns whether it was handed to the system — `false` means
/// there is no bundle, and the caller should fall back to its other path.
///
/// `identifier` groups replacements: posting twice with the same one replaces the
/// first rather than stacking a second banner.
pub fn post(identifier: &str, title: &str, body: &str) -> bool {
    let Some(center) = center() else {
        return false;
    };
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    // `trigger: nil` is what "deliver now" means here; every other trigger is a
    // schedule.
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(identifier),
        &content,
        None,
    );
    center.addNotificationRequest_withCompletionHandler(&request, None);
    true
}

#[cfg(test)]
mod tests {
    /// The bundle guard is the one thing here that must never be wrong: asking
    /// `UNUserNotificationCenter` for the current center without a bundle raises an
    /// Objective-C exception, which from Rust is an abort — the GUI would die rather
    /// than fail to notify. A test binary is a bare executable with no bundle, i.e.
    /// exactly the `tauri dev` shape, so this exercises that path.
    #[test]
    fn without_a_bundle_every_entry_point_is_a_quiet_no_op() {
        assert!(!super::available(), "a test binary has no bundle");
        assert!(!super::post("arvolo.test", "t", "b"), "post must decline");
        // The one with no return value to check: it must simply not abort.
        super::request_authorization();
    }
}
