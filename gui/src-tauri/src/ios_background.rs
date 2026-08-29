//! Keeping a transfer moving after the app has been put away, and stopping it
//! cleanly when the system says time is up.
//!
//! iOS suspends an app that goes to the background. A transfer running at that
//! moment is not paused — it is frozen, its connection dies somewhere the app
//! cannot see, and what comes back is a *failure* whose resume record has
//! already been dropped: the partial survives, but the only way on is for the
//! sender to offer the file again. That is the difference between "put the phone
//! in your pocket while a photo goes" and "start again".
//!
//! Two things fix it, and they have to be in this order.
//!
//! **Ask for the time.** `beginBackgroundTask` tells the system this app has
//! something to finish; it answers with a stretch of running time — tens of
//! seconds, never promised, never long — and a handler it calls when that time
//! is up. Small files simply finish inside it.
//!
//! **Spend the end of it stopping.** The handler is the one moment the app is
//! still alive and knows it is about to stop being. It pauses what is still
//! moving, which keeps the partial *and* the resume record and marks the stop as
//! the machine's rather than a person's — so the way back, and the next launch,
//! both know to pick it up again, and both leave alone whatever the person
//! paused on purpose.
//!
//! What this deliberately does not do is ask for a background *mode*. Those are
//! for apps whose work is the background — audio, navigation — and an app that
//! claims one to keep a file transfer alive is an app that gets rejected, and
//! deserves to be. The rule here is the honest one: as long as the system
//! allows, and then a clean stop.

#![cfg(target_os = "ios")]

use std::sync::{Mutex, OnceLock};

/// What to do when time is up. Set once, when the engine comes up.
static ON_EXPIRY: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// The task the system gave us, while we hold one.
static TASK: Mutex<Option<usize>> = Mutex::new(None);

/// Say how to stop. Called once at startup with the engine's own suspend.
pub fn set_on_expiry(f: impl Fn() + Send + Sync + 'static) {
    let _ = ON_EXPIRY.set(Box::new(f));
}

/// The app has gone to the background: ask for time, and arrange to stop at the
/// end of it. Safe to call twice — the second call is ignored rather than
/// leaking a second task the system would complain about.
pub fn entered_background(app: &tauri::AppHandle) {
    if TASK.lock().unwrap().is_some() {
        return;
    }
    let _ = app.run_on_main_thread(|| {
        let Some(mtm) = objc2_foundation::MainThreadMarker::new() else {
            return;
        };
        let ui = objc2_ui_kit::UIApplication::sharedApplication(mtm);
        let name = objc2_foundation::NSString::from_str("arvolo.transfers");
        // The handler runs on the main thread when the system decides the
        // stretch is over. Everything it does has to be quick and has to end
        // with giving the task back, or the app is killed instead of suspended.
        let handler = block2::RcBlock::new(move || {
            if let Some(f) = ON_EXPIRY.get() {
                f();
            }
            end_now();
        });
        let id = ui.beginBackgroundTaskWithName_expirationHandler(Some(&name), Some(&handler));
        *TASK.lock().unwrap() = Some(id);
    });
}

/// The app is back in front: give the time back before the system takes it.
/// Whatever was stopped at expiry is started again by the window, which asks the
/// engine for exactly the transfers the machine paused.
pub fn left_background(app: &tauri::AppHandle) {
    if TASK.lock().unwrap().is_none() {
        return;
    }
    let _ = app.run_on_main_thread(end_now);
}

fn end_now() {
    let Some(id) = TASK.lock().unwrap().take() else {
        return;
    };
    let Some(mtm) = objc2_foundation::MainThreadMarker::new() else {
        return;
    };
    let ui = objc2_ui_kit::UIApplication::sharedApplication(mtm);
    ui.endBackgroundTask(id);
}
