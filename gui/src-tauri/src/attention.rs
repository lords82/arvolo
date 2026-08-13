//! The tray icon's attention state.
//!
//! No counter and no badge: the mark says it itself. The wedge is the file in
//! flight, so when something is waiting on the user it **falls into the box** and
//! stays there; at rest it sits above, still in the air. That leaves the glyph
//! whole, which a corner badge never managed — at 36px a circle either shrinks to
//! an invisible sliver or eats the box's right arm until the mark reads as an L.
//!
//! The frames are pre-rendered by `icons/source/gen-attention.py`. Frame 0 is the
//! resting state, the last frame is the landed one that the tray holds for as long
//! as anything is pending, and the ones between are only played on the way in.
//! Nothing here is macOS-only: every platform goes through `TrayIcon::set_icon`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use arvolo_ipc::client::DaemonClient;
use tauri::image::Image;
use tauri::{AppHandle, Manager};

/// Frames of the wedge dropping, resting state first.
#[cfg(target_os = "macos")]
const FRAMES: [&[u8]; 16] = [
    include_bytes!("../icons/attention/tray-0.png"),
    include_bytes!("../icons/attention/tray-1.png"),
    include_bytes!("../icons/attention/tray-2.png"),
    include_bytes!("../icons/attention/tray-3.png"),
    include_bytes!("../icons/attention/tray-4.png"),
    include_bytes!("../icons/attention/tray-5.png"),
    include_bytes!("../icons/attention/tray-6.png"),
    include_bytes!("../icons/attention/tray-7.png"),
    include_bytes!("../icons/attention/tray-8.png"),
    include_bytes!("../icons/attention/tray-9.png"),
    include_bytes!("../icons/attention/tray-10.png"),
    include_bytes!("../icons/attention/tray-11.png"),
    include_bytes!("../icons/attention/tray-12.png"),
    include_bytes!("../icons/attention/tray-13.png"),
    include_bytes!("../icons/attention/tray-14.png"),
    include_bytes!("../icons/attention/tray-15.png"),
];
#[cfg(not(target_os = "macos"))]
const FRAMES: [&[u8]; 16] = [
    include_bytes!("../icons/attention/app-0.png"),
    include_bytes!("../icons/attention/app-1.png"),
    include_bytes!("../icons/attention/app-2.png"),
    include_bytes!("../icons/attention/app-3.png"),
    include_bytes!("../icons/attention/app-4.png"),
    include_bytes!("../icons/attention/app-5.png"),
    include_bytes!("../icons/attention/app-6.png"),
    include_bytes!("../icons/attention/app-7.png"),
    include_bytes!("../icons/attention/app-8.png"),
    include_bytes!("../icons/attention/app-9.png"),
    include_bytes!("../icons/attention/app-10.png"),
    include_bytes!("../icons/attention/app-11.png"),
    include_bytes!("../icons/attention/app-12.png"),
    include_bytes!("../icons/attention/app-13.png"),
    include_bytes!("../icons/attention/app-14.png"),
    include_bytes!("../icons/attention/app-15.png"),
];

const RESTING: usize = 0;
const LANDED: usize = FRAMES.len() - 1;
/// Three falls: enough to catch the eye without becoming a fidget. A menu bar
/// item that moves forever is an irritant, and it keeps the CPU awake for nothing.
const CYCLES: usize = 3;

/// The fall, as (frame, how long to hold it). The frames are evenly spaced, so the
/// shape of the movement lives here rather than in the bitmaps — which is why
/// retuning it costs nothing.
///
/// This is an **ease-out**: quick off the mark, slowing into the box. Picked by
/// eye against a linear ramp, an ease-in and an ease-in-with-bounce, all played
/// side by side at this exact size and timing. Physics would argue for the
/// ease-in — things fall faster, not slower — but over 32 units of 100, about 11px
/// once the menu bar has scaled it, the accelerating version reads as a stutter
/// that ends in a jump, and this one reads as a movement that arrives. Consecutive
/// repeats are collapsed into a single longer hold, so the tail is one 108ms rest
/// on the floor rather than three redundant repaints.
const FALL: [(usize, u64); 11] = [
    (0, 36),
    (2, 36),
    (4, 36),
    (6, 36),
    (8, 36),
    (9, 36),
    (11, 36),
    (12, 36),
    (13, 36),
    (14, 72),
    (15, 108),
];

/// A full second on the floor before the wedge goes back up and falls again. The
/// stillness is what makes the repeat legible: without it the reset to the top is
/// a jump cut, and a jump cut in a 36px glyph reads as a glitch rather than as
/// "here it comes again".
const PAUSE_MS: u64 = 1000;

/// Whether anything is pending, plus a generation counter that lets a newer call
/// retire an animation still in flight — otherwise a burst of arrivals would leave
/// several loops fighting over the icon.
#[derive(Default)]
pub struct State {
    pending: AtomicBool,
    generation: AtomicU64,
}

/// How many things are waiting for the user to decide.
///
/// Only decisions count. A finished download already fired its notification, and
/// folding those in would turn this into something to clear rather than something
/// to read. Trusted senders never land here either: the daemon auto-accepts them,
/// so they never reach the pending list and the wedge stays up.
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

/// Ask the daemon what is pending and set the tray to match. Quiet on failure: a
/// state that cannot be read is not worth an error in the UI, and the next event
/// runs this again anyway.
pub async fn refresh(app: &AppHandle) {
    if let Some(n) = pending_count().await {
        apply(app, n > 0);
    }
}

/// Drive the tray to `pending`. Going from nothing to something plays the drop;
/// staying pending just holds the landed frame, so a second arrival while one is
/// already waiting does not re-animate.
pub fn apply(app: &AppHandle, pending: bool) {
    let state = app.state::<State>();
    let was = state.pending.swap(pending, Ordering::SeqCst);
    // Retire whatever loop may be running: its generation is now stale.
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;

    if !pending {
        set_frame(app, RESTING);
        return;
    }
    if was {
        set_frame(app, LANDED);
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move { fall(app, generation).await });
}

/// Play the drop, then leave the wedge in the box. Bails the moment a newer call
/// bumps the generation, so the last caller always owns the final frame.
async fn fall(app: AppHandle, generation: u64) {
    let live = || {
        app.try_state::<State>()
            .is_some_and(|s| s.generation.load(Ordering::SeqCst) == generation)
    };
    for cycle in 0..CYCLES {
        for (frame, hold) in FALL {
            if !live() {
                return;
            }
            set_frame(&app, frame);
            tokio::time::sleep(Duration::from_millis(hold)).await;
        }
        if cycle + 1 < CYCLES {
            if !live() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(PAUSE_MS)).await;
        }
    }
    if live() {
        set_frame(&app, LANDED);
    }
}

/// Paint one frame onto the tray icon.
fn set_frame(app: &AppHandle, frame: usize) {
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    let Ok(icon) = Image::from_bytes(FRAMES[frame]) else {
        return;
    };
    // Atomic on macOS: setting the icon and the template flag separately makes the
    // menu bar render it twice and flicker — which an animation would show off.
    let _ = tray.set_icon_with_as_template(Some(icon), cfg!(target_os = "macos"));
}
